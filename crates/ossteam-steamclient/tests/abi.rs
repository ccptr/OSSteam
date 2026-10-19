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
//!
//! On Windows the game needs the MSVC C++ ABI. `$OSSTEAM_ABI_GAME` names
//! a game built beforehand; otherwise `$CXX`, else `clang-cl`, builds it.
//! From Linux, under Wine, clang builds it for the MSVC ABI and MinGW
//! links it (`i686` for the 32-bit library):
//!
//! ```sh
//! clang --target=x86_64-pc-windows-msvc -std=c++11 -Wall -Werror -fno-exceptions \
//!     -c crates/ossteam-steamclient/tests/abi/game.cpp -o /tmp/game.obj
//! x86_64-w64-mingw32-g++ -static -o /tmp/game.exe /tmp/game.obj
//! OSSTEAM_ABI_GAME='Z:\tmp\game.exe' CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUNNER=wine \
//!     cargo test -p ossteam-steamclient --test abi --target x86_64-pc-windows-gnu
//! ```
//!
//! The library finds the fake daemon through `OSSTEAM_TEST_ROOT` there,
//! which only debug builds read.

use std::io::{Read, Write};
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

/// Answers one game connection: welcomes it to app 480 and answers stats
/// requests, `NumGames` being 5 for the account and 9 for anyone else.
fn serve(mut stream: impl Read + Write) {
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
        if write_message(&mut stream, &answer).is_err() {
            break;
        }
    }
}

/// A fake daemon where the library looks for it with `env` set: the game
/// socket under `$XDG_RUNTIME_DIR/ossteam`.
#[cfg(unix)]
fn daemon(base: &Path) -> Vec<(&'static str, PathBuf)> {
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixListener;

    let dir = base.join("ossteam");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let listener = UnixListener::bind(dir.join("game.sock")).unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let stream = stream.unwrap();
            std::thread::spawn(move || serve(stream));
        }
    });
    vec![("XDG_RUNTIME_DIR", base.into()), ("TMPDIR", base.into())]
}

/// As above: the named pipe of the test directory `base`.
#[cfg(windows)]
fn daemon(base: &Path) -> Vec<(&'static str, PathBuf)> {
    use interprocess::local_socket::traits::ListenerExt as _;
    use interprocess::local_socket::{GenericNamespaced, ListenerOptions, ToNsName};

    let dir = base.file_name().unwrap().to_string_lossy();
    let name = format!("ossteam-game-test-{dir}");
    let listener = ListenerOptions::new()
        .name(name.to_ns_name::<GenericNamespaced>().unwrap())
        .create_sync()
        .unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let stream = stream.unwrap();
            std::thread::spawn(move || serve(stream));
        }
    });
    vec![("OSSTEAM_TEST_ROOT", base.into())]
}

/// The library cargo built beside this test, in `target/<profile>/deps/`.
/// The copy in `target/<profile>/` is only refreshed by `cargo build`, so
/// a test run could find a stale one there.
fn library() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    exe.parent().unwrap().join(format!(
        "{}ossteam_steamclient{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    ))
}

/// Builds the game into `dir` with the platform's C++ compiler.
#[cfg(unix)]
fn build_game(dir: &Path) -> PathBuf {
    let game = dir.join("game");
    let cxx = std::env::var("CXX").unwrap_or_else(|_| "c++".into());
    let mut build = Command::new(&cxx);
    build.args(["-std=c++11", "-Wall", "-Werror", "-o"]);
    build.arg(&game).arg(source());
    if cfg!(target_arch = "x86") {
        build.arg("-m32");
    }
    if cfg!(target_os = "linux") {
        build.arg("-ldl");
    }
    compile(&cxx, build);
    game
}

/// As above, with an MSVC-ABI compiler, unless one was built beforehand.
#[cfg(windows)]
fn build_game(dir: &Path) -> PathBuf {
    if let Some(game) = std::env::var_os("OSSTEAM_ABI_GAME") {
        return game.into();
    }
    let game = dir.join("game.exe");
    let cxx = std::env::var("CXX").unwrap_or_else(|_| "clang-cl".into());
    let mut build = Command::new(&cxx);
    build.args(["/nologo", "/W3", "/WX", "/EHs-c-"]);
    build.arg(source());
    build.arg(format!("/Fe{}", game.display()));
    build.arg(format!("/Fo{}\\", dir.display()));
    compile(&cxx, build);
    game
}

fn source() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/abi/game.cpp")
}

fn compile(cxx: &str, mut build: Command) {
    let built = build
        .output()
        .unwrap_or_else(|e| panic!("cannot run the C++ compiler {cxx} (set CXX): {e}"));
    assert!(
        built.status.success(),
        "{cxx} failed:\n{}{}",
        String::from_utf8_lossy(&built.stdout),
        String::from_utf8_lossy(&built.stderr)
    );
}

#[test]
fn a_cpp_game_calls_the_library() {
    let base = std::env::temp_dir().join(format!("ossteam-abi-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let env = daemon(&base);

    let lib = library();
    assert!(lib.exists(), "no library at {}", lib.display());
    let game = build_game(&base);

    let run = Command::new(&game)
        .arg(&lib)
        .envs(env)
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
