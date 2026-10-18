// SPDX-License-Identifier: GPL-2.0-or-later
//! `ISteamUserStats` (`docs/spec/steamworks/isteamuserstats.md`).

use std::collections::{BTreeSet, HashMap};
use std::ffi::{c_char, c_void};
use std::sync::{Arc, Mutex, MutexGuard};

use ossteam_gameipc::{
    AchievementDef, AchievementIconRequest, GlobalAchievementsRequest, RequestBody, ResponseBody,
    StatDef, StatValue, StatsSchema, StoreStatsRequest, UserStats, UserStatsRequest,
};

use crate::abi::{EMPTY, intern, put, read_str, write_str_truncated};
use crate::callbacks;
use crate::log::log;
use crate::object::Object;
use crate::pipe::{self, Pipe};
use crate::vtables::{SLOTS, VERSIONS};

const INT: u32 = 1;
const FLOAT: u32 = 2;
const AVGRATE: u32 = 3;
const ACHIEVEMENTS: u32 = 4;

/// `EResult`s.
const OK: u32 = 1;
const FAIL: u32 = 2;
const NO_CONNECTION: u32 = 3;
const INVALID_PARAM: u32 = 8;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// One user's stats as Steam sent them (§3.1).
struct UserData {
    crc: u32,
    schema: Arc<StatsSchema>,
    /// With the game's changes.
    values: HashMap<u32, u32>,
    /// As Steam last confirmed them, to tell which achievements a store
    /// unlocks.
    stored: HashMap<u32, u32>,
    /// By (stat, bit).
    unlocks: HashMap<(u32, u32), u32>,
}

impl UserData {
    fn new(u: UserStats) -> Self {
        let values: HashMap<u32, u32> = u.values.iter().map(|v| (v.id, v.value)).collect();
        Self {
            crc: u.crc,
            stored: values.clone(),
            values,
            unlocks: u
                .unlocks
                .iter()
                .map(|x| ((x.stat, x.bit), x.time))
                .collect(),
            schema: Arc::new(u.schema),
        }
    }

    fn value(&self, stat: &StatDef) -> u32 {
        self.values.get(&stat.id).copied().unwrap_or(stat.default)
    }

    fn stored(&self, stat: &StatDef) -> u32 {
        self.stored.get(&stat.id).copied().unwrap_or(stat.default)
    }

    /// The stat of kind 1 to 3 called `name`.
    fn stat(&self, name: &str) -> Option<&StatDef> {
        self.schema
            .stats
            .iter()
            .find(|s| (INT..=AVGRATE).contains(&s.kind) && s.name == name)
    }

    /// The achievement called `name`, with its block.
    fn achievement(&self, name: &str) -> Option<(&StatDef, &AchievementDef)> {
        achievements(&self.schema).find(|(_, a)| a.name == name)
    }

    fn achieved(&self, block: &StatDef, a: &AchievementDef) -> bool {
        self.value(block) & (1 << a.bit) != 0
    }

    fn unlock_time(&self, block: &StatDef, a: &AchievementDef) -> u32 {
        if self.achieved(block, a) {
            self.unlocks.get(&(block.id, a.bit)).copied().unwrap_or(0)
        } else {
            0
        }
    }
}

/// The achievements of type-4 blocks, in the schema's order.
fn achievements(schema: &StatsSchema) -> impl Iterator<Item = (&StatDef, &AchievementDef)> {
    schema
        .stats
        .iter()
        .filter(|s| s.kind == ACHIEVEMENTS)
        .flat_map(|s| s.achievements.iter().map(move |a| (s, a)))
}

/// A pipe's stats (§3.1).
#[derive(Default)]
pub struct Stats {
    own: Option<UserData>,
    fetching: bool,
    /// A version without `RequestCurrentStats` was handed out: fetch
    /// without being asked.
    auto: bool,
    /// Whether every version handed out is `003`, whose
    /// `UserStatsReceived_t` has no Steam id. `None` before the first.
    legacy: Option<bool>,
    /// Stats changed since the last store.
    changed: BTreeSet<u32>,
    others: HashMap<u64, UserData>,
    /// Ordered from the most achieved down.
    global: Option<Vec<(String, f32)>>,
    /// Image handles by icon file; 0 while one is being fetched.
    icons: HashMap<String, i32>,
}

fn game(p: &Pipe) -> u64 {
    u64::from(p.info().app.app_id)
}

fn received_size(legacy: bool) -> usize {
    if legacy {
        callbacks::game_result(0, 0).len()
    } else {
        callbacks::user_stats_received(0, 0, 0).len()
    }
}

/// A version of `ISteamUserStats` was handed out on `p`.
pub fn handed_out(p: &Pipe, version: usize) {
    let v = &VERSIONS[version];
    let short = v.name.to_bytes() == b"STEAMUSERSTATS_INTERFACE_VERSION003";
    let asks = (0..v.vtable.len()).any(|i| SLOTS[v.first_slot + i].method == "RequestCurrentStats");
    let fetch_now = {
        let mut s = lock(&p.stats);
        s.legacy = Some(s.legacy.unwrap_or(true) && short);
        if asks || s.auto {
            false
        } else {
            s.auto = true;
            s.own.is_none()
        }
    };
    if fetch_now && p.logged_on() {
        fetch(p);
    }
}

/// The account logged on to Steam (§3.1).
pub fn logged_on(p: &Pipe) {
    let due = {
        let s = lock(&p.stats);
        s.auto && s.own.is_none() && !s.fetching
    };
    if due {
        fetch(p);
    }
}

/// Fetches the account's stats; `UserStatsReceived_t` follows. Returns
/// whether the request went out (or one already had).
fn fetch(p: &Pipe) -> bool {
    {
        let mut s = lock(&p.stats);
        if s.fetching {
            return true;
        }
        s.fetching = true;
    }
    let sent = p.send_then(
        RequestBody::UserStats(UserStatsRequest { steam_id: 0 }),
        Box::new(|p, body| {
            let own_id = p.info().session.steam_id;
            let mut s = lock(&p.stats);
            s.fetching = false;
            let (result, user) = match body {
                Some(ResponseBody::UserStats(u)) if u.result == OK => {
                    let user = u.steam_id;
                    s.own = Some(UserData::new(u.clone()));
                    s.changed.clear();
                    (OK, user)
                }
                Some(ResponseBody::UserStats(u)) => (u.result, own_id),
                Some(ResponseBody::Error(e)) => {
                    log!(Warn, "fetching stats: {}", e.message);
                    (FAIL, own_id)
                }
                Some(_) => (FAIL, own_id),
                None => (NO_CONNECTION, own_id),
            };
            let legacy = s.legacy == Some(true);
            drop(s);
            let g = game(p);
            let data = if legacy {
                callbacks::game_result(g, result as i32)
            } else {
                callbacks::user_stats_received(g, result as i32, user)
            };
            p.push(callbacks::USER_STATS_RECEIVED, data);
        }),
    );
    if !sent {
        lock(&p.stats).fetching = false;
    }
    sent
}

/// Runs `f` on the pipe's stats.
fn with<R>(o: &Object, default: R, f: impl FnOnce(&Pipe, &mut Stats) -> R) -> R {
    match pipe::get(o.pipe) {
        Some(p) => {
            let mut s = lock(&p.stats);
            f(&p, &mut s)
        }
        None => default,
    }
}

/// Runs `f` on the account's stats, once they have arrived.
fn with_own<R>(o: &Object, default: R, f: impl FnOnce(&mut UserData) -> R) -> R {
    with(o, None, |_, s| s.own.as_mut().map(f)).unwrap_or(default)
}

/// Reads a name the game passes.
fn name<'a>(p: *mut c_void) -> Option<&'a str> {
    // SAFETY: the SDK declares a NUL-terminated string.
    unsafe { read_str(p) }
}

pub fn request_current_stats(o: &Object) -> bool {
    match pipe::get(o.pipe) {
        Some(p) if p.logged_on() => fetch(&p),
        _ => false,
    }
}

/// Reads stat `n` into `out` when its kind is one of `kinds`.
fn get_value(data: &UserData, n: *mut c_void, kinds: &[u32], out: *mut c_void) -> bool {
    let Some(stat) = name(n).and_then(|n| data.stat(n)) else {
        return false;
    };
    if !kinds.contains(&stat.kind) {
        return false;
    }
    // SAFETY: the SDK declares an `int32 *` or a `float *`, both 4 bytes.
    unsafe { put(out, data.value(stat)) };
    true
}

pub fn get_stat_int(o: &Object, n: *mut c_void, out: *mut c_void) -> bool {
    with_own(o, false, |d| get_value(d, n, &[INT], out))
}

pub fn get_stat_float(o: &Object, n: *mut c_void, out: *mut c_void) -> bool {
    with_own(o, false, |d| get_value(d, n, &[FLOAT, AVGRATE], out))
}

fn set_value(o: &Object, n: *mut c_void, kind: u32, value: u32) -> bool {
    with(o, false, |_, s| {
        let Some(d) = s.own.as_mut() else {
            return false;
        };
        let Some(stat) = name(n).and_then(|n| d.stat(n)).filter(|s| s.kind == kind) else {
            return false;
        };
        let id = stat.id;
        d.values.insert(id, value);
        s.changed.insert(id);
        true
    })
}

pub fn set_stat_int(o: &Object, n: *mut c_void, value: i32) -> bool {
    set_value(o, n, INT, value as u32)
}

pub fn set_stat_float(o: &Object, n: *mut c_void, value: f32) -> bool {
    set_value(o, n, FLOAT, value.to_bits())
}

pub fn get_achievement(o: &Object, n: *mut c_void, achieved: *mut c_void) -> bool {
    get_achievement_and_unlock_time(o, n, achieved, std::ptr::null_mut())
}

pub fn get_achievement_and_unlock_time(
    o: &Object,
    n: *mut c_void,
    achieved: *mut c_void,
    time: *mut c_void,
) -> bool {
    with_own(o, false, |d| user_achievement(d, n, achieved, time))
}

fn user_achievement(
    d: &UserData,
    n: *mut c_void,
    achieved: *mut c_void,
    time: *mut c_void,
) -> bool {
    let Some((block, a)) = name(n).and_then(|n| d.achievement(n)) else {
        return false;
    };
    // SAFETY: the SDK declares a `bool *` and a `uint32 *`.
    unsafe {
        put(achieved, u8::from(d.achieved(block, a)));
        put(time, d.unlock_time(block, a));
    }
    true
}

/// Sets or clears an achievement's bit.
fn mark(o: &Object, n: *mut c_void, set: bool) -> bool {
    with(o, false, |p, s| {
        let Some(d) = s.own.as_mut() else {
            return false;
        };
        let Some((block, a)) = name(n).and_then(|n| d.achievement(n)) else {
            return false;
        };
        let (id, bit) = (block.id, a.bit);
        let was = d.achieved(block, a);
        let value = d.value(block);
        let value = if set {
            value | (1 << bit)
        } else {
            value & !(1 << bit)
        };
        d.values.insert(id, value);
        if set && !was {
            d.unlocks.insert((id, bit), now(p));
        } else if !set {
            d.unlocks.remove(&(id, bit));
        }
        s.changed.insert(id);
        true
    })
}

/// Steam's clock, or the computer's before there is one.
fn now(p: &Pipe) -> u32 {
    match p.info().server_time() {
        0 => std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs().min(u64::from(u32::MAX)) as u32),
        t => t,
    }
}

pub fn set_achievement(o: &Object, n: *mut c_void) -> bool {
    mark(o, n, true)
}

pub fn clear_achievement(o: &Object, n: *mut c_void) -> bool {
    mark(o, n, false)
}

pub fn store_stats(o: &Object) -> bool {
    pipe::get(o.pipe).is_some_and(|p| store(&p, false))
}

pub fn reset_all_stats(o: &Object, achievements_too: bool) -> bool {
    let Some(p) = pipe::get(o.pipe) else {
        return false;
    };
    {
        let mut s = lock(&p.stats);
        let s = &mut *s;
        let Some(d) = s.own.as_mut() else {
            return false;
        };
        let schema = Arc::clone(&d.schema);
        for stat in &schema.stats {
            if (INT..=AVGRATE).contains(&stat.kind) {
                d.values.insert(stat.id, stat.default);
            } else if achievements_too {
                d.values.insert(stat.id, 0);
                d.unlocks.retain(|(id, _), _| *id != stat.id);
            } else {
                continue;
            }
            s.changed.insert(stat.id);
        }
    }
    store(&p, true)
}

/// Sends the changed stats (§3.1). False when the stats have not
/// arrived or the pipe is closed.
fn store(p: &Pipe, reset: bool) -> bool {
    if p.is_closed() {
        return false;
    }
    let (crc, values, unlocked) = {
        let mut s = lock(&p.stats);
        let s = &mut *s;
        let Some(d) = s.own.as_ref() else {
            return false;
        };
        let changed = std::mem::take(&mut s.changed);
        let mut values = Vec::new();
        let mut unlocked = Vec::new();
        for stat in d.schema.stats.iter().filter(|st| changed.contains(&st.id)) {
            let value = d.value(stat);
            values.push(StatValue { id: stat.id, value });
            if stat.kind == ACHIEVEMENTS {
                let before = d.stored(stat);
                for a in &stat.achievements {
                    let bit = 1 << a.bit;
                    if value & bit != 0 && before & bit == 0 {
                        unlocked.push((stat.id, a.bit, a.name.clone()));
                    }
                }
            }
        }
        (d.crc, values, unlocked)
    };
    let ids: Vec<u32> = values.iter().map(|v| v.id).collect();
    let sent_values = values.clone();
    let sent = p.send_then(
        RequestBody::StoreStats(StoreStatsRequest { crc, values, reset }),
        Box::new(move |p, body| stored(p, body, sent_values, unlocked)),
    );
    if !sent {
        lock(&p.stats).changed.extend(ids);
    }
    sent
}

/// The daemon answered a store (§3.1).
fn stored(
    p: &Pipe,
    body: Option<&ResponseBody>,
    sent: Vec<StatValue>,
    unlocked: Vec<(u32, u32, String)>,
) {
    let mut refetch = false;
    let mut announce = Vec::new();
    let result = {
        let mut guard = lock(&p.stats);
        let s = &mut *guard;
        match (body, s.own.as_mut()) {
            (Some(ResponseBody::StoreStats(r)), _) if r.out_of_date => {
                // Steam keeps what it has; the fetch replaces the changes.
                refetch = true;
                s.changed.clear();
                if r.result == OK { FAIL } else { r.result }
            }
            (Some(ResponseBody::StoreStats(r)), Some(d)) if r.result == OK => {
                d.crc = r.crc;
                for v in &sent {
                    d.stored.insert(v.id, v.value);
                }
                for v in &r.reverted {
                    d.values.insert(v.id, v.value);
                    d.stored.insert(v.id, v.value);
                }
                announce = unlocked
                    .into_iter()
                    .filter(|(id, bit, _)| d.stored.get(id).is_some_and(|v| v & (1 << bit) != 0))
                    .map(|(_, _, n)| n)
                    .collect();
                if r.reverted.is_empty() {
                    OK
                } else {
                    INVALID_PARAM
                }
            }
            (other, _) => {
                // Sent again with the next store.
                s.changed.extend(sent.iter().map(|v| v.id));
                match other {
                    Some(ResponseBody::StoreStats(r)) => r.result,
                    Some(ResponseBody::Error(e)) => {
                        log!(Warn, "storing stats: {}", e.message);
                        FAIL
                    }
                    Some(_) => FAIL,
                    None => NO_CONNECTION,
                }
            }
        }
    };
    let g = game(p);
    for n in announce {
        p.push(
            callbacks::USER_ACHIEVEMENT_STORED,
            callbacks::user_achievement_stored(g, &n, 0, 0),
        );
    }
    p.push(
        callbacks::USER_STATS_STORED,
        callbacks::game_result(g, result as i32),
    );
    if refetch {
        fetch(p);
    }
}

pub fn indicate_achievement_progress(o: &Object, n: *mut c_void, cur: u32, max: u32) -> bool {
    let Some(p) = pipe::get(o.pipe) else {
        return false;
    };
    let found = {
        let s = lock(&p.stats);
        s.own.as_ref().and_then(|d| {
            let (block, a) = d.achievement(name(n)?)?;
            (!d.achieved(block, a)).then(|| a.name.clone())
        })
    };
    let Some(achievement) = found else {
        return false;
    };
    p.push(
        callbacks::USER_ACHIEVEMENT_STORED,
        callbacks::user_achievement_stored(game(&p), &achievement, cur, max),
    );
    true
}

pub fn num_achievements(o: &Object) -> u32 {
    with_own(o, 0, |d| achievements(&d.schema).count() as u32)
}

pub fn achievement_name(o: &Object, i: u32) -> *const c_char {
    with_own(o, EMPTY.as_ptr(), |d| {
        achievements(&d.schema)
            .nth(i as usize)
            .map_or(EMPTY.as_ptr(), |(_, a)| intern(&a.name))
    })
}

pub fn achievement_display_attribute(
    o: &Object,
    n: *mut c_void,
    key: *mut c_void,
) -> *const c_char {
    with_own(o, EMPTY.as_ptr(), |d| {
        let Some((_, a)) = name(n).and_then(|n| d.achievement(n)) else {
            return EMPTY.as_ptr();
        };
        match name(key) {
            Some("name") => intern(&a.display_name),
            Some("desc") => intern(&a.description),
            Some("hidden") => intern(if a.hidden { "1" } else { "0" }),
            _ => EMPTY.as_ptr(),
        }
    })
}

/// Reads an achievement's progress limits when its progress stat's kind
/// is one of `kinds`.
fn progress_limits(
    o: &Object,
    n: *mut c_void,
    kinds: &[u32],
    min: *mut c_void,
    max: *mut c_void,
) -> bool {
    with_own(o, false, |d| {
        let Some((_, a)) = name(n).and_then(|n| d.achievement(n)) else {
            return false;
        };
        if !d
            .stat(&a.progress_stat)
            .is_some_and(|s| kinds.contains(&s.kind))
        {
            return false;
        }
        // SAFETY: the SDK declares two `int32 *` or two `float *`.
        unsafe {
            put(min, a.progress_min);
            put(max, a.progress_max);
        }
        true
    })
}

pub fn progress_limits_int(o: &Object, n: *mut c_void, min: *mut c_void, max: *mut c_void) -> bool {
    progress_limits(o, n, &[INT], min, max)
}

pub fn progress_limits_float(
    o: &Object,
    n: *mut c_void,
    min: *mut c_void,
    max: *mut c_void,
) -> bool {
    progress_limits(o, n, &[FLOAT, AVGRATE], min, max)
}

pub fn achievement_icon(o: &Object, n: *mut c_void) -> i32 {
    let Some(p) = pipe::get(o.pipe) else {
        return 0;
    };
    let (file, achievement, achieved) = {
        let mut s = lock(&p.stats);
        let Some((achievement, achieved, file)) = s.own.as_ref().and_then(|d| {
            let (block, a) = d.achievement(name(n)?)?;
            let achieved = d.achieved(block, a);
            let file = if achieved { &a.icon } else { &a.icon_gray };
            Some((a.name.clone(), achieved, file.clone()))
        }) else {
            return 0;
        };
        if file.is_empty() {
            return 0;
        }
        if let Some(&handle) = s.icons.get(&file) {
            return handle;
        }
        s.icons.insert(file.clone(), 0);
        (file, achievement, achieved)
    };
    let key = file.clone();
    let unsent = file.clone();
    let sent = p.send_then(
        RequestBody::AchievementIcon(AchievementIconRequest { file }),
        Box::new(move |p, body| {
            let image = match body {
                Some(ResponseBody::Image(i))
                    if i.result == OK
                        && i.rgba.len() as u64 == u64::from(i.width) * u64::from(i.height) * 4 =>
                {
                    Some(crate::images::add(crate::images::Image {
                        width: i.width,
                        height: i.height,
                        rgba: i.rgba.clone().into_boxed_slice(),
                    }))
                }
                _ => None,
            };
            {
                let mut s = lock(&p.stats);
                match image {
                    Some(h) => s.icons.insert(key, h),
                    // Asked again on the next call.
                    None => s.icons.remove(&key),
                };
            }
            p.push(
                callbacks::USER_ACHIEVEMENT_ICON_FETCHED,
                callbacks::user_achievement_icon_fetched(
                    game(p),
                    &achievement,
                    achieved,
                    image.unwrap_or(0),
                ),
            );
        }),
    );
    if !sent {
        lock(&p.stats).icons.remove(&unsent);
    }
    0
}

pub fn request_user_stats(o: &Object, user: u64) -> u64 {
    let Some(p) = pipe::get(o.pipe) else {
        return 0;
    };
    p.begin_call(
        RequestBody::UserStats(UserStatsRequest { steam_id: user }),
        callbacks::USER_STATS_RECEIVED,
        received_size(false),
        Box::new(move |p, body| {
            let ResponseBody::UserStats(u) = body else {
                return None;
            };
            if u.result == OK {
                lock(&p.stats).others.insert(user, UserData::new(u.clone()));
            }
            Some(callbacks::user_stats_received(
                game(p),
                u.result as i32,
                user,
            ))
        }),
    )
}

/// Runs `f` on the stats `RequestUserStats` fetched for `user`.
fn with_user<R>(o: &Object, user: u64, default: R, f: impl FnOnce(&UserData) -> R) -> R {
    with(o, None, |_, s| s.others.get(&user).map(f)).unwrap_or(default)
}

pub fn get_user_stat_int(o: &Object, user: u64, n: *mut c_void, out: *mut c_void) -> bool {
    with_user(o, user, false, |d| get_value(d, n, &[INT], out))
}

pub fn get_user_stat_float(o: &Object, user: u64, n: *mut c_void, out: *mut c_void) -> bool {
    with_user(o, user, false, |d| get_value(d, n, &[FLOAT, AVGRATE], out))
}

pub fn get_user_achievement(o: &Object, user: u64, n: *mut c_void, achieved: *mut c_void) -> bool {
    with_user(o, user, false, |d| {
        user_achievement(d, n, achieved, std::ptr::null_mut())
    })
}

pub fn get_user_achievement_and_unlock_time(
    o: &Object,
    user: u64,
    n: *mut c_void,
    achieved: *mut c_void,
    time: *mut c_void,
) -> bool {
    with_user(o, user, false, |d| user_achievement(d, n, achieved, time))
}

pub fn request_global_achievement_percentages(o: &Object) -> u64 {
    let Some(p) = pipe::get(o.pipe) else {
        return 0;
    };
    p.begin_call(
        RequestBody::GlobalAchievements(GlobalAchievementsRequest {}),
        callbacks::GLOBAL_ACHIEVEMENT_PERCENTAGES_READY,
        callbacks::game_result(0, 0).len(),
        Box::new(|p, body| {
            let ResponseBody::GlobalAchievements(g) = body else {
                return None;
            };
            if g.result == OK {
                let mut s = lock(&p.stats);
                // The schema's order, then the most achieved first; the sort
                // is stable.
                let order: HashMap<&str, usize> = s.own.as_ref().map_or_else(HashMap::new, |d| {
                    achievements(&d.schema)
                        .enumerate()
                        .map(|(i, (_, a))| (a.name.as_str(), i))
                        .collect()
                });
                let mut list: Vec<(usize, String, f32)> = g
                    .achievements
                    .iter()
                    .map(|a| {
                        let at = order.get(a.name.as_str()).copied().unwrap_or(usize::MAX);
                        (at, a.name.clone(), f32::from_bits(a.percent))
                    })
                    .collect();
                drop(order);
                list.sort_by_key(|(at, _, _)| *at);
                list.sort_by(|a, b| b.2.total_cmp(&a.2));
                s.global = Some(list.into_iter().map(|(_, n, pct)| (n, pct)).collect());
            }
            Some(callbacks::game_result(game(p), g.result as i32))
        }),
    )
}

/// Achievement `i` in the global order: its name, its percentage, and
/// whether the account has it. The index, or -1.
fn most_achieved(
    o: &Object,
    i: i32,
    buf: *mut c_void,
    size: u32,
    percent: *mut c_void,
    achieved: *mut c_void,
) -> i32 {
    with(o, -1, |_, s| {
        let Some((n, pct)) = usize::try_from(i)
            .ok()
            .and_then(|i| s.global.as_ref()?.get(i))
        else {
            return -1;
        };
        let has = s.own.as_ref().is_some_and(|d| {
            d.achievement(n)
                .is_some_and(|(block, a)| d.achieved(block, a))
        });
        // SAFETY: the SDK declares a buffer of `size` bytes, a `float *` and
        // a `bool *`.
        unsafe {
            write_str_truncated(buf, size.into(), n);
            put(percent, *pct);
            put(achieved, u8::from(has));
        }
        i
    })
}

pub fn most_achieved_achievement_info(
    o: &Object,
    buf: *mut c_void,
    size: u32,
    percent: *mut c_void,
    achieved: *mut c_void,
) -> i32 {
    most_achieved(o, 0, buf, size, percent, achieved)
}

pub fn next_most_achieved_achievement_info(
    o: &Object,
    previous: i32,
    buf: *mut c_void,
    size: u32,
    percent: *mut c_void,
    achieved: *mut c_void,
) -> i32 {
    if previous < 0 {
        return -1;
    }
    most_achieved(o, previous.saturating_add(1), buf, size, percent, achieved)
}

pub fn achievement_achieved_percent(o: &Object, n: *mut c_void, percent: *mut c_void) -> bool {
    with(o, false, |_, s| {
        let Some(n) = name(n) else {
            return false;
        };
        let Some((_, pct)) = s
            .global
            .as_ref()
            .and_then(|g| g.iter().find(|(a, _)| a == n))
        else {
            return false;
        };
        // SAFETY: the SDK declares a `float *`.
        unsafe { put(percent, *pct) };
        true
    })
}
