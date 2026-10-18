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

/// The game: writes how it was started next to itself, then does what its
/// first extra argument says.
const SCRIPT: &[u8] = br#"#!/bin/sh
pwd > cwd.txt
printf '%s\n' "$@" > args.txt
env > env.txt
echo "game output"
case "$2" in
  exit) exit 3 ;;
  wait) exec sleep 60 ;;
  shim) exec "$3" --exact testing::m1c::shim_in_a_game --ignored --nocapture ;;
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
        std::os::unix::fs::symlink(&valve, steam.join("sdk64")).unwrap();
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
    wait_until("the entries to go", || !exists(&pid_file));
    assert!(!exists(&steam.join("sdk64")));
    assert!(!exists(&steam.join("sdk32")));
    login(&d);
    wait_until("the entries", || exists(&pid_file));
    d.stop();
    assert!(!exists(&pid_file));
    assert!(!exists(&steam.join("sdk64")));
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
    let bin = dir.join("bin");
    let read = |n: &str| std::fs::read_to_string(bin.join(n)).unwrap_or_default();
    assert_eq!(read("args.txt"), "-steam\nexit\nit's\n");
    assert_eq!(read("cwd.txt").trim(), bin.to_str().unwrap());
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
fn shim_connect(
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

fn hello(app_id: u32) -> ossteam_gameipc::Hello {
    ossteam_gameipc::Hello {
        protocol: ossteam_gameipc::PROTOCOL,
        pid: std::process::id(),
        app_id,
        arch: "x86_64".into(),
        version: "test".into(),
    }
}

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
    let bin = dir.join("bin");
    std::fs::write(
        bin.join("shim-address.txt"),
        root.path().join("run").to_str().unwrap(),
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
        std::fs::read_to_string(bin.join("launched.txt")).unwrap(),
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
