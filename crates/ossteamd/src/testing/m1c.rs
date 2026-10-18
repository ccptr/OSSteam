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
