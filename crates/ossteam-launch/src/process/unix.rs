// SPDX-License-Identifier: GPL-2.0-or-later
//! Game processes on Linux and macOS (`launch.md` §4 and §5).
//!
//! Linux runs the game under a reaper, a re-execution of the daemon with
//! `--reaper` that is the child subreaper of the game's tree, as the
//! official client's `reaper` is. macOS has no subreaper, so the game's
//! process group stands for its tree there.

use std::fs::File;
use std::io;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use rustix::process::{Pid, Signal};

use super::{Exit, Spec};

pub struct Game {
    child: Child,
    done: Arc<AtomicBool>,
}

impl Game {
    pub fn spawn(spec: &Spec) -> io::Result<Self> {
        let log = File::create(&spec.log)?;
        let mut cmd = if cfg!(target_os = "linux") {
            let mut c = Command::new(&spec.reaper);
            c.arg("--reaper")
                .arg(format!("AppId={}", spec.app.0))
                .arg("--");
            c
        } else {
            Command::new("/bin/sh")
        };
        if cfg!(target_os = "linux") {
            cmd.arg("/bin/sh");
        }
        cmd.arg("-c")
            .arg(&spec.command_line)
            .current_dir(&spec.working_dir)
            .envs(spec.env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            // Its own group, so that signals meant for the daemon's group
            // do not reach the game.
            .process_group(0);
        let child = cmd.spawn()?;
        Ok(Self {
            child,
            done: Arc::default(),
        })
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    pub fn stopper(&self) -> Stopper {
        Stopper {
            pid: self.child.id() as i32,
            done: self.done.clone(),
        }
    }

    pub fn wait(mut self) -> io::Result<Exit> {
        let status = self.child.wait();
        // On Linux the reaper has outlived the tree. Elsewhere the group
        // may still have members after its leader is gone.
        #[cfg(not(target_os = "linux"))]
        if status.is_ok() {
            wait_for_group(self.child.id() as i32);
        }
        self.done.store(true, Ordering::SeqCst);
        let status = status?;
        Ok(match (status.code(), status.signal()) {
            (Some(c), _) => Exit::Code(i64::from(c)),
            (None, Some(s)) => Exit::Signal(s),
            (None, None) => Exit::Code(-1),
        })
    }
}

/// Checks once a second until no process is left in the group.
#[cfg(not(target_os = "linux"))]
fn wait_for_group(pgid: i32) {
    let Some(pgid) = Pid::from_raw(pgid) else {
        return;
    };
    while rustix::process::test_kill_process_group(pgid) != Err(rustix::io::Errno::SRCH) {
        std::thread::sleep(Duration::from_secs(1));
    }
}

#[derive(Clone)]
pub struct Stopper {
    pid: i32,
    done: Arc<AtomicBool>,
}

impl Stopper {
    pub fn stop(&self, timeout: Duration) {
        if self.done.load(Ordering::SeqCst) {
            return;
        }
        signal_tree(self.pid, Signal::TERM);
        let s = self.clone();
        std::thread::Builder::new()
            .name("game-stop".into())
            .spawn(move || {
                let deadline = Instant::now() + timeout;
                while Instant::now() < deadline {
                    if s.done.load(Ordering::SeqCst) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                if !s.done.load(Ordering::SeqCst) {
                    tracing::info!(pid = s.pid, "the game did not close in time; killing it");
                    signal_tree(s.pid, Signal::KILL);
                }
            })
            .map(drop)
            .unwrap_or_else(|e| tracing::error!("starting the stop timer failed: {e}"));
    }
}

impl Stopper {
    #[cfg(target_os = "linux")]
    pub fn contains(&self, pid: u32) -> bool {
        !self.done.load(Ordering::SeqCst)
            && i32::try_from(pid).is_ok_and(|p| descendants(self.pid).contains(&p))
    }

    #[cfg(not(target_os = "linux"))]
    pub fn contains(&self, pid: u32) -> bool {
        if self.done.load(Ordering::SeqCst) {
            return false;
        }
        let Some(p) = i32::try_from(pid).ok().and_then(Pid::from_raw) else {
            return false;
        };
        rustix::process::getpgid(Some(p)).is_ok_and(|g| g.as_raw_nonzero().get() == self.pid)
    }
}

/// Linux: every descendant of the reaper, but not the reaper, which must
/// outlive them.
#[cfg(target_os = "linux")]
fn signal_tree(reaper: i32, sig: Signal) {
    for pid in descendants(reaper) {
        if let Some(p) = Pid::from_raw(pid) {
            let _ = rustix::process::kill_process(p, sig);
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn signal_tree(pgid: i32, sig: Signal) {
    if let Some(p) = Pid::from_raw(pgid) {
        let _ = rustix::process::kill_process_group(p, sig);
    }
}

/// The descendants of `root`, from `/proc`.
#[cfg(target_os = "linux")]
fn descendants(root: i32) -> Vec<i32> {
    let mut children: std::collections::HashMap<i32, Vec<i32>> = Default::default();
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    for entry in dir.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<i32>().ok())
        else {
            continue;
        };
        if let Some(ppid) = std::fs::read_to_string(entry.path().join("stat"))
            .ok()
            .as_deref()
            .and_then(parent_of)
        {
            children.entry(ppid).or_default().push(pid);
        }
    }
    let mut out = Vec::new();
    let mut queue = vec![root];
    while let Some(p) = queue.pop() {
        for &c in children.get(&p).into_iter().flatten() {
            out.push(c);
            queue.push(c);
        }
    }
    out
}

/// The parent pid in `/proc/<pid>/stat`: the second field after the
/// command name, which is in parentheses and may contain anything.
#[cfg(target_os = "linux")]
fn parent_of(stat: &str) -> Option<i32> {
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace().nth(1)?.parse().ok()
}

/// `ossteamd --reaper AppId=<id> -- <program> <args>...`: becomes the
/// child subreaper, runs the program, waits until no child is left and
/// exits as the program did (`launch.md` §4).
#[cfg(target_os = "linux")]
pub fn reaper_main(args: &[std::ffi::OsString]) -> ! {
    use rustix::io::Errno;
    use rustix::process::WaitOptions;

    let Some(sep) = args.iter().position(|a| a == "--") else {
        eprintln!("reaper: usage: --reaper AppId=<id> -- <program> [<args>...]");
        std::process::exit(2);
    };
    let Some((program, rest)) = args[sep + 1..].split_first() else {
        eprintln!("reaper: no program given");
        std::process::exit(2);
    };
    if let Err(e) = rustix::process::set_child_subreaper(Some(rustix::process::getpid())) {
        eprintln!("reaper: becoming the child subreaper failed: {e}");
        std::process::exit(1);
    }
    let leader = match Command::new(program).args(rest).spawn() {
        Ok(c) => c.id() as i32,
        Err(e) => {
            eprintln!("reaper: starting {}: {e}", program.to_string_lossy());
            std::process::exit(127);
        }
    };
    let mut status = None;
    loop {
        match rustix::process::wait(WaitOptions::empty()) {
            Ok(Some((pid, st))) => {
                if pid.as_raw_nonzero().get() == leader {
                    status = Some(st);
                }
            }
            Ok(None) | Err(Errno::INTR) => {}
            Err(Errno::CHILD) => break,
            Err(e) => {
                eprintln!("reaper: waiting failed: {e}");
                std::process::exit(1);
            }
        }
    }
    let Some(status) = status else {
        std::process::exit(1);
    };
    if let Some(code) = status.exit_status() {
        std::process::exit(code);
    }
    let sig = status.terminating_signal().unwrap_or(0);
    if let Some(signal) = Signal::from_named_raw(sig) {
        // Ended by the same signal, without a core dump of the reaper. The
        // Rust runtime ignores SIGPIPE, so the default is put back first.
        // SAFETY: `signal` only changes the disposition, and no handler of
        // this program depends on the old one.
        unsafe { libc::signal(sig, libc::SIG_DFL) };
        let none = rustix::process::Rlimit {
            current: Some(0),
            maximum: Some(0),
        };
        let _ = rustix::process::setrlimit(rustix::process::Resource::Core, none);
        let _ = rustix::process::kill_process(rustix::process::getpid(), signal);
    }
    std::process::exit(128 + sig);
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "linux")]
    #[test]
    fn parent_from_stat() {
        assert_eq!(super::parent_of("42 (a) b) c) S 7 42 42 0"), Some(7));
        assert_eq!(super::parent_of("1 (init) S 0 1 1"), Some(0));
        assert_eq!(super::parent_of("garbage"), None);
    }
}
