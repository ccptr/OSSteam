// SPDX-License-Identifier: GPL-2.0-or-later
//! `ossteamd --reaper` (`launch.md` §4): it waits for the whole tree, not
//! only the program it started, and ends as that program did.
#![cfg(target_os = "linux")]

use std::os::unix::process::ExitStatusExt;
use std::process::{Command, ExitStatus};
use std::time::{Duration, Instant};

fn reaper(script: &str) -> (ExitStatus, Duration) {
    let started = Instant::now();
    let status = Command::new(env!("CARGO_BIN_EXE_ossteamd"))
        .args(["--reaper", "AppId=480", "--", "/bin/sh", "-c", script])
        .status()
        .expect("run the reaper");
    (status, started.elapsed())
}

#[test]
fn waits_for_orphans_and_keeps_the_exit_code() {
    // The shell exits at once; the orphaned sleep is re-parented to the
    // reaper, which waits for it.
    let (status, took) = reaper("sleep 1 & exit 3");
    assert_eq!(status.code(), Some(3));
    assert!(
        took >= Duration::from_millis(900),
        "returned after {took:?}"
    );
}

#[test]
fn ends_by_the_signal_that_ended_the_program() {
    let (status, _) = reaper("kill -TERM $$");
    assert_eq!(status.signal(), Some(15));
    // SIGPIPE, which the Rust runtime ignores, is raised as well.
    let (status, _) = reaper("kill -PIPE $$");
    assert_eq!(status.signal(), Some(13));
    assert!(!status.core_dumped());
}

#[test]
fn a_program_that_cannot_start() {
    let status = Command::new(env!("CARGO_BIN_EXE_ossteamd"))
        .args(["--reaper", "AppId=480", "--", "/nonexistent/game"])
        .status()
        .expect("run the reaper");
    assert_eq!(status.code(), Some(127));
    let status = Command::new(env!("CARGO_BIN_EXE_ossteamd"))
        .args(["--reaper", "AppId=480"])
        .status()
        .expect("run the reaper");
    assert_eq!(status.code(), Some(2));
}
