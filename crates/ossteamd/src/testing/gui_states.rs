// SPDX-License-Identifier: GPL-2.0-or-later
//! A small library in every state the GUI draws (`gui.md` §4), for the
//! keyboard-only check of `docs/acceptance/m1e.md`: an installed game with
//! two launch options that runs until stopped, a Windows game run through
//! Proton, one with an update, a download slow enough to watch with
//! another queued behind it, a game with a DLC, and accounts that sign in
//! with each kind of Steam Guard. On request, an update of [`NATIVE`]
//! arrives in a new PICS change; each persona state the CM gets is printed.
//! Not a test: it serves a daemon until a client stops it, so it is ignored
//! unless asked for:
//!
//! ```text
//! cargo test -p ossteamd -- --ignored --nocapture gui_bench_states
//! ```

#![cfg(target_os = "linux")]

use std::sync::Arc;
use std::time::Duration;

use ossteam_content::fake::{Depot, Entry};
use ossteam_content::manifest::flags;
use ossteam_ipc::v1::Topic;
use ossteam_types::{DepotId, ManifestId};

use super::gui_bench::{assets, seed_images, serve_gui};
use super::m1b::{data, done, installed, key, login, section};
use super::m1d::{
    MANIFESTS, NATIVE, PROTON, SNIPER, WINDOWS, add_app, manifests_section, tool_depot,
};
use super::*;

/// Installed, with an update.
const UPDATER: u32 = 30;
/// Installing, slowly.
const BIG: u32 = 40;
/// Queued behind [`BIG`].
const QUEUED: u32 = 50;
/// Not installed, with a DLC the account owns.
const FRESH: u32 = 60;
const FRESH_DLC: u32 = 61;
const FRESH_DLC_DEPOT: u32 = 612;
const GAMES: [u32; 6] = [NATIVE, WINDOWS, UPDATER, BIG, QUEUED, FRESH];
/// Each chunk of [`BIG`]'s 40 MiB takes this long: about two minutes
/// with the daemon's eight download threads.
const PACE: Duration = Duration::from_millis(100);
const BIG_SIZE: usize = 40 << 20;
/// The QR code and device confirmations are approved after this long.
const APPROVE_AFTER: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_secs(1);

/// The game: runs until stopped, or exits with 3 when given `-quick`.
/// Given `-report`, it connects to the daemon as the shim does, sends a
/// report and exits (`m1c::shim_in_a_game`), so that the App page has a
/// last shim report to show.
fn game(root: &Root) -> Vec<u8> {
    let exe = std::env::current_exe().expect("the test binary");
    format!(
        r#"#!/bin/sh
case " $* " in
  *" -quick "*) exit 3 ;;
  *" -report "*)
    printf '%s' '{socket}' > shim-address.txt
    exec '{exe}' --exact testing::m1c::shim_in_a_game --ignored --nocapture ;;
esac
exec sleep 600
"#,
        socket = root.socket_dir().display(),
        exe = exe.display(),
    )
    .into_bytes()
}

fn game_depot(app: u32, manifest: u64, files: &[(&'static str, &[u8])]) -> Depot {
    let id = app * 10 + 1;
    let entries: Vec<Entry<'_>> = files
        .iter()
        .map(|&(name, data)| Entry::File {
            name,
            data,
            flags: flags::EXECUTABLE,
        })
        .collect();
    Depot::build(DepotId(id), ManifestId(manifest), key(id), &entries)
}

fn depots(app: u32, os: &str, manifest: u64) -> ossteam_vdf::Object {
    let id = app * 10 + 1;
    section(&format!(
        r#""{id}" {{ "config" {{ "oslist" "{os}" }}
                     "manifests" {{ "public" {{ "gid" "{manifest}" "size" "1" }} }} }}
           "branches" {{ "public" {{ "buildid" "{manifest}" }} }}"#
    ))
}

fn steam(cdn: &Arc<FakeCdn>, big: &[u8], game: &[u8]) -> FakeCm {
    let cm = FakeCm::new()
        .package(0, &[MANIFESTS, SNIPER, PROTON])
        .package(
            200,
            &[NATIVE, WINDOWS, UPDATER, BIG, QUEUED, FRESH, FRESH_DLC],
        )
        .package(300, &[UPDATER, FRESH])
        .app(MANIFESTS, "SteamPlay 2.0 Manifests", "Config", None)
        .app(SNIPER, "Steam Linux Runtime 3.0 (sniper)", "Tool", None)
        .app(PROTON, "Proton Experimental", "Tool", None)
        .app(NATIVE, "Sleeper", "Game", None)
        .app(WINDOWS, "Windows Sleeper", "Game", None)
        .app(UPDATER, "Updater", "Game", None)
        .app(BIG, "Big Download", "Game", None)
        .app(QUEUED, "Queued", "Game", None)
        .app(FRESH, "Fresh", "Game", None)
        .app(FRESH_DLC, "Fresh Extra", "DLC", Some(FRESH))
        .cdn(cdn.clone())
        .qr_rotations(1)
        .account(Account::new("alice", 1001).license(0, 0).license(200, 0))
        .account(
            Account::new("bob", 1002)
                .guard(Guard::EmailCode("BOB12".into()))
                .license(300, 0),
        )
        .account(Account::new("carol", 1003).license(300, 0))
        .account(
            Account::new("dave", 1004)
                .guard(Guard::DeviceConfirmation)
                .license(300, 0),
        )
        .account(
            Account::new("erin", 1005)
                .guard(Guard::DeviceCode("ERIN5".into()))
                .license(300, 0),
        );
    cm.app_section(MANIFESTS, "extended", manifests_section());
    add_app(
        &cm,
        &tool_depot(
            SNIPER,
            r#""manifest" { "version" "2" "commandline" "/_v2-entry-point --verb=%verb% --"
                            "use_tool_subprocess_reaper" "1" }"#,
            "_v2-entry-point",
        ),
        "linux",
        "SteamLinuxRuntime_sniper",
    );
    add_app(
        &cm,
        &tool_depot(
            PROTON,
            &format!(
                r#""manifest" {{ "version" "2" "commandline" "/proton %verb%"
                                "require_tool_appid" "{SNIPER}" }}"#
            ),
            "proton",
        ),
        "linux",
        "Proton - Experimental",
    );
    let big_files: [(&'static str, &[u8]); 2] = [("bin/game.sh", game), ("data.pak", big)];
    for (app, os, exe, dir) in [
        (NATIVE, "linux", "bin/game.sh", "Sleeper"),
        (WINDOWS, "windows", "bin/game.exe", "Windows Sleeper"),
        (UPDATER, "linux", "bin/game.sh", "Updater"),
        (BIG, "linux", "bin/game.sh", "Big Download"),
        (QUEUED, "linux", "bin/game.sh", "Queued"),
        (FRESH, "linux", "bin/game.sh", "Fresh"),
    ] {
        let exe_only: [(&'static str, &[u8]); 1] = [(exe, game)];
        let files: &[(&'static str, &[u8])] = if app == BIG { &big_files } else { &exe_only };
        let depot = game_depot(app, 1, files);
        cm.publish(&depot, 1);
        cm.app_section(app, "depots", depots(app, os, 1));
        cm.app_section(
            app,
            "common",
            section(&format!(r#""oslist" "{os}" {}"#, assets())),
        );
        // Two launch options for the first game, so that Play asks.
        let quick = if app == NATIVE {
            format!(
                r#""1" {{ "executable" "{exe}" "arguments" "-quick" "description" "Quick exit"
                         "config" {{ "oslist" "{os}" }} }}"#
            )
        } else {
            String::new()
        };
        cm.app_section(
            app,
            "config",
            section(&format!(
                r#""installdir" "{dir}"
                   "launch" {{ "0" {{ "executable" "{exe}" "arguments" "-steam"
                                     "config" {{ "oslist" "{os}" }} }} {quick} }}"#
            )),
        );
    }
    let extra = Depot::build(
        DepotId(FRESH_DLC_DEPOT),
        ManifestId(1),
        key(FRESH_DLC_DEPOT),
        &[Entry::File {
            name: "dlc/extra.pak",
            data: b"extra",
            flags: 0,
        }],
    );
    cm.publish(&extra, 1);
    let fresh = section(&format!(
        r#""{d}" {{ "config" {{ "oslist" "linux" }}
                   "manifests" {{ "public" {{ "gid" "1" "size" "1" }} }} }}
           "{FRESH_DLC_DEPOT}" {{ "dlcappid" "{FRESH_DLC}"
                                 "manifests" {{ "public" {{ "gid" "1" "size" "1" }} }} }}
           "branches" {{ "public" {{ "buildid" "1" }} }}"#,
        d = FRESH * 10 + 1
    ));
    cm.app_section(FRESH, "depots", fresh);
    cm
}

#[test]
#[ignore = "serves a daemon for the GUI until stopped"]
fn gui_bench_states() {
    let root = Root::new();
    root.install_client_libraries();
    seed_images(&root, GAMES);
    let cdn = Arc::new(FakeCdn::new());
    let big = data(4, BIG_SIZE);
    let game = game(&root);
    let cm = steam(&cdn, &big, &game);
    // Games get no private ~/.steam, and an official client owns the real
    // one, so the GUI warns (`steamworks-abi.md` §1.1.6).
    let no_private_steam =
        cfg!(target_os = "linux") && std::env::var_os("OSSTEAM_BENCH_NO_PRIVATE_STEAM").is_some();
    if no_private_steam {
        root.refuse_private_steam();
        let steam = root.path().join("home/.steam");
        let official = root.path().join("home/.local/share/Steam/linux64");
        std::fs::create_dir_all(&official).expect("official client");
        std::fs::create_dir_all(&steam).expect("~/.steam");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&official, steam.join("sdk64")).expect("official sdk64");
    }
    let d = TestDaemon::start_with_cdn(&root, &cm, POLL, cdn.clone());
    login(&d);
    let c = d.subscribed(&[Topic::Jobs]);
    c.set_setting("content.auto_update", Some("never"))
        .expect("setting");
    for app in [NATIVE, UPDATER] {
        done(&c, c.install(app, "", "").expect("install queued"));
    }
    let update = game_depot(UPDATER, 2, &[("bin/game.sh", &game), ("readme.txt", b"2")]);
    cm.publish(&update, 2);
    cm.app_section(UPDATER, "depots", depots(UPDATER, "linux", 2));
    wait_until("the update", || {
        installed(&c, UPDATER).is_some_and(|i| i.update_available)
    });
    cdn.pace(PACE);
    for app in [BIG, QUEUED] {
        c.install(app, "", "").expect("install queued");
    }
    drop(c);

    let trigger = root.path().join("update-sleeper");
    {
        let cm = cm.clone();
        let trigger = trigger.clone();
        std::thread::spawn(move || {
            let mut personas = 0;
            loop {
                cm.approve_waiting("carol", APPROVE_AFTER);
                if std::fs::remove_file(&trigger).is_ok() {
                    let update =
                        game_depot(NATIVE, 2, &[("bin/game.sh", &game), ("readme.txt", b"2")]);
                    cm.publish(&update, 2);
                    cm.app_section(NATIVE, "depots", depots(NATIVE, "linux", 2));
                    println!("Sleeper has an update in a new PICS change.");
                }
                let states = cm.persona_states();
                for (id, state) in &states[personas..] {
                    println!("The CM got persona state {state} for {}.", id.0);
                }
                personas = states.len();
                std::thread::sleep(Duration::from_millis(500));
            }
        });
    }
    println!("Signed in as alice. Other accounts, each with the password <name>-password:");
    println!("  bob (an email code, BOB12), dave (a confirmation, approved after 10 s),");
    println!("  erin (an authenticator code, ERIN5); a QR code signs in carol after 10 s.");
    println!("Launched with -report, a game sends a shim report and exits.");
    if !no_private_steam && cfg!(target_os = "linux") {
        println!("Set OSSTEAM_BENCH_NO_PRIVATE_STEAM for games without a private ~/.steam.");
    }
    println!(
        "touch {} to publish an update to Sleeper.",
        trigger.display()
    );
    serve_gui(d, &root, "A library in every state", "");
}
