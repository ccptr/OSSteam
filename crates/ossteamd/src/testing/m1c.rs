// SPDX-License-Identifier: GPL-2.0-or-later
//! M1c against the fake CM and a fake CDN: launching an installed app,
//! the checks before a launch (`launch.md` §2), supervision, `StopGame`,
//! play sessions (PLAY-1) and the discovery entries (`steamworks-abi.md`
//! §1). The reaper is a stand-in here; `tests/reaper.rs` tests the real
//! one.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use ossteam_content::fake::{Depot, Entry};
use ossteam_content::manifest::flags;
use ossteam_ipc::v1::{self, ErrorCode, Topic};
#[cfg(target_os = "linux")]
use ossteam_ipc::v1::{event, game_exited};
use ossteam_ipc::{Client, ClientError};
use ossteam_launch::Platform;
use ossteam_types::{DepotId, ManifestId};

use super::m1b::{done, installed, key, login, section};
use super::*;

const GAME: u32 = 20;
/// Owned, never installed.
const OTHER: u32 = 21;
const GAME_DEPOT: u32 = 201;
const ACCOUNT_ID: u32 = 1001;
const POLL: Duration = Duration::from_millis(200);

/// The game: writes how it was started into its working directory, the
/// install directory (`launch.md` §3), then does what its first extra
/// argument says.
const SCRIPT: &[u8] = br#"#!/bin/sh
pwd > cwd.txt
printf '%s\n' "$@" > args.txt
env > env.txt
echo "game output"
case "$2" in
  exit) exit 3 ;;
  wait) exec sleep 60 ;;
  shim) exec "$3" --exact testing::m1c::shim_in_a_game --ignored --nocapture ;;
  steam) ls -A "$3" > steam.txt; cat "$3/steam.pid" > pid.txt
         ls -A "$4/userdata" > userdata.txt; ls -A "$4/config" > config.txt ;;
esac
"#;

/// The OS name launch options use for this machine, and one that is not
/// this machine's.
fn oses() -> (&'static str, &'static str) {
    let os = Platform::native().expect("an OS with launch options").os;
    (os, if os == "windows" { "linux" } else { "windows" })
}

/// A PE image with a `.bind` section: what SteamStub leaves (`launch.md`
/// §7). Only the headers the check reads are there.
fn steamstub_image() -> Vec<u8> {
    let mut b = vec![0u8; 64];
    b[..2].copy_from_slice(b"MZ");
    b[60..64].copy_from_slice(&64u32.to_le_bytes());
    b.extend_from_slice(b"PE\0\0");
    let mut coff = [0u8; 20];
    coff[2..4].copy_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&coff);
    let mut section = [0u8; 40];
    section[..5].copy_from_slice(b".bind");
    b.extend_from_slice(&section);
    b
}

fn game_depot(manifest: u64, protected: &[u8]) -> Depot {
    Depot::build(
        DepotId(GAME_DEPOT),
        ManifestId(manifest),
        key(GAME_DEPOT),
        &[
            Entry::File {
                name: "bin/game.sh",
                data: SCRIPT,
                flags: flags::EXECUTABLE,
            },
            Entry::File {
                name: "bin/protected.exe",
                data: protected,
                flags: flags::EXECUTABLE,
            },
        ],
    )
}

fn depots_section(manifest: u64) -> ossteam_vdf::Object {
    section(&format!(
        r#""{GAME_DEPOT}" {{ "manifests" {{ "public" {{ "gid" "{manifest}" "size" "1" }} }} }}
           "branches" {{ "public" {{ "buildid" "{manifest}" }} }}"#
    ))
}

/// `config` with these launch options, each `(executable, oslist)`.
fn config(options: &[(&str, &str)]) -> ossteam_vdf::Object {
    let launch: String = options
        .iter()
        .enumerate()
        .map(|(i, (exe, os))| {
            format!(
                r#""{i}" {{ "executable" "{exe}" "arguments" "-steam" "type" "default"
                           "config" {{ "oslist" "{os}" }} }}"#
            )
        })
        .collect();
    section(&format!(r#""installdir" "Game" "launch" {{ {launch} }}"#))
}

fn steam(cdn: &Arc<FakeCdn>, options: &[(&str, &str)]) -> FakeCm {
    let cm = FakeCm::new()
        .package(200, &[GAME, OTHER])
        .app(GAME, "Game", "Game", None)
        .app(OTHER, "Other", "Game", None)
        .cdn(cdn.clone())
        .account(Account::new("alice", ACCOUNT_ID).license(200, 0));
    cm.app_section(GAME, "config", config(options));
    cm.app_section(
        GAME,
        "ufs",
        section(r#""quota" "100000000" "maxnumfiles" "500""#),
    );
    let depot = game_depot(1, &steamstub_image());
    cm.publish(&depot, 1);
    cm.app_section(GAME, "depots", depots_section(1));
    cm
}

/// Installs the game and returns its directory.
fn install(c: &Client) -> PathBuf {
    c.set_setting("content.auto_update", Some("never"))
        .expect("setting");
    done(c, c.install(GAME, "", "").expect("install queued"));
    PathBuf::from(installed(c, GAME).expect("installed").path)
}

fn code<T: std::fmt::Debug>(r: Result<T, ClientError>) -> ErrorCode {
    match r {
        Err(ClientError::Remote(e)) => e.code,
        other => panic!("expected an error from the daemon, got {other:?}"),
    }
}

fn launch(c: &Client, option: Option<u32>, args: &[&str]) -> Result<v1::RunningGame, ClientError> {
    let args: Vec<String> = args.iter().map(|&a| a.to_owned()).collect();
    c.launch(GAME, option, &args, false)
}

#[cfg(target_os = "linux")]
/// The `GameExited` of the game; `c` must be subscribed to GAMES.
fn exited(c: &Client) -> v1::GameExited {
    wait_event(c, "the game to exit", |k| match k {
        event::Kind::GameExited(e) if e.app_id == GAME => Some(*e),
        _ => None,
    })
}

#[cfg(target_os = "linux")]
fn steam_dir(root: &Root) -> PathBuf {
    root.path().join("home").join(".steam")
}

#[cfg(target_os = "linux")]
fn exists(p: &std::path::Path) -> bool {
    p.symlink_metadata().is_ok()
}

#[test]
fn launches_are_refused_as_launch_md_says() {
    let cdn = Arc::new(FakeCdn::new());
    let (os, other_os) = oses();
    let cm = steam(
        &cdn,
        &[
            ("bin/game.sh", os),
            ("bin/protected.exe", os),
            ("bin/missing.sh", os),
            ("../outside.sh", os),
            ("bin/game.exe", other_os),
        ],
    );
    let root = Root::new();
    // Without a private ~/.steam, an official client's entries stop
    // launches (§1.1.4).
    root.refuse_private_steam();
    let d = TestDaemon::start_with_cdn(&root, &cm, POLL, cdn.clone());
    let c = d.client();
    assert_eq!(code(launch(&c, Some(0), &[])), ErrorCode::NotLoggedIn);
    login(&d);
    let c = d.subscribed(&[Topic::Jobs]);
    assert_eq!(code(c.launch(999, None, &[], false)), ErrorCode::NotOwned);
    assert_eq!(code(c.launch(OTHER, None, &[], false)), ErrorCode::NotFound);
    install(&c);

    let r = c.get_launch_options(GAME).expect("launch options");
    let qualifies: Vec<bool> = r.options.iter().map(|o| o.qualifies).collect();
    assert_eq!(qualifies, [true, true, true, true, false]);
    assert!(!r.single);
    assert_eq!(code(c.get_launch_options(999)), ErrorCode::NotFound);

    // Several qualify, so an index is needed; it must exist and qualify.
    assert_eq!(code(launch(&c, None, &[])), ErrorCode::InvalidArgument);
    assert_eq!(code(launch(&c, Some(4), &[])), ErrorCode::InvalidArgument);
    assert_eq!(code(launch(&c, Some(9), &[])), ErrorCode::InvalidArgument);
    // Missing, or outside the install directory.
    assert_eq!(code(launch(&c, Some(2), &[])), ErrorCode::NotFound);
    assert_eq!(code(launch(&c, Some(3), &[])), ErrorCode::NotFound);
    // SteamStub (SPEC §13.2).
    assert_eq!(code(launch(&c, Some(1), &[])), ErrorCode::Unsupported);
    // No client library next to the daemon (and, on macOS, no entries
    // until M1e).
    assert_eq!(code(launch(&c, Some(0), &[])), ErrorCode::Unsupported);

    // An official client's entries are left alone (DISC-3).
    #[cfg(target_os = "linux")]
    {
        root.install_client_libraries();
        let steam = steam_dir(&root);
        let valve = root.path().join("Steam").join("linux64");
        std::fs::create_dir_all(&valve).unwrap();
        std::fs::create_dir_all(&steam).unwrap();
        // GetStatus says why (steamworks-abi.md §1.1.6), before and after
        // the official client's entries appear.
        let isolation = |c: &Client| {
            c.get_status()
                .expect("status")
                .game_isolation
                .expect("game_isolation on Linux")
        };
        let before = isolation(&c);
        assert!(!before.available);
        assert!(before.reason.contains("turned off"), "{}", before.reason);
        assert!(!before.launches_blocked);
        assert!(!before.holds_official_entries);
        std::os::unix::fs::symlink(&valve, steam.join("sdk64")).unwrap();
        assert!(isolation(&c).launches_blocked);
        assert_eq!(code(launch(&c, Some(0), &[])), ErrorCode::Conflict);
        assert_eq!(std::fs::read_link(steam.join("sdk64")).unwrap(), valve);
        assert!(!exists(&steam.join("sdk32")));
        assert!(!exists(&steam.join("steam.pid")));
    }

    // A new build is known: the installed one launches only when asked to.
    let depot = game_depot(2, b"not a PE image any more");
    cm.publish(&depot, 2);
    cm.app_section(GAME, "depots", depots_section(2));
    wait_until("the update to be known", || {
        installed(&c, GAME).is_some_and(|i| i.update_available)
    });
    assert_eq!(code(launch(&c, Some(0), &[])), ErrorCode::Conflict);
    assert_eq!(
        code(c.launch(GAME, Some(1), &[], true)),
        ErrorCode::Unsupported,
        "allow_stale passes the update check and stops at SteamStub"
    );

    // No option for this OS.
    cm.app_section(GAME, "config", config(&[("bin/game.exe", other_os)]));
    wait_until("the new launch options", || {
        c.get_launch_options(GAME)
            .is_ok_and(|r| r.options.len() == 1)
    });
    assert_eq!(
        code(c.launch(GAME, None, &[], true)),
        ErrorCode::Unsupported
    );
}

#[cfg(target_os = "linux")]
#[test]
fn discovery_entries_follow_the_account() {
    use ossteam_launch::discovery::{self, Entries};

    let cdn = Arc::new(FakeCdn::new());
    let (os, _) = oses();
    let cm = steam(&cdn, &[("bin/game.sh", os)]);
    let root = Root::new();
    root.install_client_libraries();
    let steam = steam_dir(&root);
    let pid_file = steam.join("steam.pid");

    // What a crashed daemon leaves is removed at start (DISC-2).
    let libs = discovery::find_libraries(&root.library_dir()).unwrap();
    let entries = Entries::new(&root.path().join("home"), &root.path().join("data"));
    entries.create(&libs, 4_000_000, 0).unwrap();
    assert!(exists(&pid_file));
    let d = TestDaemon::start_with_cdn(&root, &cm, POLL, cdn.clone());
    let c = d.client();
    c.get_status().expect("status");
    assert!(!exists(&pid_file));
    assert!(!exists(&steam.join("sdk64")));

    // Created with an active account (DISC-1).
    login(&d);
    wait_until("the entries", || exists(&pid_file));
    assert_eq!(
        std::fs::read_to_string(&pid_file).unwrap(),
        std::process::id().to_string()
    );
    assert_eq!(
        std::fs::read(steam.join("sdk64/steamclient.so")).unwrap(),
        b"64"
    );
    assert_eq!(
        std::fs::read(steam.join("sdk32/steamclient.so")).unwrap(),
        b"32"
    );

    // Removed at logout, and when the daemon stops.
    c.logout().expect("logout");
    // The pid file goes first, so wait for all of them.
    wait_until("the entries to go", || {
        !exists(&pid_file) && !exists(&steam.join("sdk64")) && !exists(&steam.join("sdk32"))
    });
    login(&d);
    wait_until("the entries", || exists(&pid_file));
    d.stop();
    assert!(!exists(&pid_file));
    assert!(!exists(&steam.join("sdk64")));
}

/// With `discovery.take_over_official` on, the daemon takes over an
/// official client's entries while it is not running, hands them back as
/// they were, yields them when the official client makes its own again,
/// and hands back at start what a crashed daemon held (DISC-5,
/// `steamworks-abi.md` §1.1.5).
#[cfg(target_os = "linux")]
#[test]
fn official_entries_are_taken_over_and_handed_back() {
    use ossteam_launch::discovery::takeover::Record;

    let cdn = Arc::new(FakeCdn::new());
    let (os, _) = oses();
    let cm = steam(&cdn, &[("bin/game.sh", os)]);
    let root = Root::new();
    root.install_client_libraries();
    root.refuse_private_steam();
    let steam = steam_dir(&root);
    let valve = root.path().join("home/.local/share/Steam/linux64");
    std::fs::create_dir_all(&valve).unwrap();
    std::fs::create_dir_all(&steam).unwrap();
    std::os::unix::fs::symlink(&valve, steam.join("sdk64")).unwrap();
    // Not a live process with steam.pipe open: the client is not running.
    std::fs::write(steam.join("steam.pid"), "99").unwrap();
    let record = root.path().join("config/official-entries.toml");
    let ours = root.path().join("data/steam-sdk/sdk64");
    let official_as_left = || {
        assert_eq!(std::fs::read_link(steam.join("sdk64")).unwrap(), valve);
        assert_eq!(
            std::fs::read_to_string(steam.join("steam.pid")).unwrap(),
            "99"
        );
        assert!(!exists(&steam.join("sdk32")));
        assert!(!exists(&record));
    };

    let d = TestDaemon::start_with_cdn(&root, &cm, POLL, cdn.clone());
    login(&d);
    let c = d.subscribed(&[Topic::Notices]);
    let isolation = |c: &Client| c.get_status().unwrap().game_isolation.unwrap();
    wait_until("the private entries", || {
        exists(&root.path().join("data/steam-sdk/steam.pid"))
    });
    assert!(isolation(&c).launches_blocked);
    assert!(!isolation(&c).holds_official_entries);

    let warning = |c: &Client, what: &str, part: &'static str| {
        wait_event(c, what, |k| match k {
            event::Kind::Notice(n)
                if n.level == v1::NoticeLevel::Warning as i32 && n.text.contains(part) =>
            {
                Some(n.text.clone())
            }
            _ => None,
        })
    };
    c.set_setting("discovery.take_over_official", Some("true"))
        .expect("set");
    warning(
        &c,
        "the setting's warning",
        "discovery.take_over_official is on",
    );
    wait_until("the take-over", || isolation(&c).holds_official_entries);
    assert!(!isolation(&c).launches_blocked);
    assert_eq!(std::fs::read_link(steam.join("sdk64")).unwrap(), ours);
    assert_eq!(
        std::fs::read_to_string(steam.join("steam.pid")).unwrap(),
        std::process::id().to_string()
    );
    assert_eq!(
        Record::read(&record).unwrap(),
        Some(Record {
            sdk64: Some(valve.clone()),
            sdk32: None,
            pid: Some("99".into()),
        })
    );

    // Turned off: handed back as they were.
    c.set_setting("discovery.take_over_official", Some("false"))
        .expect("set");
    official_as_left();
    assert!(!isolation(&c).holds_official_entries);

    // On again, then the official client starts: steam.sh makes sdk64
    // again, and the daemon yields.
    c.set_setting("discovery.take_over_official", None)
        .expect("reset");
    c.set_setting("discovery.take_over_official", Some("true"))
        .expect("set");
    wait_until("the take-over", || isolation(&c).holds_official_entries);
    let tmp = steam.join("sdk64.tmp");
    std::os::unix::fs::symlink(&valve, &tmp).unwrap();
    std::fs::rename(&tmp, steam.join("sdk64")).unwrap();
    warning(&c, "the yield", "took ~/.steam back");
    assert!(!isolation(&c).holds_official_entries);
    assert_eq!(std::fs::read_link(steam.join("sdk64")).unwrap(), valve);
    c.set_setting("discovery.take_over_official", Some("false"))
        .expect("set");
    drop(c);
    drop(d);

    // A crashed daemon's take-over is handed back at start.
    std::fs::write(steam.join("steam.pid"), "99").unwrap();
    let libs = ossteam_launch::discovery::find_libraries(&root.library_dir()).unwrap();
    let entries = ossteam_launch::discovery::Entries::new(
        &root.path().join("home"),
        &root.path().join("data"),
    );
    let _ = entries.create(&libs, 4_000_000, 0);
    entries
        .take_over(&record, 4_000_000, &mut || Ok(()))
        .unwrap();
    assert_eq!(std::fs::read_link(steam.join("sdk64")).unwrap(), ours);
    let d = TestDaemon::start_with_cdn(&root, &cm, POLL, cdn.clone());
    d.client().get_status().expect("status");
    official_as_left();
}

/// The take-over follows the official client's close at once: not after
/// the 30 s of quiet that its own writes to `~/.steam` would otherwise
/// start, nor on the next look every 10 s (DISC-5, `steamworks-abi.md`
/// §1.1.5).
#[cfg(target_os = "linux")]
#[test]
fn official_entries_are_taken_over_as_the_client_closes() {
    let cdn = Arc::new(FakeCdn::new());
    let (os, _) = oses();
    let cm = steam(&cdn, &[("bin/game.sh", os)]);
    let root = Root::new();
    root.install_client_libraries();
    root.refuse_private_steam();
    root.slow_take_over();
    let steam = steam_dir(&root);
    let valve = root.path().join("home/.local/share/Steam/linux64");
    std::fs::create_dir_all(&valve).unwrap();
    std::fs::create_dir_all(&steam).unwrap();
    std::os::unix::fs::symlink(&valve, steam.join("sdk64")).unwrap();
    // The official client runs: steam.pid names a process with
    // steam.pipe open.
    std::fs::write(steam.join("steam.pipe"), b"").unwrap();
    let pipe = std::fs::File::open(steam.join("steam.pipe")).unwrap();
    let mut client = std::process::Command::new("sleep")
        .arg("60")
        .stdin(pipe)
        .spawn()
        .expect("start a stand-in client");
    std::fs::write(steam.join("steam.pid"), client.id().to_string()).unwrap();

    let d = TestDaemon::start_with_cdn(&root, &cm, POLL, cdn.clone());
    login(&d);
    let c = d.subscribed(&[Topic::Notices]);
    let isolation = |c: &Client| c.get_status().unwrap().game_isolation.unwrap();
    wait_until("the private entries", || {
        exists(&root.path().join("data/steam-sdk/steam.pid"))
    });
    c.set_setting("discovery.take_over_official", Some("true"))
        .expect("set");
    std::thread::sleep(Duration::from_millis(300));
    assert!(!isolation(&c).holds_official_entries, "the client runs");

    // It writes as it closes, then exits.
    std::fs::write(steam.join("registry.vdf"), b"closing").unwrap();
    std::thread::sleep(Duration::from_millis(100));
    let closed = Instant::now();
    client.kill().unwrap();
    client.wait().unwrap();
    wait_until("the take-over", || isolation(&c).holds_official_entries);
    assert!(
        closed.elapsed() < Duration::from_secs(5),
        "{:?}",
        closed.elapsed()
    );
    assert_eq!(
        std::fs::read_link(steam.join("sdk64")).unwrap(),
        root.path().join("data/steam-sdk/sdk64")
    );
    c.set_setting("discovery.take_over_official", Some("false"))
        .expect("set");
    assert_eq!(std::fs::read_link(steam.join("sdk64")).unwrap(), valve);
}

/// A launched game finds ossteam in its private `~/.steam` while an
/// official client owns the real one, and sees ossteam's `userdata/` and
/// an empty `config/` in the official installation (DISC-4,
/// `steamworks-abi.md` §1.1.2).
#[cfg(target_os = "linux")]
#[test]
fn games_get_a_private_steam() {
    let root = Root::new();
    if let Err(e) = ossteam_launch::isolation::check(&root.path().join("check")) {
        eprintln!("skipped: no user namespace here: {e}");
        return;
    }
    let cdn = Arc::new(FakeCdn::new());
    let (os, _) = oses();
    let cm = steam(&cdn, &[("bin/game.sh", os)]);
    root.install_client_libraries();
    // An official client that has run: its links, its pid, and its
    // installation with an account's data.
    let steam = steam_dir(&root);
    let official = root.path().join("home/.local/share/Steam");
    std::fs::create_dir_all(official.join("linux64")).unwrap();
    std::fs::create_dir_all(official.join("userdata/1234/config")).unwrap();
    std::fs::create_dir_all(official.join("config")).unwrap();
    std::fs::write(official.join("config/loginusers.vdf"), b"accounts").unwrap();
    std::fs::create_dir_all(&steam).unwrap();
    std::os::unix::fs::symlink(official.join("linux64"), steam.join("sdk64")).unwrap();
    std::os::unix::fs::symlink(&official, steam.join("root")).unwrap();
    std::fs::write(steam.join("steam.pid"), "99").unwrap();

    let d = TestDaemon::start_with_cdn(&root, &cm, POLL, cdn.clone());
    login(&d);
    let c = d.subscribed(&[Topic::Jobs, Topic::Games]);
    let status = c.get_status().expect("status");
    assert_eq!(
        status.game_isolation,
        Some(v1::GameIsolation {
            available: true,
            ..Default::default()
        })
    );
    let dir = install(&c);
    let steam_arg = steam.to_str().unwrap();
    launch(&c, None, &["steam", steam_arg, official.to_str().unwrap()]).expect("launched");
    exited(&c);
    let read = |n: &str| std::fs::read_to_string(dir.join(n)).unwrap();
    assert_eq!(read("steam.txt"), "sdk32\nsdk64\nsteam.pid\n");
    assert_eq!(read("pid.txt"), std::process::id().to_string());
    assert!(!read("userdata.txt").contains("1234"));
    assert_eq!(read("config.txt"), "");

    // The real ~/.steam and the installation are as the official client
    // left them.
    assert_eq!(
        std::fs::read_link(steam.join("sdk64")).unwrap(),
        official.join("linux64")
    );
    assert_eq!(
        std::fs::read_to_string(steam.join("steam.pid")).unwrap(),
        "99"
    );
    assert!(!exists(&steam.join("sdk32")));
    assert!(official.join("userdata/1234/config").is_dir());
    assert!(official.join("config/loginusers.vdf").is_file());
    d.stop();
}

#[cfg(target_os = "linux")]
#[test]
fn launch_supervise_stop_and_play_session() {
    let cdn = Arc::new(FakeCdn::new());
    let (os, other_os) = oses();
    let cm = steam(&cdn, &[("bin/game.sh", os), ("bin/game.exe", other_os)]);
    let root = Root::new();
    root.install_client_libraries();
    let d = TestDaemon::start_with_cdn(&root, &cm, POLL, cdn.clone());
    login(&d);
    let c = d.subscribed(&[Topic::Jobs, Topic::Games]);
    let dir = install(&c);
    let r = c.get_launch_options(GAME).expect("launch options");
    assert!(r.single);
    assert_eq!(r.options[0].arguments, "-steam");

    // A game that exits by itself. The option is the only one that
    // qualifies, so no index is needed.
    let game = launch(&c, None, &["exit", "it's"]).expect("launched");
    assert_eq!(game.app_id, GAME);
    assert_eq!(game.option, 0);
    let started = wait_event(&c, "the game to start", |k| match k {
        event::Kind::GameStarted(g) => g.game.clone(),
        _ => None,
    });
    assert_eq!(started, game);
    let e = exited(&c);
    assert_eq!(e.status, Some(game_exited::Status::ExitCode(3)));
    let read = |n: &str| std::fs::read_to_string(dir.join(n)).unwrap_or_default();
    assert_eq!(read("args.txt"), "-steam\nexit\nit's\n");
    assert_eq!(read("cwd.txt").trim(), dir.to_str().unwrap());
    let env = read("env.txt");
    for var in [
        "SteamAppId=20",
        "SteamGameId=20",
        "SteamClientLaunch=1",
        "SteamUser=alice",
    ] {
        assert!(
            env.lines().any(|l| l == var),
            "{var} is missing from\n{env}"
        );
    }
    assert_eq!(std::fs::read_to_string(&game.log).unwrap(), "game output\n");
    assert!(c.list_running().unwrap().is_empty());

    // The play session: the game while it ran, then none (PLAY-1).
    cm.wait(WAIT, "the play session to end", |v| {
        v.games_played().len() >= 2
    });
    cm.wait(WAIT, "the play session", |v| {
        let sessions = v.games_played();
        let [.., (_, first), (_, last)] = sessions else {
            return false;
        };
        let [g] = first.games_played.as_slice() else {
            return false;
        };
        g.game_id == Some(u64::from(GAME))
            && g.process_id == Some(game.pid)
            && g.owner_id == Some(ACCOUNT_ID)
            && g.game_build_id == Some(1)
            && g.beta_name.is_none()
            && last.games_played.is_empty()
    });

    // A game that runs until it is stopped.
    let game = launch(&c, None, &["wait"]).expect("launched");
    let running = c.list_running().unwrap();
    assert_eq!(running, std::slice::from_ref(&game));
    assert_eq!(code(launch(&c, None, &["wait"])), ErrorCode::Conflict);
    assert_eq!(code(c.logout()), ErrorCode::Conflict);
    assert_eq!(code(c.shutdown(false)), ErrorCode::Conflict);
    assert_eq!(c.get_status().unwrap().running_games, 1);
    // Sent again after a new logon, since the server forgot it.
    let sent = std::cell::Cell::new(0);
    cm.wait(WAIT, "the play session", |v| {
        sent.set(v.games_played().len());
        v.games_played()
            .last()
            .is_some_and(|(_, p)| p.games_played.len() == 1)
    });
    cm.drop_connections();
    cm.wait(WAIT, "the play session after the new logon", |v| {
        v.games_played().len() > sent.get()
            && v.games_played()
                .last()
                .is_some_and(|(_, p)| p.games_played.len() == 1)
    });

    c.stop_game(GAME).expect("stop");
    let e = exited(&c);
    assert!(
        matches!(
            e.status,
            Some(game_exited::Status::ExitCode(143) | game_exited::Status::Signal(15))
        ),
        "{e:?}"
    );
    assert!(c.list_running().unwrap().is_empty());
    assert_eq!(code(c.stop_game(GAME)), ErrorCode::NotFound);
    d.stop();
}

/// Connects to the test daemon's game endpoint as the shim does.
#[cfg_attr(not(unix), allow(dead_code))]
pub(super) fn shim_connect(
    addr: &ossteam_gameipc::endpoint::Address,
    hello: ossteam_gameipc::Hello,
) -> (
    ossteam_gameipc::endpoint::Connection,
    ossteam_gameipc::Message,
) {
    let mut conn = ossteam_gameipc::endpoint::connect(addr).expect("connect to the game endpoint");
    ossteam_gameipc::write_message(&mut conn.writer, &ossteam_gameipc::Message::Hello(hello))
        .expect("hello");
    let answer = ossteam_gameipc::read_message(&mut conn.reader).expect("an answer");
    (conn, answer)
}

#[cfg_attr(not(unix), allow(dead_code))]
pub(super) fn hello(app_id: u32) -> ossteam_gameipc::Hello {
    ossteam_gameipc::Hello {
        protocol: ossteam_gameipc::PROTOCOL,
        pid: std::process::id(),
        app_id,
        arch: "x86_64".into(),
        version: "test".into(),
    }
}

// Used by the endpoint tests, which run on Unix only.
#[cfg_attr(not(unix), allow(dead_code))]
fn shim_report(c: &Client, pid: u32) -> v1::ShimReport {
    wait_event(c, "a shim report", |k| match k {
        ossteam_ipc::v1::event::Kind::ShimReport(r) if r.pid == pid => Some(r.clone()),
        _ => None,
    })
}

/// A process that is not launched (`steamworks-abi.md` §3): the
/// handshake, what it is told, `AppState`, updates and its report.
#[cfg(unix)]
#[test]
fn games_talk_to_the_game_endpoint() {
    use ossteam_gameipc::{
        AppStateRequest, Call, Message, Report, Request, RequestBody, ResponseBody,
    };

    let cdn = Arc::new(FakeCdn::new());
    let (os, _) = oses();
    let cm = steam(&cdn, &[("bin/game.sh", os)]);
    let root = Root::new();
    let addr = root.game_address();
    let d = TestDaemon::start_with_cdn(&root, &cm, POLL, cdn.clone());

    // The endpoint is there before any login, and refuses what does not
    // fit.
    let (_, answer) = shim_connect(
        &addr,
        ossteam_gameipc::Hello {
            protocol: 99,
            ..hello(GAME)
        },
    );
    assert!(matches!(answer, Message::Refused(r) if r.reason.contains("99")));
    let (_, answer) = shim_connect(
        &addr,
        ossteam_gameipc::Hello {
            pid: 1,
            ..hello(GAME)
        },
    );
    assert!(matches!(answer, Message::Refused(r) if r.reason.contains("pid")));
    let (_, answer) = shim_connect(&addr, hello(GAME));
    let Message::Welcome(w) = answer else {
        panic!("{answer:?}")
    };
    assert!(!w.session.active);
    assert!(!w.app.owned);
    assert!(w.app.user_dir.is_empty());

    login(&d);
    let c = d.subscribed(&[Topic::Jobs, Topic::Games]);
    let dir = install(&c);
    let (mut conn, answer) = shim_connect(&addr, hello(GAME));
    let Message::Welcome(w) = answer else {
        panic!("{answer:?}")
    };
    let s = &w.session;
    assert!(s.active && s.logged_on);
    assert_eq!(ossteam_types::SteamId(s.steam_id).account_id(), ACCOUNT_ID);
    assert_eq!(s.ip_country, super::fake_cm::FAKE_COUNTRY);
    assert!(s.server_time >= super::fake_cm::FAKE_SERVER_TIME);
    assert_eq!(s.universe, 1);
    assert_eq!(s.persona_state, 1);
    let a = &w.app;
    assert_eq!(a.app_id, GAME);
    assert!(a.owned && a.installed && !a.launched);
    assert_eq!(PathBuf::from(&a.install_dir), dir);
    assert_eq!(a.build_id, 1);
    assert_eq!(a.depots, [GAME_DEPOT]);
    assert!(a.branch.is_empty());
    assert_eq!(a.owner, s.steam_id);
    assert_eq!((a.cloud_quota, a.cloud_max_files), (100_000_000, 500));
    assert_eq!(
        PathBuf::from(&a.user_dir),
        root.path()
            .join("data/userdata")
            .join(ACCOUNT_ID.to_string())
            .join(GAME.to_string())
    );

    // Another app, and one the account does not own.
    let ask = |conn: &mut ossteam_gameipc::endpoint::Connection, id: u32, app_id: u32| {
        let req = Message::Request(Request {
            id,
            body: RequestBody::AppState(AppStateRequest { app_id }),
        });
        ossteam_gameipc::write_message(&mut conn.writer, &req).unwrap();
        loop {
            match ossteam_gameipc::read_message(&mut conn.reader).unwrap() {
                Message::Response(r) if r.id == id => break r.body,
                _ => {}
            }
        }
    };
    let ResponseBody::AppState(other) = ask(&mut conn, 1, OTHER) else {
        panic!()
    };
    assert!(other.owned && !other.installed && other.install_dir.is_empty());
    let ResponseBody::AppState(none) = ask(&mut conn, 2, 999) else {
        panic!()
    };
    assert!(!none.owned && !none.installed);

    // The connection to Steam drops: the game hears of it.
    cm.drop_connections();
    loop {
        match ossteam_gameipc::read_message(&mut conn.reader).unwrap() {
            Message::SessionUpdate(s) if !s.logged_on => break,
            _ => {}
        }
    }

    // Its report arrives when it disconnects.
    let report = Report {
        interfaces: vec!["SteamClient020".into(), "SteamUser021".into()],
        unknown: vec!["SteamFoo001".into()],
        calls: vec![Call {
            interface: "SteamUser021".into(),
            method: "GetVoice".into(),
            count: 3,
        }],
    };
    ossteam_gameipc::write_message(&mut conn.writer, &Message::Report(report)).unwrap();
    let more = Report {
        interfaces: vec!["SteamUser021".into()],
        unknown: vec![],
        calls: vec![
            Call {
                interface: "SteamUser021".into(),
                method: "GetVoice".into(),
                count: 1,
            },
            Call {
                interface: "exports".into(),
                method: "Breakpad_SteamSetAppID".into(),
                count: 9,
            },
        ],
    };
    ossteam_gameipc::write_message(&mut conn.writer, &Message::Report(more)).unwrap();
    (conn.closer)();
    drop(conn);
    let r = shim_report(&c, std::process::id());
    assert_eq!(r.app_id, GAME);
    assert!(!r.launched);
    assert_eq!(r.interfaces, ["SteamClient020", "SteamUser021"]);
    assert_eq!(r.unknown_versions, ["SteamFoo001"]);
    let calls: Vec<(&str, &str, u64)> = r
        .unimplemented
        .iter()
        .map(|c| (c.interface.as_str(), c.method.as_str(), c.count))
        .collect();
    assert_eq!(
        calls,
        [
            ("exports", "Breakpad_SteamSetAppID", 9),
            ("SteamUser021", "GetVoice", 4)
        ]
    );
    // GetApp keeps it as the app's last report (gui.md §5).
    assert_eq!(c.get_app(GAME).unwrap().last_shim_report, Some(r));
    d.stop();
}

/// Run by the game script, not on its own: a process of the launched
/// game that connects as the shim does, reports and exits.
#[cfg(target_os = "linux")]
#[test]
#[ignore = "run by launch_reports_what_the_game_used"]
fn shim_in_a_game() {
    use ossteam_gameipc::{Call, Message, Report};

    let Ok(app) = std::env::var("SteamAppId") else {
        return;
    };
    let addr = std::fs::read_to_string("shim-address.txt").expect("the address");
    let addr = ossteam_gameipc::endpoint::Address::in_dir(addr.trim());
    let (mut conn, answer) = shim_connect(&addr, hello(app.parse().unwrap()));
    let Message::Welcome(w) = answer else {
        panic!("{answer:?}")
    };
    std::fs::write("launched.txt", w.app.launched.to_string()).unwrap();
    let report = Report {
        interfaces: vec!["SteamClient020".into()],
        unknown: vec![],
        calls: vec![Call {
            interface: "SteamUser021".into(),
            method: "GetPlayerSteamLevel".into(),
            count: 2,
        }],
    };
    ossteam_gameipc::write_message(&mut conn.writer, &Message::Report(report)).unwrap();
}

/// A launched game's processes are recognised, and their report comes
/// just before `GameExited` (`steamworks-abi.md` §3.5, §4).
#[cfg(target_os = "linux")]
#[test]
fn launch_reports_what_the_game_used() {
    let cdn = Arc::new(FakeCdn::new());
    let (os, _) = oses();
    let cm = steam(&cdn, &[("bin/game.sh", os)]);
    let root = Root::new();
    root.install_client_libraries();
    let d = TestDaemon::start_with_cdn(&root, &cm, POLL, cdn.clone());
    login(&d);
    let c = d.subscribed(&[Topic::Jobs, Topic::Games]);
    let dir = install(&c);
    std::fs::write(
        dir.join("shim-address.txt"),
        root.socket_dir().to_str().unwrap(),
    )
    .unwrap();
    let exe = std::env::current_exe().unwrap();
    let game = launch(&c, None, &["shim", exe.to_str().unwrap()]).expect("launched");
    let mut report = None;
    let e = wait_event(&c, "the game to exit", |k| match k {
        event::Kind::ShimReport(r) => {
            report = Some(r.clone());
            None
        }
        event::Kind::GameExited(e) if e.app_id == GAME => Some(*e),
        _ => None,
    });
    let log = std::fs::read_to_string(&game.log).unwrap_or_default();
    assert_eq!(e.status, Some(game_exited::Status::ExitCode(0)), "{log}");
    assert_eq!(
        std::fs::read_to_string(dir.join("launched.txt")).unwrap(),
        "true",
        "{log}"
    );
    let r = report.expect("a shim report before GameExited");
    assert!(r.launched);
    assert_eq!((r.app_id, r.pid), (GAME, game.pid));
    assert_eq!(r.interfaces, ["SteamClient020"]);
    assert_eq!(r.unimplemented.len(), 1);
    assert_eq!(r.unimplemented[0].method, "GetPlayerSteamLevel");
    assert_eq!(r.unimplemented[0].count, 2);
    d.stop();
}

/// Stats through the game endpoint (`isteamuserstats.md` §1, §2): the
/// schema, a store, a stale crc, global percentages and an icon; and
/// what a game that does not own its app gets.
#[cfg(unix)]
#[test]
fn games_keep_their_stats() {
    use ossteam_gameipc::{
        AchievementIconRequest, GlobalAchievementsRequest, Message, Request, RequestBody,
        ResponseBody, StatValue, StoreStatsRequest, UserStatsRequest,
    };

    use super::fake_cm::FakeStats;

    let cdn = Arc::new(FakeCdn::new());
    let (os, _) = oses();
    let cm = steam(&cdn, &[("bin/game.sh", os)])
        .stats_schema(GAME, crate::stats::tests::spacewar_schema());
    let me = ossteam_types::SteamId::individual(ACCOUNT_ID);
    let friend = ossteam_types::SteamId::individual(ACCOUNT_ID + 1);
    cm.set_stats(
        me,
        GAME,
        FakeStats {
            crc: 40,
            values: [(1, 0b10), (2, 7)].into(),
            unlocks: [((1, 1), 1_700_000_000)].into(),
        },
    );
    cm.set_stats(
        friend,
        GAME,
        FakeStats {
            crc: 3,
            values: [(2, 99)].into(),
            ..Default::default()
        },
    );
    cm.achievement_percentages(
        GAME,
        &[("ACH_WIN_ONE_GAME", "61.2"), ("ACH_TRAVEL_FAR_ACCUM", "x")],
    );
    let root = Root::new();
    let addr = root.game_address();
    let d = TestDaemon::start_with_cdn(&root, &cm, POLL, cdn.clone());

    // Before a login there is no account, so nothing is owned.
    let ask = |conn: &mut ossteam_gameipc::endpoint::Connection, id: u32, body: RequestBody| {
        let req = Message::Request(Request { id, body });
        ossteam_gameipc::write_message(&mut conn.writer, &req).unwrap();
        loop {
            match ossteam_gameipc::read_message(&mut conn.reader).unwrap() {
                Message::Response(r) if r.id == id => break r.body,
                _ => {}
            }
        }
    };
    let own = || RequestBody::UserStats(UserStatsRequest { steam_id: 0 });
    let (mut conn, _) = shim_connect(&addr, hello(GAME));
    assert!(matches!(ask(&mut conn, 1, own()), ResponseBody::Error(_)));

    login(&d);
    let (mut conn, _) = shim_connect(&addr, hello(GAME));
    let ResponseBody::UserStats(s) = ask(&mut conn, 1, own()) else {
        panic!()
    };
    assert_eq!(s.result, 1);
    assert_eq!(s.steam_id, me.0);
    assert_eq!(s.crc, 40);
    assert_eq!(s.schema.version, 20);
    assert_eq!(s.schema.stats.len(), 4);
    assert_eq!(s.schema.stats[0].achievements[0].display_name, "Winner");
    assert_eq!(
        s.values,
        [
            StatValue { id: 1, value: 0b10 },
            StatValue { id: 2, value: 7 }
        ]
    );
    assert_eq!(s.unlocks.len(), 1);
    assert_eq!(
        (s.unlocks[0].stat, s.unlocks[0].bit, s.unlocks[0].time),
        (1, 1, 1_700_000_000)
    );
    let ResponseBody::UserStats(f) = ask(
        &mut conn,
        2,
        RequestBody::UserStats(UserStatsRequest { steam_id: friend.0 }),
    ) else {
        panic!()
    };
    assert_eq!((f.result, f.steam_id, f.values.len()), (1, friend.0, 1));

    // A store, then one with the stale crc.
    let store = |crc| {
        RequestBody::StoreStats(StoreStatsRequest {
            crc,
            values: vec![
                StatValue { id: 1, value: 0b11 },
                StatValue { id: 2, value: 8 },
            ],
            reset: false,
        })
    };
    let ResponseBody::StoreStats(r) = ask(&mut conn, 3, store(40)) else {
        panic!()
    };
    assert_eq!((r.result, r.crc, r.out_of_date), (1, 41, false));
    let kept = cm.stats(me, GAME);
    assert_eq!(kept.values[&2], 8);
    assert_eq!(kept.unlocks[&(1, 0)], super::fake_cm::FAKE_SERVER_TIME);
    let sent = cm.stores();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].game_id(), u64::from(GAME));
    assert!(!sent[0].explicit_reset());
    let ResponseBody::StoreStats(r) = ask(&mut conn, 4, store(40)) else {
        panic!()
    };
    assert!(r.out_of_date);
    assert_ne!(r.result, 1);

    // Global percentages: one does not parse and is left out.
    let ResponseBody::GlobalAchievements(g) = ask(
        &mut conn,
        5,
        RequestBody::GlobalAchievements(GlobalAchievementsRequest {}),
    ) else {
        panic!()
    };
    assert_eq!(g.result, 1);
    assert_eq!(g.achievements.len(), 1);
    assert_eq!(g.achievements[0].name, "ACH_WIN_ONE_GAME");
    assert_eq!(f32::from_bits(g.achievements[0].percent), 61.2);

    // An icon from the cache, and a name that is not a file name.
    let icons = root
        .path()
        .join("cache/achievement-icons")
        .join(GAME.to_string());
    std::fs::create_dir_all(&icons).unwrap();
    std::fs::write(
        icons.join("win.jpg"),
        include_bytes!("../../testdata/icon.jpg"),
    )
    .unwrap();
    let icon =
        |file: &str| RequestBody::AchievementIcon(AchievementIconRequest { file: file.into() });
    let ResponseBody::Image(i) = ask(&mut conn, 6, icon("win.jpg")) else {
        panic!()
    };
    assert_eq!((i.result, i.width, i.height, i.rgba.len()), (1, 4, 2, 32));
    let ResponseBody::Image(i) = ask(&mut conn, 7, icon("../win.jpg")) else {
        panic!()
    };
    assert_eq!(i.result, 2);

    // An app the account does not own gets nothing.
    let (mut other, _) = shim_connect(&addr, hello(999));
    assert!(
        matches!(ask(&mut other, 1, own()), ResponseBody::Error(e) if e.message.contains("999"))
    );
    // An owned app without stats: Steam's refusal is passed on.
    let (mut no_stats, _) = shim_connect(&addr, hello(OTHER));
    let ResponseBody::UserStats(s) = ask(&mut no_stats, 1, own()) else {
        panic!()
    };
    assert_eq!(s.result, 2);
    d.stop();
}

/// A game connection whose messages are read on a thread, so a test
/// waits with a timeout.
#[cfg(unix)]
struct GameConn {
    writer: Box<dyn std::io::Write + Send>,
    closer: Box<dyn Fn() + Send + Sync>,
    rx: crossbeam_channel::Receiver<ossteam_gameipc::Message>,
    notices: Vec<ossteam_gameipc::NoticeBody>,
    next: u32,
}

#[cfg(unix)]
impl GameConn {
    fn open(addr: &ossteam_gameipc::endpoint::Address, app: u32) -> Self {
        let (conn, answer) = shim_connect(addr, hello(app));
        assert!(
            matches!(answer, ossteam_gameipc::Message::Welcome(_)),
            "{answer:?}"
        );
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut reader = conn.reader;
        std::thread::spawn(move || {
            while let Ok(m) = ossteam_gameipc::read_message(&mut reader) {
                if tx.send(m).is_err() {
                    break;
                }
            }
        });
        Self {
            writer: conn.writer,
            closer: conn.closer,
            rx,
            notices: Vec::new(),
            next: 1,
        }
    }

    fn recv(&mut self) -> ossteam_gameipc::Message {
        self.rx
            .recv_timeout(Duration::from_secs(10))
            .expect("a message from the daemon")
    }

    fn ask(&mut self, body: ossteam_gameipc::RequestBody) -> ossteam_gameipc::ResponseBody {
        use ossteam_gameipc::{Message, Request};
        let id = self.next;
        self.next += 1;
        ossteam_gameipc::write_message(&mut self.writer, &Message::Request(Request { id, body }))
            .unwrap();
        loop {
            match self.recv() {
                Message::Response(r) if r.id == id => return r.body,
                Message::Notice(n) => self.notices.push(n),
                _ => {}
            }
        }
    }

    fn notice(&mut self) -> ossteam_gameipc::NoticeBody {
        if !self.notices.is_empty() {
            return self.notices.remove(0);
        }
        loop {
            if let ossteam_gameipc::Message::Notice(n) = self.recv() {
                return n;
            }
        }
    }

    fn close(self) {
        (self.closer)();
    }
}

/// A plain HTTP server on the loopback for `isteamhttp.md`: one request
/// per connection. It keeps each request's head and answers by path.
#[cfg(unix)]
fn http_server() -> (u16, Arc<std::sync::Mutex<Vec<String>>>) {
    use std::io::{BufRead as _, BufReader, Read as _, Write as _};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let kept = Arc::clone(&seen);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let kept = Arc::clone(&kept);
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut head = String::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        return;
                    }
                    if line == "\r\n" {
                        break;
                    }
                    head.push_str(&line);
                }
                let header = |name: &str| {
                    head.lines().find_map(|l| {
                        let (n, v) = l.split_once(':')?;
                        n.eq_ignore_ascii_case(name).then(|| v.trim().to_owned())
                    })
                };
                let length = header("content-length")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0);
                let mut body = vec![0u8; length];
                reader.read_exact(&mut body).unwrap();
                let path = head.split(' ').nth(1).unwrap_or("").to_owned();
                // How many GETs of this path came, this one included.
                let gets = {
                    let mut kept = kept.lock().unwrap();
                    kept.push(head.clone());
                    let get = format!("GET {path} ");
                    kept.iter().filter(|h| h.starts_with(&get)).count()
                };
                let respond = |stream: &mut std::net::TcpStream,
                               status: &str,
                               extra: &str,
                               body: &[u8]| {
                    let _ = write!(
                        stream,
                        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n{extra}\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(body);
                };
                match path.as_str() {
                    _ if head.starts_with("PATCH ") => {
                        respond(&mut stream, "405 Method Not Allowed", "", b"")
                    }
                    "/redirect" => respond(
                        &mut stream,
                        "302 Found",
                        "Location: /hello\r\nSet-Cookie: r=0; Path=/\r\n",
                        b"",
                    ),
                    "/hello" => respond(
                        &mut stream,
                        "200 OK",
                        "Content-Type: text/plain\r\nSet-Cookie: s=1; Path=/\r\n",
                        b"hello world",
                    ),
                    "/cookie" => {
                        let c = header("cookie").unwrap_or_default();
                        respond(&mut stream, "200 OK", "", c.as_bytes());
                    }
                    "/post" => respond(&mut stream, "201 Created", "", &body),
                    "/cached" => respond(
                        &mut stream,
                        "200 OK",
                        "Cache-Control: max-age=3600\r\n",
                        format!("n={gets}").as_bytes(),
                    ),
                    "/private" => respond(
                        &mut stream,
                        "200 OK",
                        "Cache-Control: max-age=3600, private\r\n",
                        b"p",
                    ),
                    "/nostore" => respond(
                        &mut stream,
                        "200 OK",
                        "Cache-Control: max-age=3600, no-store\r\n",
                        b"x",
                    ),
                    "/etag" if header("if-none-match").as_deref() == Some("\"v1\"") => {
                        respond(&mut stream, "304 Not Modified", "ETag: \"v1\"\r\n", b"")
                    }
                    "/etag" => respond(
                        &mut stream,
                        "200 OK",
                        "Cache-Control: no-cache\r\nETag: \"v1\"\r\n",
                        format!("etag {gets}").as_bytes(),
                    ),
                    "/stall" => {
                        let _ =
                            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nabc");
                        std::thread::sleep(Duration::from_secs(5));
                    }
                    "/hang" => std::thread::sleep(Duration::from_secs(5)),
                    _ => respond(&mut stream, "404 Not Found", "", b"no"),
                }
            });
        }
    });
    (port, seen)
}

/// `isteamhttp.md` §3, §3.1, §7 and §8 against a server on the loopback.
#[cfg(unix)]
#[test]
fn games_make_http_requests() {
    use ossteam_gameipc::{
        Done, HttpCancelRequest, HttpCookieContainerRequest, HttpDone, HttpHeader, HttpSendRequest,
        HttpSetCookieRequest, NoticeBody, RequestBody, ResponseBody,
    };

    let cdn = Arc::new(FakeCdn::new());
    let (os, _) = oses();
    let cm = steam(&cdn, &[("bin/game.sh", os)]);
    let root = Root::new();
    let addr = root.game_address();
    let d = TestDaemon::start_with_cdn(&root, &cm, POLL, cdn.clone());
    login(&d);
    let (port, seen) = http_server();
    let url = |path: &str| format!("http://127.0.0.1:{port}{path}");
    let mut g = GameConn::open(&addr, GAME);
    let mut handle = 0;
    // Sends a request; returns how it ended, its status and body.
    let mut fetch = |g: &mut GameConn, request: HttpSendRequest| {
        handle += 1;
        let request = HttpSendRequest { handle, ..request };
        let ResponseBody::HttpDone(done) = g.ask(RequestBody::HttpSend(request)) else {
            panic!("no HttpDone");
        };
        let mut status = None;
        let mut body = Vec::new();
        for n in std::mem::take(&mut g.notices) {
            match n {
                NoticeBody::HttpHeaders(h) if h.handle == handle => status = Some(h.status),
                NoticeBody::HttpData(d) if d.handle == handle => body.extend(d.data),
                _ => {}
            }
        }
        (done, status, String::from_utf8(body).unwrap())
    };
    let get = |path: &str| HttpSendRequest {
        method: 1,
        url: url(path),
        activity_timeout_ms: 5000,
        ..Default::default()
    };
    let ok = |status| HttpDone {
        successful: true,
        status,
        timed_out: false,
    };

    // A redirect is followed; the User-Agent names the app; cookies of
    // every hop are kept in the container.
    assert_eq!(
        g.ask(RequestBody::HttpCookieContainer(
            HttpCookieContainerRequest {
                handle: 1,
                allow_modify: true
            }
        )),
        ResponseBody::Done(Done {})
    );
    let (done, status, body) = fetch(
        &mut g,
        HttpSendRequest {
            user_agent_info: "extra".into(),
            cookies: 1,
            headers: vec![HttpHeader {
                name: "X-Game".into(),
                value: b"1".to_vec(),
            }],
            ..get("/redirect")
        },
    );
    assert_eq!(
        (done, status, body.as_str()),
        (ok(200), Some(200), "hello world")
    );
    let heads = seen.lock().unwrap().clone();
    assert_eq!(heads.len(), 2);
    assert!(heads[0].starts_with("GET /redirect "), "{}", heads[0]);
    assert!(heads[1].starts_with("GET /hello "), "{}", heads[1]);
    assert!(
        heads[1].contains(&format!(
            "user-agent: Valve/Steam HTTP Client 1.0 ({GAME};extra)"
        )) || heads[1].contains(&format!(
            "User-Agent: Valve/Steam HTTP Client 1.0 ({GAME};extra)"
        )),
        "{}",
        heads[1]
    );
    assert!(heads[1].to_ascii_lowercase().contains("x-game: 1"));
    assert!(heads[1].to_ascii_lowercase().contains("cookie: r=0"));
    let (_, _, body) = fetch(
        &mut g,
        HttpSendRequest {
            cookies: 1,
            ..get("/cookie")
        },
    );
    assert_eq!(body, "r=0; s=1");
    assert_eq!(
        g.ask(RequestBody::HttpSetCookie(HttpSetCookieRequest {
            handle: 1,
            host: "127.0.0.1".into(),
            url: String::new(),
            cookie: "t=2; Path=/cookie".into(),
        })),
        ResponseBody::Done(Done {})
    );
    let (_, _, body) = fetch(
        &mut g,
        HttpSendRequest {
            cookies: 1,
            ..get("/cookie")
        },
    );
    assert_eq!(body, "t=2; r=0; s=1");
    // Without the container, no cookies.
    let (_, _, body) = fetch(&mut g, get("/cookie"));
    assert_eq!(body, "");

    // A POST with a body; a status that is not 200 is still a response.
    let (done, _, body) = fetch(
        &mut g,
        HttpSendRequest {
            method: 3,
            body: b"abc".to_vec(),
            ..get("/post")
        },
    );
    assert_eq!((done, body.as_str()), (ok(201), "abc"));
    let (done, status, _) = fetch(&mut g, get("/none"));
    assert_eq!((done, status), (ok(404), Some(404)));

    // The cache (§3.1). only-if-cached never reaches the server: with
    // nothing stored, the cache answers 504 itself.
    let hits = |path: &str| {
        let get = format!("GET {path} ");
        seen.lock()
            .unwrap()
            .iter()
            .filter(|h| h.starts_with(&get))
            .count()
    };
    let only = |path: &str| HttpSendRequest {
        headers: vec![HttpHeader {
            name: "Cache-Control".into(),
            value: b"only-if-cached, max-stale=2678400".to_vec(),
        }],
        ..get(path)
    };
    let (done, _, body) = fetch(&mut g, only("/cached"));
    assert_eq!(done, ok(504));
    assert!(body.contains("504 Gateway Timeout"), "{body}");
    assert_eq!(hits("/cached"), 0);
    // A fresh response is answered from the cache, streamed too.
    let (_, _, a) = fetch(&mut g, get("/cached"));
    let (_, _, b) = fetch(&mut g, get("/cached"));
    let (done, _, c) = fetch(&mut g, only("/cached"));
    let (_, status, d) = fetch(
        &mut g,
        HttpSendRequest {
            streaming: true,
            ..get("/cached")
        },
    );
    assert_eq!(
        (a.as_str(), b.as_str(), c.as_str(), d.as_str(), done, status),
        ("n=1", "n=1", "n=1", "n=1", ok(200), Some(200))
    );
    assert_eq!(hits("/cached"), 1);
    // A POST to it invalidates it.
    let (done, _, _) = fetch(
        &mut g,
        HttpSendRequest {
            method: 3,
            body: b"x".to_vec(),
            ..get("/cached")
        },
    );
    assert_eq!(done, ok(200));
    let (_, _, e) = fetch(&mut g, get("/cached"));
    assert_eq!((e.as_str(), hits("/cached")), ("n=2", 2));
    // So does a PATCH, as in the official client, though it failed.
    let (done, _, _) = fetch(
        &mut g,
        HttpSendRequest {
            method: 7,
            body: b"x".to_vec(),
            ..get("/cached")
        },
    );
    assert_eq!(done, ok(405));
    let (_, _, e) = fetch(&mut g, get("/cached"));
    assert_eq!((e.as_str(), hits("/cached")), ("n=3", 3));
    // no-cache with an ETag: validated each time; a 304 keeps the body.
    let (_, _, a) = fetch(&mut g, get("/etag"));
    let (done, status, b) = fetch(&mut g, get("/etag"));
    assert_eq!((done, status, a.as_str()), (ok(200), Some(200), "etag 1"));
    assert_eq!(b, a);
    assert_eq!(hits("/etag"), 2);
    let last = seen.lock().unwrap().last().unwrap().to_ascii_lowercase();
    assert!(last.contains("if-none-match: \"v1\""), "{last}");
    let (done, _, _) = fetch(&mut g, only("/etag"));
    assert_eq!((done, hits("/etag")), (ok(504), 2));
    // no-store is never stored.
    fetch(&mut g, get("/nostore"));
    fetch(&mut g, get("/nostore"));
    assert_eq!(hits("/nostore"), 2);
    // As in the official client: a private response is not stored, and
    // HEAD uses the cache, apart from GET.
    fetch(&mut g, get("/private"));
    fetch(&mut g, get("/private"));
    assert_eq!(hits("/private"), 2);
    let head = |path: &str| HttpSendRequest {
        method: 2,
        ..get(path)
    };
    let heads = || {
        seen.lock()
            .unwrap()
            .iter()
            .filter(|h| h.starts_with("HEAD /cached "))
            .count()
    };
    let (a, _, body) = fetch(&mut g, head("/cached"));
    let (b, _, _) = fetch(&mut g, head("/cached"));
    assert_eq!((a, b, body.as_str(), heads()), (ok(200), ok(200), "", 1));
    let (_, _, body) = fetch(&mut g, get("/cached"));
    assert_eq!(body, "n=3");

    // A stalled body runs out of time; other schemes are not made.
    let (done, _, body) = fetch(
        &mut g,
        HttpSendRequest {
            activity_timeout_ms: 300,
            ..get("/stall")
        },
    );
    assert_eq!(
        (done.successful, done.timed_out, body.as_str()),
        (false, true, "abc")
    );
    let (done, _, _) = fetch(
        &mut g,
        HttpSendRequest {
            url: "file:///etc/passwd".into(),
            ..get("")
        },
    );
    assert_eq!(done, HttpDone::default());

    // A cancelled request is never answered; the connection goes on.
    use ossteam_gameipc::{Message, Request};
    ossteam_gameipc::write_message(
        &mut g.writer,
        &Message::Request(Request {
            id: 1000,
            body: RequestBody::HttpSend(HttpSendRequest {
                handle: 1000,
                activity_timeout_ms: 300,
                ..get("/hang")
            }),
        }),
    )
    .unwrap();
    while !seen
        .lock()
        .unwrap()
        .last()
        .is_some_and(|h| h.starts_with("GET /hang"))
    {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        g.ask(RequestBody::HttpCancel(HttpCancelRequest { handle: 1000 })),
        ResponseBody::Done(Done {})
    );
    std::thread::sleep(Duration::from_millis(600));
    let (done, _, _) = fetch(&mut g, get("/hello"));
    assert_eq!(done, ok(200));
    while let Ok(m) = g.rx.try_recv() {
        assert!(
            !matches!(&m, Message::Response(r) if r.id == 1000),
            "the cancelled request was answered"
        );
    }
    g.close();
}

/// `isteamgamecoordinator.md` §1, §2 against the fake CM and its GC.
#[cfg(unix)]
#[test]
fn games_talk_to_their_game_coordinator() {
    use ossteam_gameipc::{
        GcMessage, GcSendRequest, GcSent, NoticeBody, RequestBody, ResponseBody,
    };

    let cdn = Arc::new(FakeCdn::new());
    let (os, _) = oses();
    let cm = steam(&cdn, &[("bin/game.sh", os)]);
    let me = ossteam_types::SteamId::individual(ACCOUNT_ID);
    let root = Root::new();
    let addr = root.game_address();
    let d = TestDaemon::start_with_cdn(&root, &cm, POLL, cdn.clone());
    login(&d);
    // Two connections of the game; only the second talks to the GC.
    let quiet = GameConn::open(&addr, GAME);
    let mut g = GameConn::open(&addr, GAME);
    let send = |msg_type, payload: &[u8]| {
        RequestBody::GcSend(GcSendRequest {
            msg_type,
            payload: payload.to_vec(),
        })
    };

    let hello = [0xa6, 0x0f, 0, 0x80, 0, 0, 0, 0, 1, 2, 3];
    assert_eq!(
        g.ask(send(0x8000_0fa6, &hello)),
        ResponseBody::GcSent(GcSent { result: 0 })
    );
    let mut back = hello.to_vec();
    back.reverse();
    assert_eq!(
        g.notice(),
        NoticeBody::GcMessage(GcMessage {
            msg_type: 0x8000_0fa7,
            payload: back,
        })
    );
    // Steam got the game's bytes as they were, routed to the game's app.
    let sent = cm.gc_sent();
    let [(who, routing, m)] = sent.as_slice() else {
        panic!("{sent:?}")
    };
    assert_eq!((*who, *routing), (me, Some(GAME)));
    assert_eq!(
        (m.appid(), m.msgtype(), m.payload()),
        (GAME, 0x8000_0fa6, &hello[..])
    );

    // Answers come in the order Steam sent them, and only to the
    // connection that talks to the GC.
    for i in 0..3u8 {
        assert_eq!(
            g.ask(send(u32::from(i), &[i])),
            ResponseBody::GcSent(GcSent { result: 0 })
        );
    }
    for i in 0..3u8 {
        assert_eq!(
            g.notice(),
            NoticeBody::GcMessage(GcMessage {
                msg_type: u32::from(i) + 1,
                payload: vec![i],
            })
        );
    }
    assert!(quiet.rx.recv_timeout(Duration::from_millis(300)).is_err());

    // An app the account does not own reaches no GC.
    let mut other = GameConn::open(&addr, 999);
    assert!(matches!(other.ask(send(1, &[])), ResponseBody::Error(e) if e.message.contains("999")));
    assert_eq!(cm.gc_sent().len(), 4);
    d.stop();
}

/// `isteamnetworkingsocketsserialized.md` §3, §5 against the fake CM.
#[cfg(unix)]
#[test]
fn games_get_networking_certificates_and_keep_relay_tickets() {
    use ossteam_gameipc::{
        CacheRelayTicketRequest, Done, NetworkingCertRequest, RelayTickets, RelayTicketsRequest,
        RequestBody, ResponseBody,
    };
    use ossteam_types::EResult;
    use ring::signature::KeyPair as _;

    let cdn = Arc::new(FakeCdn::new());
    let (os, _) = oses();
    let cm = steam(&cdn, &[("bin/game.sh", os)]);
    let root = Root::new();
    let addr = root.game_address();
    let d = TestDaemon::start_with_cdn(&root, &cm, POLL, cdn.clone());
    login(&d);
    let mut g = GameConn::open(&addr, GAME);
    let mut cert = || match g.ask(RequestBody::NetworkingCert(NetworkingCertRequest {})) {
        ResponseBody::NetworkingCert(c) => c,
        other => panic!("{other:?}"),
    };

    // Steam signs the daemon's new public key; the game gets the key pair.
    let first = cert();
    assert_eq!(first.result, 1, "{}", first.message);
    let [(_, asked)] = cm.cert_requests().try_into().expect("one request");
    assert_eq!(asked.app_id(), GAME);
    let public = asked.key_data().to_vec();
    assert_eq!(public.len(), 32);
    assert_eq!(first.private_key.len(), 64);
    assert_eq!(first.private_key[..32], public[..]);
    let pair = ring::signature::Ed25519KeyPair::from_seed_unchecked(&first.private_key[32..])
        .expect("a seed");
    assert_eq!(pair.public_key().as_ref(), &public[..]);
    assert_eq!((first.ca_key_id, first.signature.len()), (42, 64));
    assert!(!first.cert.is_empty());

    // With more than 4 hours left it is the answer again.
    assert_eq!(cert(), first);
    assert_eq!(cm.cert_requests().len(), 1);

    // With less, Steam is asked again. Each app has its own.
    cm.set_certs(2 * 3600, None);
    let mut o = GameConn::open(&addr, OTHER);
    let mut other_cert = || match o.ask(RequestBody::NetworkingCert(NetworkingCertRequest {})) {
        ResponseBody::NetworkingCert(c) => c,
        other => panic!("{other:?}"),
    };
    let short = other_cert();
    assert_eq!(short.result, 1, "{}", short.message);
    let again = other_cert();
    assert_eq!(cm.cert_requests().len(), 3);
    assert_eq!(cm.cert_requests()[2].1.app_id(), OTHER);
    assert_ne!(again.private_key, short.private_key);
    // When Steam refuses, a kept certificate with time left stands in.
    cm.set_certs(2 * 3600, Some(EResult::BUSY));
    assert_eq!(other_cert(), again);
    assert_eq!(cm.cert_requests().len(), 4);
    assert_eq!(cert(), first);

    // Relay tickets: kept once each, in order, for every connection of the
    // app.
    let keep = |g: &mut GameConn, t: &[u8]| {
        assert_eq!(
            g.ask(RequestBody::CacheRelayTicket(CacheRelayTicketRequest {
                ticket: t.to_vec()
            })),
            ResponseBody::Done(Done {})
        );
    };
    keep(&mut g, &[1, 2]);
    keep(&mut g, &[3]);
    keep(&mut g, &[1, 2]);
    keep(&mut g, &[]);
    let mut other = GameConn::open(&addr, GAME);
    assert_eq!(
        other.ask(RequestBody::RelayTickets(RelayTicketsRequest {})),
        ResponseBody::RelayTickets(RelayTickets {
            tickets: vec![vec![1, 2], vec![3]]
        })
    );

    // An app the account does not own gets neither.
    let mut unowned = GameConn::open(&addr, 999);
    assert!(matches!(
        unowned.ask(RequestBody::NetworkingCert(NetworkingCertRequest {})),
        ResponseBody::Error(_)
    ));
    assert!(matches!(
        unowned.ask(RequestBody::RelayTickets(RelayTicketsRequest {})),
        ResponseBody::Error(_)
    ));
    d.stop();
}

/// `isteamappdisableupdate.md` §3 against the fake CM.
#[cfg(unix)]
#[test]
fn games_hold_their_automatic_updates() {
    use ossteam_gameipc::{Done, RequestBody, ResponseBody, UpdateHoldRequest};

    let cdn = Arc::new(FakeCdn::new());
    let (os, _) = oses();
    let cm = steam(&cdn, &[("bin/game.sh", os)]);
    let root = Root::new();
    let addr = root.game_address();
    let d = TestDaemon::start_with_cdn(&root, &cm, POLL, cdn.clone());
    login(&d);
    let c = d.subscribed(&[Topic::Jobs]);
    done(&c, c.install(GAME, "", "").expect("install queued"));
    let mut g = GameConn::open(&addr, GAME);
    let mut hold = |seconds| {
        assert_eq!(
            g.ask(RequestBody::UpdateHold(UpdateHoldRequest { seconds })),
            ResponseBody::Done(Done {})
        );
    };

    // A new build while the game holds updates: marked, not queued.
    hold(900);
    cm.publish(&game_depot(2, b"two"), 2);
    cm.app_section(GAME, "depots", depots_section(2));
    wait_until("the update to be known", || {
        installed(&c, GAME).is_some_and(|i| i.update_available)
    });
    std::thread::sleep(Duration::from_millis(300));
    let updates = |c: &Client| {
        c.list_jobs()
            .expect("jobs")
            .into_iter()
            .filter(|j| j.kind() == v1::JobKind::Update)
            .count()
    };
    assert_eq!(updates(&c), 0);

    // A shorter hold replaces it, and the update is queued when it runs out.
    hold(1);
    let update = wait_event(&c, "an update job", |k| match k {
        event::Kind::JobStateChanged(v1::JobStateChanged { job: Some(j) })
            if j.kind() == v1::JobKind::Update && j.app_id == GAME =>
        {
            Some(j.clone())
        }
        _ => None,
    });
    let j = if super::m1b::finished(update.state()) {
        update
    } else {
        super::m1b::ended(&c, update.job_id)
    };
    assert_eq!(j.state(), v1::JobState::Done, "{:?}", j.error);

    // Ending a hold with 0 queues the update at once.
    hold(900);
    cm.publish(&game_depot(3, b"three"), 3);
    cm.app_section(GAME, "depots", depots_section(3));
    wait_until("the next update to be known", || {
        installed(&c, GAME).is_some_and(|i| i.update_available)
    });
    assert_eq!(updates(&c), 1);
    hold(0);
    wait_until("the held update to be queued", || updates(&c) == 2);
    d.stop();
}

/// `isteamuser.md` §2, §3 against the fake CM.
#[cfg(unix)]
#[test]
fn games_get_tickets() {
    use ossteam_gameipc::{
        AuthTicketRequest, BeginAuthSessionRequest, CancelAuthTicketRequest,
        EncryptedAppTicketRequest, EndAuthSessionRequest, NoticeBody, RequestBody, ResponseBody,
        TicketActive, TicketValidated, UserLicenseRequest,
    };
    use ossteam_proto::Message as _;

    use super::fake_cm::{FAKE_PUBLIC_IP, FAKE_SERVER_TIME, fake_ownership_ticket};
    use crate::tickets::{self, tests::ownership_ticket};

    let cdn = Arc::new(FakeCdn::new());
    let (os, _) = oses();
    let cm = steam(&cdn, &[("bin/game.sh", os)]);
    let me = ossteam_types::SteamId::individual(ACCOUNT_ID);
    let friend = ossteam_types::SteamId::individual(ACCOUNT_ID + 1).0;
    let root = Root::new();
    let addr = root.game_address();
    let d = TestDaemon::start_with_cdn(&root, &cm, POLL, cdn.clone());
    login(&d);
    let mut g = GameConn::open(&addr, GAME);
    let session = |max| {
        RequestBody::AuthTicket(AuthTicketRequest {
            kind: tickets::SESSION,
            identity: String::new(),
            max,
        })
    };

    // A session ticket: a token, the header and the ownership ticket.
    let ResponseBody::AuthTicket(t) = g.ask(session(1024)) else {
        panic!()
    };
    assert_eq!(t.result, 1);
    let split = tickets::split(&t.ticket).expect("a well-formed ticket");
    assert_eq!(split.ownership, &fake_ownership_ticket(me.0, GAME)[..]);
    assert_eq!(split.auth[..4], 20u32.to_le_bytes());
    assert_eq!(split.auth[24 + 12..24 + 16], FAKE_PUBLIC_IP.to_le_bytes());
    assert_eq!(t.crc, tickets::crc(split.auth));
    let first = t.crc;
    assert_eq!(
        g.notice(),
        NoticeBody::TicketActive(TicketActive {
            crc: first,
            result: 1
        })
    );
    let last_list = || cm.auth_lists().last().cloned().expect("an auth list");
    // The daemon answers before Steam has the list.
    let wait_list = |what: &str, f: &dyn Fn(&ossteam_proto::steam::CMsgClientAuthList) -> bool| {
        cm.wait(Duration::from_secs(10), what, |v| {
            v.auth_lists().last().is_some_and(f)
        });
        last_list()
    };
    let l = last_list();
    assert_eq!(l.tickets.len(), 1);
    let entry = &l.tickets[0];
    assert_eq!(
        (
            entry.estate(),
            entry.steamid(),
            entry.gameid(),
            entry.ticket_crc()
        ),
        (0, 0, u64::from(GAME), first)
    );
    assert_eq!(entry.ticket(), split.auth);
    assert_eq!(l.app_ids, [GAME]);

    // A buffer too small, then a Web API ticket.
    let ResponseBody::AuthTicket(t) = g.ask(session(100)) else {
        panic!()
    };
    assert_eq!(t.result, 2);
    let ResponseBody::AuthTicket(web) = g.ask(RequestBody::AuthTicket(AuthTicketRequest {
        kind: tickets::WEB_API,
        identity: "svc".into(),
        max: 0,
    })) else {
        panic!()
    };
    assert_eq!(web.result, 1);
    assert_eq!(web.ticket[24 + 8..24 + 12], 5u32.to_le_bytes());
    assert!(matches!(g.notice(), NoticeBody::TicketActive(a) if a.crc == web.crc));
    let l = last_list();
    assert_eq!(l.tickets[1].server_secret(), b"str:svc\0");

    // Cancelling one.
    assert!(matches!(
        g.ask(RequestBody::CancelAuthTicket(CancelAuthTicketRequest {
            crc: first
        })),
        ResponseBody::Done(_)
    ));
    let l = wait_list("the list without the cancelled ticket", &|l| {
        l.tickets.len() == 1
    });
    assert_eq!(l.tickets[0].ticket_crc(), web.crc);

    // Auth sessions with other users.
    // Each user's client has tokens of its own.
    let other_ticket = |ownership: &[u8]| {
        let token = [&ownership[8..16], &[9; 12]].concat();
        let auth = tickets::auth_ticket(&token, tickets::SESSION, 0, 1, 1);
        tickets::full_ticket(&auth, ownership)
    };
    let begin = |g: &mut GameConn, steam_id: u64, ticket: Vec<u8>| {
        let ResponseBody::BeginAuthSession(b) =
            g.ask(RequestBody::BeginAuthSession(BeginAuthSessionRequest {
                steam_id,
                ticket,
            }))
        else {
            panic!()
        };
        b.result
    };
    let friends = other_ticket(&fake_ownership_ticket(friend, GAME));
    assert_eq!(begin(&mut g, friend, friends.clone()), tickets::BEGIN_OK);
    assert_eq!(
        g.notice(),
        NoticeBody::TicketValidated(TicketValidated {
            steam_id: friend,
            owner: friend,
            response: 0
        })
    );
    assert_eq!(
        begin(&mut g, friend, friends.clone()),
        tickets::BEGIN_DUPLICATE
    );
    assert_eq!(begin(&mut g, friend + 1, friends), tickets::BEGIN_INVALID);
    assert_eq!(
        begin(
            &mut g,
            friend,
            other_ticket(&fake_ownership_ticket(friend, OTHER))
        ),
        tickets::BEGIN_GAME_MISMATCH
    );
    assert_eq!(
        begin(
            &mut g,
            friend + 2,
            other_ticket(&ownership_ticket(friend + 2, GAME, FAKE_SERVER_TIME - 10))
        ),
        tickets::BEGIN_EXPIRED
    );
    assert_eq!(begin(&mut g, friend, vec![1, 2, 3]), tickets::BEGIN_INVALID);
    let l = wait_list("the friend's ticket", &|l| {
        l.tickets.iter().any(|t| t.estate() == 1)
    });
    let theirs = l.tickets.iter().find(|t| t.estate() == 1).unwrap();
    assert_eq!(
        (theirs.steamid(), theirs.gameid()),
        (friend, u64::from(GAME))
    );

    // Licenses come from the session's ownership ticket, which lists DLC
    // 1001.
    let dlc_owner = friend + 3;
    let far = FAKE_SERVER_TIME + 1_000_000;
    cm.set_verdict(5);
    assert_eq!(
        begin(
            &mut g,
            dlc_owner,
            other_ticket(&ownership_ticket(dlc_owner, GAME, far))
        ),
        tickets::BEGIN_OK
    );
    assert_eq!(
        g.notice(),
        NoticeBody::TicketValidated(TicketValidated {
            steam_id: dlc_owner,
            owner: dlc_owner,
            response: 5
        })
    );
    let license = |g: &mut GameConn, steam_id: u64, app_id: u32| {
        let ResponseBody::UserLicense(u) = g.ask(RequestBody::UserLicense(UserLicenseRequest {
            steam_id,
            app_id,
        })) else {
            panic!()
        };
        u.result
    };
    // A ticket Steam turned down is gone, with its session.
    assert_eq!(license(&mut g, dlc_owner, 1001), tickets::LICENSE_NO_AUTH);
    cm.set_verdict(0);
    let dlc_owner = friend + 4;
    assert_eq!(
        begin(
            &mut g,
            dlc_owner,
            other_ticket(&ownership_ticket(dlc_owner, GAME, far))
        ),
        tickets::BEGIN_OK
    );
    assert_eq!(license(&mut g, dlc_owner, 1001), tickets::LICENSE_YES);
    assert_eq!(license(&mut g, dlc_owner, GAME), tickets::LICENSE_YES);
    assert_eq!(license(&mut g, dlc_owner, 1002), tickets::LICENSE_NO);
    assert!(matches!(
        g.ask(RequestBody::EndAuthSession(EndAuthSessionRequest {
            steam_id: friend
        })),
        ResponseBody::Done(_)
    ));
    wait_list("the list without the friend", &|l| {
        !l.tickets.iter().any(|t| t.steamid() == friend)
    });

    // An encrypted app ticket.
    let ResponseBody::EncryptedAppTicket(e) =
        g.ask(RequestBody::EncryptedAppTicket(EncryptedAppTicketRequest {
            userdata: vec![1, 2, 3],
        }))
    else {
        panic!()
    };
    assert_eq!(e.result, 1);
    let decoded = ossteam_proto::steam::EncryptedAppTicket::decode(&e.ticket[..]).unwrap();
    assert_eq!(decoded.encrypted_ticket(), [3, 2, 1]);

    // A game that closes takes its tickets with it.
    let lists = cm.auth_lists().len();
    g.close();
    cm.wait(
        Duration::from_secs(10),
        "the closed game's auth list",
        |v| {
            v.auth_lists().len() > lists
                && v.auth_lists().last().is_some_and(|l| l.tickets.is_empty())
        },
    );
    let l = last_list();
    assert_eq!(l.app_ids, [GAME]);
    drop(d);
}
