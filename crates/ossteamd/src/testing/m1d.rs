// SPDX-License-Identifier: GPL-2.0-or-later
//! M1d against the fake CM and a fake CDN: compatibility tools
//! (`compat-tools.md`). Valve's tool list comes from app 891390; the
//! tools are scripts that write down how they were started and run what
//! follows, as the real ones run the game.

#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use ossteam_content::fake::{Depot, Entry};
use ossteam_content::manifest::flags;
use ossteam_ipc::Client;
use ossteam_ipc::v1::{self, Topic, event, game_exited};
use ossteam_types::{DepotId, ManifestId};

use super::m1b::{done, installed, key, login, section};
use super::*;

const ACCOUNT_ID: u32 = 1001;
const POLL: Duration = Duration::from_millis(200);
/// A Linux game that Valve maps to the sniper runtime.
const NATIVE: u32 = 20;
/// A Windows-only game, run by the default tool.
const WINDOWS: u32 = 22;
const MANIFESTS: u32 = 891390;
const SNIPER: u32 = 1628350;
const PROTON: u32 = 1493710;

/// The game: writes how it was started into its working directory and
/// exits with 3.
const GAME: &[u8] = br#"#!/bin/sh
printf '%s\n' "$@" > args.txt
env > env.txt
exit 3
"#;

/// A tool's entry point: writes its arguments and environment next to
/// itself, skips its own arguments up to `--` (the runtime) or the verb
/// (Proton), and runs the rest.
const TOOL: &[u8] = br#"#!/bin/sh
here="${0%/*}"
printf '%s\n' "$@" > "$here/args.txt"
env > "$here/env.txt"
case "$1" in
  --verb=*)
    while [ "$#" -gt 0 ]; do a="$1"; shift; [ "$a" = "--" ] && break; done ;;
  *) shift ;;
esac
exec "$@"
"#;

fn manifests_section() -> ossteam_vdf::Object {
    section(&format!(
        r#""compat_tools"
           {{
               "proton_experimental"
               {{
                   "appid" "{PROTON}" "depotid" "{}" "require_tool_appid" "{SNIPER}"
                   "display_name" "Proton Experimental" "from_oslist" "windows"
                   "to_oslist" "linux" "aliases" "proton-experimental"
               }}
               "steamlinuxruntime_sniper"
               {{
                   "appid" "{SNIPER}" "depotid" "{}" "display_name" "Steam Linux Runtime 3.0"
                   "from_oslist" "linux" "to_oslist" "linux" "unlisted" "1" "aliases" "sniper"
               }}
           }}
           "app_mappings"
           {{
               "{NATIVE}" {{ "appid" "{NATIVE}" "platform" "linux" "tool" "SteamLinuxRuntime_sniper" }}
           }}
           "app_compat_configs"
           {{
               "{WINDOWS}" {{ "appid" "{WINDOWS}" "config" "noesync,nofsync" "environment" "FAKE_HDR=$SteamAppId-0" }}
           }}"#,
        PROTON + 1,
        SNIPER + 1
    ))
}

fn tool_depot(app: u32, manifest: &str, entry: &'static str) -> Depot {
    let id = app + 1;
    let manifest: &'static [u8] = Box::leak(manifest.as_bytes().to_vec().into_boxed_slice());
    Depot::build(
        DepotId(id),
        ManifestId(u64::from(id)),
        key(id),
        &[
            Entry::File {
                name: "toolmanifest.vdf",
                data: manifest,
                flags: 0,
            },
            Entry::File {
                name: entry,
                data: TOOL,
                flags: flags::EXECUTABLE,
            },
        ],
    )
}

fn game_depot(app: u32, exe: &'static str) -> Depot {
    let id = app * 10 + 1;
    Depot::build(
        DepotId(id),
        ManifestId(u64::from(id)),
        key(id),
        &[Entry::File {
            name: exe,
            data: GAME,
            flags: flags::EXECUTABLE,
        }],
    )
}

/// An app with one depot for `os` and its install directory.
fn add_app(cm: &FakeCm, depot: &Depot, os: &str, installdir: &str) {
    let id = depot.manifest.depot.0;
    let app = depot_app(id);
    cm.publish(depot, u64::from(id));
    cm.app_section(
        app,
        "depots",
        section(&format!(
            r#""{id}" {{ "config" {{ "oslist" "{os}" }}
                         "manifests" {{ "public" {{ "gid" "{id}" "size" "1" }} }} }}
               "branches" {{ "public" {{ "buildid" "1" }} }}"#
        )),
    );
    cm.app_section(
        app,
        "config",
        section(&format!(r#""installdir" "{installdir}""#)),
    );
}

fn depot_app(depot: u32) -> u32 {
    match depot {
        d if d == SNIPER + 1 => SNIPER,
        d if d == PROTON + 1 => PROTON,
        d => d / 10,
    }
}

fn steam(cdn: &Arc<FakeCdn>) -> FakeCm {
    let cm = FakeCm::new()
        .package(0, &[MANIFESTS, SNIPER, PROTON])
        .package(200, &[NATIVE, WINDOWS])
        .app(NATIVE, "Native", "Game", None)
        .app(WINDOWS, "Windows", "Game", None)
        .app(MANIFESTS, "SteamPlay 2.0 Manifests", "Config", None)
        .app(SNIPER, "Steam Linux Runtime 3.0 (sniper)", "Tool", None)
        .app(PROTON, "Proton Experimental", "Tool", None)
        .cdn(cdn.clone())
        .account(
            Account::new("alice", ACCOUNT_ID)
                .license(0, 0)
                .license(200, 0),
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
    for (app, os, exe, dir) in [
        (NATIVE, "linux", "bin/game.sh", "Native"),
        (WINDOWS, "windows", "bin/game.exe", "Windows"),
    ] {
        add_app(&cm, &game_depot(app, exe), os, dir);
        cm.app_section(app, "common", section(&format!(r#""oslist" "{os}""#)));
        cm.app_section(
            app,
            "config",
            section(&format!(
                r#""installdir" "{dir}"
                   "launch" {{ "0" {{ "executable" "{exe}" "arguments" "-steam"
                                     "config" {{ "oslist" "{os}" }} }} }}"#
            )),
        );
    }
    cm
}

fn install(c: &Client, app: u32) -> PathBuf {
    done(c, c.install(app, "", "").expect("install queued"));
    PathBuf::from(installed(c, app).expect("installed").path)
}

fn exited(c: &Client, app: u32) -> v1::GameExited {
    wait_event(c, "the game to exit", |k| match k {
        event::Kind::GameExited(e) if e.app_id == app => Some(*e),
        _ => None,
    })
}

fn lines(p: &Path) -> Vec<String> {
    std::fs::read_to_string(p)
        .unwrap_or_else(|e| panic!("{}: {e}", p.display()))
        .lines()
        .map(str::to_owned)
        .collect()
}

fn env_of(p: &Path) -> Vec<String> {
    lines(&p.join("env.txt"))
}

fn has(env: &[String], var: &str) -> bool {
    env.iter().any(|l| l == var)
}

#[test]
fn a_native_game_runs_in_its_runtime_once_it_is_installed() {
    let cdn = Arc::new(FakeCdn::new());
    let cm = steam(&cdn);
    let root = Root::new();
    root.install_client_libraries();
    let d = TestDaemon::start_with_cdn(&root, &cm, POLL, cdn.clone());
    login(&d);
    let c = d.subscribed(&[Topic::Jobs, Topic::Games]);
    c.set_setting("content.auto_update", Some("never"))
        .expect("setting");
    let game = install(&c, NATIVE);
    assert!(installed(&c, SNIPER).is_none());

    // The runtime is installed first; the reply comes once the game runs.
    let running = c
        .launch(NATIVE, None, &["it's".into()], false)
        .expect("launched");
    let sniper = PathBuf::from(installed(&c, SNIPER).expect("the runtime").path);
    assert_eq!(sniper.file_name().unwrap(), "SteamLinuxRuntime_sniper");
    let e = exited(&c, NATIVE);
    assert_eq!(
        e.status,
        Some(game_exited::Status::ExitCode(3)),
        "{}",
        std::fs::read_to_string(&running.log).unwrap_or_default()
    );

    // The runtime got `--verb=waitforexitandrun --` and the game's line.
    assert_eq!(
        lines(&sniper.join("args.txt")),
        [
            "--verb=waitforexitandrun",
            "--",
            game.join("bin/game.sh").to_str().unwrap(),
            "-steam",
            "it's",
        ]
    );
    assert_eq!(lines(&game.join("args.txt")), ["-steam", "it's"]);

    let library = game.parent().unwrap().parent().unwrap();
    let data = library.join("compatdata").join(NATIVE.to_string());
    assert!(data.is_dir());
    let env = env_of(&sniper);
    for var in [
        format!("STEAM_COMPAT_APP_ID={NATIVE}"),
        format!("STEAM_COMPAT_INSTALL_PATH={}", game.display()),
        format!("STEAM_COMPAT_DATA_PATH={}", data.display()),
        format!(
            "STEAM_COMPAT_CLIENT_INSTALL_PATH={}",
            root.path().join("data").join("compat-client").display()
        ),
        format!("STEAM_COMPAT_LIBRARY_PATHS={}", library.display()),
        format!("STEAM_COMPAT_TOOL_PATHS={}", sniper.display()),
        // The test's data directory is not under its home.
        format!(
            "STEAM_COMPAT_MOUNTS={}",
            root.path().join("data").join("steam-sdk").display()
        ),
        format!("SteamAppId={NATIVE}"),
    ] {
        assert!(has(&env, &var), "{var} in {env:#?}");
    }
    assert!(!env.iter().any(|l| l.starts_with("STEAM_COMPAT_CONFIG=")));
    assert!(
        root.path()
            .join("data")
            .join("compat-client")
            .join("legacycompat")
            .is_dir()
    );
    d.stop();
}

#[test]
fn a_windows_game_gets_windows_depots_and_proton_in_the_runtime() {
    let cdn = Arc::new(FakeCdn::new());
    let cm = steam(&cdn);
    let root = Root::new();
    root.install_client_libraries();
    let d = TestDaemon::start_with_cdn(&root, &cm, POLL, cdn.clone());
    login(&d);
    let c = d.subscribed(&[Topic::Jobs, Topic::Games]);
    c.set_setting("content.auto_update", Some("never"))
        .expect("setting");
    // Only the Windows depot exists, and the default tool runs it.
    let game = install(&c, WINDOWS);
    assert!(game.join("bin/game.exe").is_file());

    c.launch(WINDOWS, None, &[], false).expect("launched");
    let proton = PathBuf::from(installed(&c, PROTON).expect("proton").path);
    let sniper = PathBuf::from(installed(&c, SNIPER).expect("the runtime").path);
    assert_eq!(
        exited(&c, WINDOWS).status,
        Some(game_exited::Status::ExitCode(3))
    );
    // Outermost first: the runtime runs Proton, which runs the game.
    assert_eq!(
        lines(&sniper.join("args.txt")),
        [
            "--verb=waitforexitandrun",
            "--",
            proton.join("proton").to_str().unwrap(),
            "waitforexitandrun",
            game.join("bin/game.exe").to_str().unwrap(),
            "-steam",
        ]
    );
    assert_eq!(
        lines(&proton.join("args.txt")),
        [
            "waitforexitandrun",
            game.join("bin/game.exe").to_str().unwrap(),
            "-steam"
        ]
    );
    let env = env_of(&game);
    let tools = format!(
        "STEAM_COMPAT_TOOL_PATHS={}:{}",
        proton.display(),
        sniper.display()
    );
    for var in [
        tools.as_str(),
        "STEAM_COMPAT_CONFIG=noesync,nofsync",
        "FAKE_HDR=22-0",
    ] {
        assert!(has(&env, var), "{var} in {env:#?}");
    }

    // Without a default tool the Windows game has no tool, and no
    // Windows launch option qualifies on Linux.
    c.set_setting("compat.default_tool", Some(""))
        .expect("setting");
    let err = c.launch(WINDOWS, None, &[], false).unwrap_err();
    assert!(err.to_string().contains("windows"), "{err}");
    // A choice must name a tool.
    assert!(c.set_setting("compat.tool.22", Some("proton_4")).is_err());
    c.set_setting("compat.tool.22", Some("proton-experimental"))
        .expect("an alias");
    d.stop();
}
