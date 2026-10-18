// SPDX-License-Identifier: GPL-2.0-or-later
//! SPEC §20: the callback pump's overhead with nothing pending, at most
//! 5 µs per `RunCallbacks`. `steam_api` asks for the next callback
//! through `Steam_BGetCallback` (`steamworks-abi.md` §2.5); this times
//! that call on a connected pipe with an empty queue.
//!
//! For the figure on the reference hardware, run it optimised and read
//! what it prints:
//!
//! ```sh
//! cargo test --release -p ossteam-steamclient --test pump -- --nocapture
//! ```
//!
//! A test binary of its own, so the shim's global state holds just this
//! one pipe.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::time::{Duration, Instant};

use ossteam_gameipc::endpoint::Address;
use ossteam_gameipc::{App, Message, Session, Welcome, read_message, write_message};
use ossteam_steamclient::{Steam_BGetCallback, pipe};

const BUDGET: Duration = Duration::from_micros(5);
const BATCHES: usize = 1000;
const PER_BATCH: u32 = 1000;

/// A daemon that welcomes the pipe and then says nothing.
fn quiet_daemon(dir: &std::path::Path) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let listener = UnixListener::bind(dir.join("game.sock")).unwrap();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut writer = stream.try_clone().unwrap();
        while let Ok(msg) = read_message(&mut stream) {
            if let Message::Hello(_) = msg {
                let welcome = Message::Welcome(Welcome {
                    session: Session {
                        active: true,
                        logged_on: true,
                        steam_id: 76561197960287930,
                        ..Session::default()
                    },
                    app: App {
                        app_id: 480,
                        owned: true,
                        ..App::default()
                    },
                });
                write_message(&mut writer, &welcome).unwrap();
            }
        }
    });
}

#[test]
fn an_empty_pump_stays_within_budget() {
    let dir = std::env::temp_dir().join(format!("ossteam-pump-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    quiet_daemon(&dir);
    pipe::override_endpoint(Address::in_dir(&dir));
    let handle = pipe::create();
    assert_ne!(handle, 0, "the pipe connects");

    // Room for a CallbackMsg_t on any target.
    let mut msg = [0u64; 4];
    let mut pump = || {
        // SAFETY: `msg` is writable and large enough.
        unsafe { Steam_BGetCallback(handle, msg.as_mut_ptr().cast(), std::ptr::null_mut()) }
    };
    for _ in 0..PER_BATCH {
        assert!(!pump(), "nothing is pending");
    }
    let mut per_call: Vec<Duration> = (0..BATCHES)
        .map(|_| {
            let start = Instant::now();
            for _ in 0..PER_BATCH {
                std::hint::black_box(pump());
            }
            start.elapsed() / PER_BATCH
        })
        .collect();
    per_call.sort();
    let median = per_call[BATCHES / 2];
    let p99 = per_call[BATCHES * 99 / 100];
    eprintln!(
        "Steam_BGetCallback with nothing pending ({}, {}): median {median:?}, p99 {p99:?} per call",
        std::env::consts::ARCH,
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
    );
    assert!(
        median <= BUDGET,
        "median {median:?} is over the {BUDGET:?} budget"
    );
    assert!(pipe::release(handle));
    let _ = std::fs::remove_dir_all(&dir);
}
