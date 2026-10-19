// SPDX-License-Identifier: GPL-2.0-or-later
//! SPEC §19.1 level 4: a C++ game (`abi/game.cpp`), built by the
//! platform's C++ compiler, loads the library and calls it through the
//! vtables the compiler lays out. That checks the calling conventions,
//! the class returns and the overload order of `steamworks-abi.md` §2.2
//! and §2.4 against a compiler rather than against the shim's own tables.
//!
//! The compiler is `$CXX`, else `c++`. For the 32-bit library, run this
//! test for the 32-bit target; the game is then built with `-m32`:
//!
//! ```sh
//! cargo test -p ossteam-steamclient --test abi --target i686-unknown-linux-gnu
//! ```

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::Command;

use ossteam_gameipc::{
    App, ErrorReply, Message, RequestBody, Response, ResponseBody, Session, StatDef, StatValue,
    StatsSchema, UserStats, Welcome, read_message, write_message,
};

const STEAM_ID: u64 = 76561197960287930;

fn schema() -> StatsSchema {
    StatsSchema {
        version: 1,
        stats: vec![
            StatDef {
                id: 2,
                name: "NumGames".into(),
                kind: 1,
                ..Default::default()
            },
            StatDef {
                id: 3,
                name: "FeetTraveled".into(),
                kind: 2,
                default: 1.5f32.to_bits(),
                ..Default::default()
            },
        ],
    }
}

/// A daemon that welcomes the game to app 480 and answers stats requests:
/// `NumGames` is 5 for the account and 9 for anyone else.
fn daemon(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let listener = UnixListener::bind(dir.join("game.sock")).unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            std::thread::spawn(move || {
                let mut writer = stream.try_clone().unwrap();
                while let Ok(msg) = read_message(&mut stream) {
                    let answer = match msg {
                        Message::Hello(_) => Message::Welcome(Welcome {
                            session: Session {
                                active: true,
                                logged_on: true,
                                steam_id: STEAM_ID,
                                ip_country: "DE".into(),
                                server_time: 1_790_000_000,
                                universe: 1,
                                ..Session::default()
                            },
                            app: App {
                                app_id: 480,
                                owned: true,
                                launched: true,
                                owner: STEAM_ID,
                                ..App::default()
                            },
                        }),
                        Message::Request(r) => {
                            let body = match r.body {
                                RequestBody::UserStats(s) => ResponseBody::UserStats(UserStats {
                                    result: 1,
                                    steam_id: if s.steam_id == 0 {
                                        STEAM_ID
                                    } else {
                                        s.steam_id
                                    },
                                    crc: 1,
                                    schema: schema(),
                                    values: vec![StatValue {
                                        id: 2,
                                        value: if s.steam_id == 0 { 5 } else { 9 },
                                    }],
                                    unlocks: vec![],
                                }),
                                other => ResponseBody::Error(ErrorReply {
                                    message: format!("unexpected {other:?}"),
                                }),
                            };
                            Message::Response(Response { id: r.id, body })
                        }
                        _ => continue,
                    };
                    if write_message(&mut writer, &answer).is_err() {
                        break;
                    }
                }
            });
        }
    });
}

/// The library cargo built beside this test (`target/<profile>/`).
fn library() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let dir = exe.parent().unwrap().parent().unwrap();
    dir.join(format!(
        "{}ossteam_steamclient{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    ))
}

#[test]
fn a_cpp_game_calls_the_library() {
    let base = std::env::temp_dir().join(format!("ossteam-abi-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    daemon(&base.join("ossteam"));

    let lib = library();
    assert!(lib.exists(), "no library at {}", lib.display());
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/abi/game.cpp");
    let game = base.join("game");
    let cxx = std::env::var("CXX").unwrap_or_else(|_| "c++".into());
    let mut build = Command::new(&cxx);
    build.args(["-std=c++11", "-Wall", "-Werror", "-o"]);
    build.arg(&game).arg(&source);
    if cfg!(target_arch = "x86") {
        build.arg("-m32");
    }
    if cfg!(target_os = "linux") {
        build.arg("-ldl");
    }
    let built = build
        .output()
        .unwrap_or_else(|e| panic!("cannot run the C++ compiler {cxx} (set CXX): {e}"));
    assert!(
        built.status.success(),
        "{cxx} failed:\n{}",
        String::from_utf8_lossy(&built.stderr)
    );

    let run = Command::new(&game)
        .arg(&lib)
        .env("XDG_RUNTIME_DIR", &base)
        .env("TMPDIR", &base)
        .env("SteamAppId", "480")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&run.stdout);
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        run.status.success() && stdout.trim() == "ok",
        "the game exited with {}:\n{stdout}{stderr}",
        run.status
    );
    let _ = std::fs::remove_dir_all(&base);
}
