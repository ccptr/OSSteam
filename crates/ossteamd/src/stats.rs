// SPDX-License-Identifier: GPL-2.0-or-later
//! Stats and achievements for games (`docs/spec/steamworks/isteamuserstats.md`
//! §1): the CM requests behind the shim's stats requests, the schema they
//! return, and achievement icons. The requests block, so a few threads of
//! their own run them; each answer goes back to the core as the response
//! to the shim's request.

use std::io;
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use ossteam_gameipc::{
    AchievementDef, AchievementPercent, GlobalAchievements, Image, ResponseBody, StatDef,
    StatValue, StatsSchema, StoreStats, Unlock, UserStats,
};
use ossteam_net::{CallError, Packet, Requester};
use ossteam_proto::steam::{
    CMsgClientGetUserStats, CMsgClientGetUserStatsResponse, CMsgClientStoreUserStats2,
    CMsgClientStoreUserStatsResponse, CPlayerGetGameAchievementsRequest, EMsg,
    c_msg_client_store_user_stats2, player,
};
use ossteam_types::{AppId, CancelToken, EResult, SteamId};
use ossteam_vdf::{Object, Value, binary};

use crate::core::Command;
use crate::gameipc::ConnId;

/// Where Steam's community pages load achievement icons from (§1.3).
pub const ICON_URL: &str = "https://shared.steamstatic.com/community_assets/images/apps";

const THREADS: usize = 4;
/// How long a CM request may take (§1.1).
const TIMEOUT: Duration = Duration::from_secs(15);
/// Icons are 64×64 JPEGs of a few KiB.
const MAX_ICON_BYTES: u64 = 1 << 20;
const MAX_ICON_SIDE: usize = 1024;
/// How long shutdown waits for requests in progress.
const STOP_WAIT: Duration = Duration::from_secs(2);

/// `EResult`s the shim is told (§2).
const NO_CONNECTION: u32 = EResult::NO_CONNECTION.0 as u32;
const FAIL: u32 = EResult::FAIL.0 as u32;
const OK: u32 = EResult::OK.0 as u32;

/// What to ask.
pub enum Ask {
    UserStats {
        user: SteamId,
    },
    Store {
        crc: u32,
        values: Vec<StatValue>,
        reset: bool,
    },
    Global,
    Icon {
        file: String,
    },
}

/// One request of a game connection.
pub struct Job {
    pub conn: ConnId,
    pub id: u32,
    pub app: AppId,
    /// The account.
    pub account: SteamId,
    /// The app's language, for the schema's display strings.
    pub language: String,
    /// `None` only for an icon, which needs no CM.
    pub requester: Option<Requester>,
    pub ask: Ask,
}

/// A finished job, for the core to send.
pub struct Done {
    pub conn: ConnId,
    pub id: u32,
    pub body: ResponseBody,
}

pub struct Handle {
    tx: Option<Sender<Job>>,
    queue: Receiver<Job>,
    done: Receiver<()>,
    threads: Vec<JoinHandle<()>>,
    stop: CancelToken,
}

impl Handle {
    /// `icons` is `<cache>/achievement-icons`.
    pub fn spawn(core: Sender<Command>, icons: PathBuf) -> io::Result<Self> {
        let (tx, rx) = crossbeam_channel::unbounded::<Job>();
        let (done_tx, done) = crossbeam_channel::bounded::<()>(0);
        let agent = ossteam_net::tls::http_agent();
        let stop = CancelToken::new();
        let threads = (0..THREADS)
            .map(|n| {
                let (rx, core, agent, done_tx, icons, stop) = (
                    rx.clone(),
                    core.clone(),
                    agent.clone(),
                    done_tx.clone(),
                    icons.clone(),
                    stop.clone(),
                );
                std::thread::Builder::new()
                    .name(format!("stats-{n}"))
                    .spawn(move || {
                        let _done = done_tx;
                        for job in rx {
                            let body = run(&job, &agent, &icons, &stop);
                            let done = Done {
                                conn: job.conn,
                                id: job.id,
                                body,
                            };
                            let _ = core.send(Command::Stats(done));
                        }
                    })
            })
            .collect::<io::Result<_>>()?;
        Ok(Self {
            tx: Some(tx),
            queue: rx,
            done,
            threads,
            stop,
        })
    }

    pub fn send(&self, job: Job) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(job);
        }
    }

    /// Drops queued requests and interrupts those in progress. Later
    /// calls do nothing.
    pub fn shutdown(&mut self) {
        let Some(tx) = self.tx.take() else {
            return;
        };
        drop(tx);
        self.stop.cancel();
        while self.queue.try_recv().is_ok() {}
        match self.done.recv_timeout(STOP_WAIT) {
            Err(RecvTimeoutError::Disconnected) | Ok(()) => {
                for t in self.threads.drain(..) {
                    let _ = t.join();
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                tracing::warn!("stats requests still running at shutdown");
            }
        }
    }
}

fn run(job: &Job, agent: &ureq::Agent, icons: &Path, stop: &CancelToken) -> ResponseBody {
    if let Ask::Icon { file } = &job.ask {
        return ResponseBody::Image(icon(agent, icons, job.app, file));
    }
    let Some(requester) = &job.requester else {
        unreachable!("the core sends CM requests with a requester");
    };
    match &job.ask {
        Ask::UserStats { user } => ResponseBody::UserStats(user_stats(job, requester, *user, stop)),
        Ask::Store { crc, values, reset } => {
            ResponseBody::StoreStats(store(job, requester, *crc, values, *reset, stop))
        }
        Ask::Global => ResponseBody::GlobalAchievements(global(job, requester, stop)),
        Ask::Icon { .. } => unreachable!("answered above"),
    }
}

/// The `EResult` the shim is told for a request that failed.
fn failure(e: &CallError) -> u32 {
    match e {
        CallError::Closed | CallError::Cancelled => NO_CONNECTION,
        CallError::Failed { eresult, .. } => eresult.0 as u32,
        _ => FAIL,
    }
}

fn user_stats(job: &Job, requester: &Requester, user: SteamId, stop: &CancelToken) -> UserStats {
    let failed = |result| UserStats {
        result,
        ..Default::default()
    };
    let body = CMsgClientGetUserStats {
        game_id: Some(u64::from(job.app.0)),
        crc_stats: Some(0),
        schema_local_version: Some(-1),
        steam_id_for_user: Some(user.0),
    };
    let packet = Packet::proto(EMsg::KEMsgClientGetUserStats, &body);
    let reply = match requester.request(packet, TIMEOUT, stop) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(app = %job.app, "fetching stats failed: {e}");
            return failed(failure(&e));
        }
    };
    if !reply.is(EMsg::KEMsgClientGetUserStatsResponse) {
        tracing::warn!(app = %job.app, emsg = reply.emsg, "unexpected reply to ClientGetUserStats");
        return failed(FAIL);
    }
    let r: CMsgClientGetUserStatsResponse = match reply.decode_body() {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(app = %job.app, "ClientGetUserStatsResponse does not decode: {e}");
            return failed(FAIL);
        }
    };
    if r.eresult() != EResult::OK.0 {
        tracing::info!(app = %job.app, eresult = r.eresult(), "Steam refused the stats");
        return failed(r.eresult() as u32);
    }
    let schema = match parse_schema(r.schema(), job.app, &job.language) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(app = %job.app, "the stats schema does not parse: {e}");
            return failed(FAIL);
        }
    };
    let unlocks = r
        .achievement_blocks
        .iter()
        .flat_map(|b| {
            let stat = b.achievement_id();
            b.unlock_time
                .iter()
                .take(32)
                .enumerate()
                .filter(|(_, t)| **t != 0)
                .map(move |(bit, &time)| Unlock {
                    stat,
                    bit: bit as u32,
                    time,
                })
        })
        .collect();
    UserStats {
        result: OK,
        steam_id: user.0,
        crc: r.crc_stats(),
        schema,
        values: r
            .stats
            .iter()
            .map(|s| StatValue {
                id: s.stat_id(),
                value: s.stat_value(),
            })
            .collect(),
        unlocks,
    }
}

fn store(
    job: &Job,
    requester: &Requester,
    crc: u32,
    values: &[StatValue],
    reset: bool,
    stop: &CancelToken,
) -> StoreStats {
    let failed = |result| StoreStats {
        result,
        ..Default::default()
    };
    let body = CMsgClientStoreUserStats2 {
        game_id: Some(u64::from(job.app.0)),
        settor_steam_id: Some(job.account.0),
        settee_steam_id: Some(job.account.0),
        crc_stats: Some(crc),
        explicit_reset: Some(reset),
        stats: values
            .iter()
            .map(|v| c_msg_client_store_user_stats2::Stats {
                stat_id: Some(v.id),
                stat_value: Some(v.value),
            })
            .collect(),
    };
    let packet = Packet::proto(EMsg::KEMsgClientStoreUserStats2, &body);
    let reply = match requester.request(packet, TIMEOUT, stop) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(app = %job.app, "storing stats failed: {e}");
            return failed(failure(&e));
        }
    };
    if !reply.is(EMsg::KEMsgClientStoreUserStatsResponse) {
        tracing::warn!(app = %job.app, emsg = reply.emsg, "unexpected reply to ClientStoreUserStats2");
        return failed(FAIL);
    }
    let r: CMsgClientStoreUserStatsResponse = match reply.decode_body() {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(app = %job.app, "ClientStoreUserStatsResponse does not decode: {e}");
            return failed(FAIL);
        }
    };
    if r.eresult() != EResult::OK.0 {
        tracing::info!(app = %job.app, eresult = r.eresult(), out_of_date = r.stats_out_of_date(), "Steam refused the stats");
    }
    StoreStats {
        result: r.eresult() as u32,
        crc: r.crc_stats(),
        reverted: r
            .stats_failed_validation
            .iter()
            .map(|f| StatValue {
                id: f.stat_id(),
                value: f.reverted_stat_value(),
            })
            .collect(),
        out_of_date: r.stats_out_of_date(),
    }
}

fn global(job: &Job, requester: &Requester, stop: &CancelToken) -> GlobalAchievements {
    let request = CPlayerGetGameAchievementsRequest {
        appid: Some(job.app.0),
        language: Some(job.language.clone()),
        hash_only: Some(false),
    };
    match requester.call::<player::GetGameAchievements>(&request, TIMEOUT, stop) {
        Ok(r) => GlobalAchievements {
            result: OK,
            achievements: r
                .achievements
                .iter()
                .filter_map(|a| {
                    let percent = a.player_percent_unlocked().trim().parse::<f32>().ok()?;
                    Some(AchievementPercent {
                        name: a.internal_name().to_owned(),
                        percent: percent.to_bits(),
                    })
                })
                .collect(),
        },
        Err(e) => {
            tracing::warn!(app = %job.app, "fetching achievement percentages failed: {e}");
            GlobalAchievements {
                result: failure(&e),
                achievements: Vec::new(),
            }
        }
    }
}

/// A file name the schema gives, which must be one plain component.
fn plain_file(file: &str) -> bool {
    !file.is_empty() && file != "." && file != ".." && !file.contains(['/', '\\', ':', '\0'])
}

fn icon(agent: &ureq::Agent, dir: &Path, app: AppId, file: &str) -> Image {
    let failed = || Image {
        result: FAIL,
        ..Default::default()
    };
    if !plain_file(file) {
        tracing::warn!(app = %app, file, "refusing an achievement icon name that is not a plain file name");
        return failed();
    }
    let path = dir.join(app.0.to_string()).join(file);
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(_) => match download(agent, &path, app, file) {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(app = %app, file, "downloading an achievement icon failed: {e}");
                return failed();
            }
        },
    };
    match decode_jpeg(&bytes) {
        Ok((width, height, rgba)) => Image {
            result: OK,
            width,
            height,
            rgba,
        },
        Err(e) => {
            tracing::warn!(app = %app, file, "an achievement icon does not decode: {e}");
            // Fetched again next time rather than kept broken.
            let _ = std::fs::remove_file(&path);
            failed()
        }
    }
}

fn download(agent: &ureq::Agent, path: &Path, app: AppId, file: &str) -> Result<Vec<u8>, String> {
    let url = format!("{ICON_URL}/{}/{file}", app.0);
    let bytes = agent
        .get(&url)
        .call()
        .map_err(|e| e.to_string())?
        .into_body()
        .with_config()
        .limit(MAX_ICON_BYTES)
        .read_to_vec()
        .map_err(|e| e.to_string())?;
    let dir = path.parent().expect("icon paths have a parent");
    let tmp = dir.join(format!(".{file}.part"));
    let stored = std::fs::create_dir_all(dir)
        .and_then(|()| std::fs::write(&tmp, &bytes))
        .and_then(|()| std::fs::rename(&tmp, path));
    if let Err(e) = stored {
        // The icon still serves this request.
        tracing::warn!("caching {}: {e}", path.display());
    }
    Ok(bytes)
}

/// Decodes a JPEG to RGBA: (width, height, pixels).
pub fn decode_jpeg(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    use zune_core::bytestream::ZCursor;
    use zune_core::colorspace::ColorSpace;
    use zune_core::options::DecoderOptions;
    use zune_jpeg::JpegDecoder;

    let options = DecoderOptions::default()
        .jpeg_set_out_colorspace(ColorSpace::RGBA)
        .set_max_width(MAX_ICON_SIDE)
        .set_max_height(MAX_ICON_SIDE);
    let mut d = JpegDecoder::new_with_options(ZCursor::new(bytes), options);
    let pixels = d.decode().map_err(|e| format!("{e:?}"))?;
    let info = d.info().ok_or("no image header")?;
    let (w, h) = (usize::from(info.width), usize::from(info.height));
    if pixels.len() != w * h * 4 {
        return Err(format!("{} bytes of pixels for {w}×{h} RGBA", pixels.len()));
    }
    Ok((w as u32, h as u32, pixels))
}

/// The schema in `ClientGetUserStatsResponse` (§1.2), with display strings
/// in `language`.
pub fn parse_schema(bytes: &[u8], app: AppId, language: &str) -> Result<StatsSchema, String> {
    if bytes.is_empty() {
        return Ok(StatsSchema::default());
    }
    let root = binary::parse_all(bytes, binary::Keys::Inline).map_err(|e| e.to_string())?;
    let app_key = app.0.to_string();
    let top = root
        .get_object(&app_key)
        .or_else(|| match root.iter().next() {
            Some((_, Value::Object(o))) if root.len() == 1 => Some(o),
            _ => None,
        })
        .ok_or("no object for the app")?;
    let num = |o: &Object, key: &str| o.get(key).and_then(Value::as_i64);
    let mut stats = Vec::new();
    if let Some(list) = top.get_object("stats") {
        for (key, v) in list.iter() {
            let Some(s) = v.as_object() else { continue };
            let Some(id) = num(s, "id")
                .or_else(|| key.parse().ok())
                .and_then(|i| u32::try_from(i).ok())
            else {
                continue;
            };
            let Some(kind) = stat_kind(s) else { continue };
            let default = s.get("default").map_or(0, |d| raw(d, kind));
            let mut achievements = Vec::new();
            if kind >= 4
                && let Some(bits) = s.get_object("bits")
            {
                for (bkey, b) in bits.iter() {
                    let Some(b) = b.as_object() else { continue };
                    let Some(bit) = num(b, "bit")
                        .or_else(|| bkey.parse().ok())
                        .and_then(|i| u32::try_from(i).ok())
                        .filter(|&i| i < 32)
                    else {
                        continue;
                    };
                    let display = b.get_object("display");
                    let text = |k: &str| {
                        display
                            .and_then(|d| d.get(k))
                            .map(|v| localized(v, language))
                            .unwrap_or_default()
                    };
                    let flag = |k: &str| {
                        display
                            .and_then(|d| d.get(k))
                            .and_then(|v| v.as_i64())
                            .is_some_and(|n| n != 0)
                    };
                    let (progress_stat, progress_min, progress_max) = progress(b, list);
                    achievements.push(AchievementDef {
                        bit,
                        name: b.get_str("name").unwrap_or_default().to_owned(),
                        display_name: text("name"),
                        description: text("desc"),
                        hidden: flag("hidden"),
                        icon: text("icon"),
                        icon_gray: text("icon_gray"),
                        progress_stat,
                        progress_min,
                        progress_max,
                    });
                }
            }
            stats.push(StatDef {
                id,
                name: if kind >= 4 {
                    String::new()
                } else {
                    s.get_str("name").unwrap_or_default().to_owned()
                },
                kind,
                default,
                achievements,
            });
        }
    }
    Ok(StatsSchema {
        version: num(top, "version").map_or(0, |v| v as u32),
        stats,
    })
}

/// A stat's kind, 1 to 5, from `type_int` or `type`.
fn stat_kind(s: &Object) -> Option<u32> {
    let v = s.get("type_int").or_else(|| s.get("type"))?;
    let kind = match v.as_i64() {
        Some(n) => u32::try_from(n).ok()?,
        None => match v.as_str()?.to_ascii_uppercase().as_str() {
            "INT" => 1,
            "FLOAT" => 2,
            "AVGRATE" => 3,
            "ACHIEVEMENTS" => 4,
            "GROUPACHIEVEMENTS" => 5,
            _ => return None,
        },
    };
    (1..=5).contains(&kind).then_some(kind)
}

/// A schema scalar as a stat's 32 bits: an `int32`, or a `float`'s bits
/// for kinds 2 and 3.
fn raw(v: &Value, kind: u32) -> u32 {
    if kind == 2 || kind == 3 {
        v.as_f32().unwrap_or(0.0).to_bits()
    } else {
        v.as_i64().map_or(0, |n| n as i32 as u32)
    }
}

/// A display string: by language, else English, else the first; or the
/// string itself.
fn localized(v: &Value, language: &str) -> String {
    match v {
        Value::Object(o) => o
            .get_str(language)
            .or_else(|| o.get_str("english"))
            .or_else(|| {
                o.iter()
                    .find(|(k, _)| *k != "token")
                    .and_then(|(_, v)| v.as_str())
            })
            .unwrap_or_default()
            .to_owned(),
        other => other.to_text_scalar().unwrap_or_default(),
    }
}

/// An achievement's progress: the stat's name and its limits in that
/// stat's 32 bits.
fn progress(b: &Object, stats: &Object) -> (String, u32, u32) {
    let Some(p) = b.get_object("progress") else {
        return Default::default();
    };
    let Some(value) = p.get_object("value") else {
        return Default::default();
    };
    if !value
        .get_str("operation")
        .is_some_and(|o| o.eq_ignore_ascii_case("statvalue"))
    {
        return Default::default();
    }
    let Some(name) = value.get_str("operand1").filter(|n| !n.is_empty()) else {
        return Default::default();
    };
    let kind = stats
        .iter()
        .filter_map(|(_, s)| s.as_object())
        .find(|s| s.get_str("name") == Some(name))
        .and_then(stat_kind)
        .unwrap_or(1);
    let limit = |k: &str| p.get(k).map_or(0, |v| raw(v, kind));
    (name.to_owned(), limit("min_val"), limit("max_val"))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn spacewar_schema() -> Vec<u8> {
        let text = r#"
"480"
{
    "gamename" "Spacewar"
    "version" "20"
    "stats"
    {
        "1"
        {
            "type" "4"
            "id" "1"
            "bits"
            {
                "0"
                {
                    "name" "ACH_WIN_ONE_GAME"
                    "bit" "0"
                    "display"
                    {
                        "name" { "english" "Winner" "german" "Gewinner" "token" "T1" }
                        "desc" { "english" "Win one game." }
                        "hidden" "0"
                        "icon" "win.jpg"
                        "icon_gray" "win_gray.jpg"
                    }
                }
                "1"
                {
                    "name" "ACH_TRAVEL_FAR_ACCUM"
                    "display"
                    {
                        "name" "Interstellar"
                        "hidden" "1"
                    }
                    "progress"
                    {
                        "min_val" "0"
                        "max_val" "5280"
                        "value" { "operation" "statvalue" "operand1" "FeetTraveled" }
                    }
                }
            }
        }
        "2" { "type" "1" "name" "NumGames" "default" "0" }
        "3" { "type_int" "2" "type" "FLOAT" "name" "FeetTraveled" "default" "1.5" }
        "4" { "type" "AVGRATE" "name" "AverageSpeed" "windowsize" "20" }
        "5" { "type" "9" "name" "Unknown" }
    }
}
"#;
        binary::to_vec(&ossteam_vdf::text::parse(text).unwrap())
    }

    #[test]
    fn parses_the_schema() {
        let s = parse_schema(&spacewar_schema(), AppId(480), "german").unwrap();
        assert_eq!(s.version, 20);
        let kinds: Vec<(u32, &str, u32)> = s
            .stats
            .iter()
            .map(|d| (d.id, d.name.as_str(), d.kind))
            .collect();
        assert_eq!(
            kinds,
            [
                (1, "", 4),
                (2, "NumGames", 1),
                (3, "FeetTraveled", 2),
                (4, "AverageSpeed", 3)
            ]
        );
        assert_eq!(s.stats[2].default, 1.5f32.to_bits());
        let a = &s.stats[0].achievements;
        assert_eq!(a.len(), 2);
        assert_eq!(
            (a[0].bit, a[0].name.as_str(), a[0].display_name.as_str()),
            (0, "ACH_WIN_ONE_GAME", "Gewinner")
        );
        // Falls back to English.
        assert_eq!(a[0].description, "Win one game.");
        assert_eq!(
            (a[0].icon.as_str(), a[0].icon_gray.as_str()),
            ("win.jpg", "win_gray.jpg")
        );
        assert!(!a[0].hidden);
        assert_eq!(
            (a[1].bit, a[1].display_name.as_str(), a[1].hidden),
            (1, "Interstellar", true)
        );
        assert_eq!(a[1].progress_stat, "FeetTraveled");
        assert_eq!(
            (a[1].progress_min, a[1].progress_max),
            (0f32.to_bits(), 5280f32.to_bits())
        );
        assert!(a[0].progress_stat.is_empty());
        // Nothing to parse is no stats.
        assert_eq!(
            parse_schema(&[], AppId(480), "english").unwrap(),
            StatsSchema::default()
        );
        assert!(parse_schema(&[1, 2, 3], AppId(480), "english").is_err());
    }

    #[test]
    fn icon_names_are_plain() {
        assert!(plain_file("4f8b3a.jpg"));
        for bad in ["", ".", "..", "a/b.jpg", "..\\x.jpg", "c:x"] {
            assert!(!plain_file(bad), "{bad}");
        }
    }

    #[test]
    fn decodes_icons() {
        let jpeg = include_bytes!("../testdata/icon.jpg");
        let (w, h, rgba) = decode_jpeg(jpeg).unwrap();
        assert_eq!((w, h), (4, 2));
        assert_eq!(rgba.len(), 4 * 2 * 4);
        assert!(rgba.chunks(4).all(|p| p[3] == 255));
        assert!(decode_jpeg(b"not a jpeg").is_err());
    }

    /// Downloads a real icon: run with `--ignored` to check the URL
    /// scheme still holds (§1.3).
    #[test]
    #[ignore = "needs the network"]
    fn fetches_an_icon_from_steam() {
        let dir = std::env::temp_dir().join(format!("ossteamd-icons-{}", std::process::id()));
        let agent = ossteam_net::tls::http_agent();
        let i = icon(&agent, &dir, AppId(620), "SURVIVE_CONTAINER_RIDE.jpg");
        assert_eq!((i.result, i.width, i.height), (OK, 64, 64));
        assert!(dir.join("620/SURVIVE_CONTAINER_RIDE.jpg").is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
