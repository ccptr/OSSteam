// SPDX-License-Identifier: GPL-2.0-or-later
//! The private `~/.steam` on Linux (`steamworks-abi.md` §1.1.2). The
//! steps run in the child, after `fork` and before `exec`, where only
//! system calls are allowed: everything they need is prepared before the
//! fork, and a failed step is reported to the parent through a pipe.

use std::ffi::{CStr, CString};
use std::io;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use rustix::fs::{Mode, OFlags, StatVfsMountFlags as St};
use rustix::io::Errno;
use rustix::mount::{MountFlags as Ms, MountPropagationFlags};
use rustix::pipe::PipeFlags;
use rustix::thread::UnshareFlags;

use super::{Failure, Step};

/// The official installations' places in the home directory, besides the
/// targets of `~/.steam/root` and `~/.steam/steam` (§1.1.2).
const OFFICIAL_ROOTS: [&str; 3] = [
    ".local/share/Steam",
    ".var/app/com.valvesoftware.Steam/.local/share/Steam",
    "snap/steam/common/.local/share/Steam",
];

/// One bind of steps 4 to 6.
#[derive(Debug)]
struct Bind {
    step: Step,
    source: CString,
    target: CString,
    /// The mount point, for messages.
    path: PathBuf,
    read_only: bool,
}

impl Bind {
    fn new(step: Step, source: &Path, target: &Path, read_only: bool) -> Result<Self, Failure> {
        let c = |p: &Path| {
            CString::new(p.as_os_str().as_bytes()).map_err(|_| Failure {
                step: Step::Prepare,
                path: Some(p.to_owned()),
                source: io::Error::new(io::ErrorKind::InvalidInput, "the path holds a NUL byte"),
            })
        };
        Ok(Self {
            step,
            source: c(source)?,
            target: c(target)?,
            path: target.to_owned(),
            read_only,
        })
    }
}

/// What a launch's child does: everything worked out before the fork.
#[derive(Debug)]
pub struct Plan {
    uid_map: Vec<u8>,
    gid_map: Vec<u8>,
    binds: Vec<Bind>,
}

impl Plan {
    /// The plan for a launch: `~/.steam` under `home`, ossteam's
    /// directories under `data` (`<data>/steam-sdk`, `<data>/userdata`,
    /// `<data>/empty`), and the official roots found now. Creates
    /// `~/.steam` and ossteam's directories when they are missing.
    pub fn new(home: &Path, data: &Path) -> Result<Self, Failure> {
        let steam = home.join(".steam");
        prepare_steam(&steam)?;
        let sdk = data.join("steam-sdk");
        let userdata = data.join("userdata");
        let empty = data.join("empty");
        for d in [&sdk, &userdata, &empty] {
            make_dir(d, 0o700)?;
        }
        let mut binds = vec![Bind::new(Step::Steam, &sdk, &steam, true)?];
        let roots = official_roots(home);
        for root in &roots {
            let target = root.join("userdata");
            if target.is_dir() {
                binds.push(Bind::new(Step::Userdata, &userdata, &target, false)?);
            }
        }
        for root in &roots {
            let target = root.join("config");
            if target.is_dir() {
                binds.push(Bind::new(Step::Config, &empty, &target, true)?);
            }
        }
        Ok(Self::with_binds(binds))
    }

    fn with_binds(binds: Vec<Bind>) -> Self {
        let uid = rustix::process::getuid().as_raw();
        let gid = rustix::process::getgid().as_raw();
        Self {
            uid_map: format!("{uid} {uid} 1").into_bytes(),
            gid_map: format!("{gid} {gid} 1").into_bytes(),
            binds,
        }
    }

    /// The mount points, in the order they are mounted.
    pub fn mount_points(&self) -> impl Iterator<Item = &Path> {
        self.binds.iter().map(|b| b.path.as_path())
    }

    /// Steps 1 to 6 in the calling process, which must be single-threaded:
    /// a forked child. System calls only, and no allocation. A failed
    /// step is written to `report` as six bytes: the step, the bind's
    /// index, and the error number.
    fn apply(&self, report: BorrowedFd<'_>) -> io::Result<()> {
        self.steps().map_err(|(step, index, e)| {
            let code = e.raw_os_error();
            let mut msg = [0u8; 6];
            msg[0] = step as u8;
            msg[1] = index;
            msg[2..].copy_from_slice(&code.to_le_bytes());
            let _ = rustix::io::write(report, &msg);
            io::Error::from_raw_os_error(code)
        })
    }

    fn steps(&self) -> Result<(), (Step, u8, Errno)> {
        let at = |step| move |e| (step, 0, e);
        // SAFETY: the file table is not unshared, so no descriptor is lost
        // to another thread; the caller is single-threaded besides.
        unsafe { rustix::thread::unshare_unsafe(UnshareFlags::NEWUSER | UnshareFlags::NEWNS) }
            .map_err(at(Step::Unshare))?;
        write_file(c"/proc/self/setgroups", b"deny").map_err(at(Step::Setgroups))?;
        write_file(c"/proc/self/uid_map", &self.uid_map).map_err(at(Step::UidMap))?;
        write_file(c"/proc/self/gid_map", &self.gid_map).map_err(at(Step::GidMap))?;
        rustix::mount::mount_change(
            c"/",
            MountPropagationFlags::REC | MountPropagationFlags::PRIVATE,
        )
        .map_err(at(Step::Private))?;
        for (i, b) in self.binds.iter().enumerate() {
            let index = u8::try_from(i).unwrap_or(u8::MAX);
            let err = |e| (b.step, index, e);
            rustix::mount::mount_bind(b.source.as_c_str(), b.target.as_c_str()).map_err(err)?;
            if b.read_only {
                let flags = rustix::fs::statvfs(b.target.as_c_str())
                    .map_err(err)?
                    .f_flag;
                rustix::mount::mount_remount(
                    b.target.as_c_str(),
                    Ms::BIND | Ms::RDONLY | kept_flags(flags),
                    c"",
                )
                .map_err(err)?;
            }
        }
        Ok(())
    }

    /// The failure a child reported, if it reported one.
    fn read_failure(&self, rx: &OwnedFd) -> Option<Failure> {
        let mut buf = [0u8; 6];
        if rustix::io::read(rx, &mut buf).ok()? != buf.len() {
            return None;
        }
        let step = Step::from_u8(buf[0])?;
        let code = i32::from_le_bytes([buf[2], buf[3], buf[4], buf[5]]);
        let path = matches!(step, Step::Steam | Step::Userdata | Step::Config)
            .then(|| self.binds.get(usize::from(buf[1])).map(|b| b.path.clone()))
            .flatten();
        Some(Failure {
            step,
            path,
            source: io::Error::from_raw_os_error(code),
        })
    }
}

/// The flags of the mount a bind comes from, which a user namespace may
/// not clear when it remounts the bind (`mount(2)`, `EPERM`). A mount
/// with no atime flag is `strictatime`, which a remount would otherwise
/// turn into `relatime`.
fn kept_flags(f: St) -> Ms {
    let mut kept = Ms::empty();
    for (st, ms) in [
        (St::NOSUID, Ms::NOSUID),
        (St::NODEV, Ms::NODEV),
        (St::NOEXEC, Ms::NOEXEC),
        (St::NOATIME, Ms::NOATIME),
        (St::NODIRATIME, Ms::NODIRATIME),
        (St::RELATIME, Ms::RELATIME),
    ] {
        if f.contains(st) {
            kept |= ms;
        }
    }
    if !f.intersects(St::NOATIME | St::RELATIME) {
        kept |= Ms::STRICTATIME;
    }
    kept
}

fn write_file(path: &CStr, bytes: &[u8]) -> Result<(), Errno> {
    let fd = rustix::fs::open(path, OFlags::WRONLY | OFlags::CLOEXEC, Mode::empty())?;
    if rustix::io::write(&fd, bytes)? != bytes.len() {
        return Err(Errno::IO);
    }
    Ok(())
}

fn prepare_err(path: Option<&Path>, source: io::Error) -> Failure {
    Failure {
        step: Step::Prepare,
        path: path.map(Path::to_owned),
        source,
    }
}

fn make_dir(dir: &Path, mode: u32) -> Result<(), Failure> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(mode)
        .create(dir)
        .map_err(|e| prepare_err(Some(dir), e))
}

/// `~/.steam` must be a directory, or a link to one, to be mounted over;
/// it is created with mode 0755 when it does not exist (§1.1.2).
fn prepare_steam(steam: &Path) -> Result<(), Failure> {
    match std::fs::metadata(steam) {
        Ok(m) if m.is_dir() => Ok(()),
        Ok(_) => Err(prepare_err(
            Some(steam),
            io::Error::new(io::ErrorKind::NotADirectory, "it is not a directory"),
        )),
        Err(e) if e.kind() == io::ErrorKind::NotFound => make_dir(steam, 0o755),
        Err(e) => Err(prepare_err(Some(steam), e)),
    }
}

/// The official installations whose `userdata/` and `config/` steps 5 and
/// 6 cover, as canonical paths, each once (§1.1.2). One under `~/.steam`
/// is left out: step 4 hides it already.
pub fn official_roots(home: &Path) -> Vec<PathBuf> {
    let steam = home.join(".steam");
    let steam_real = std::fs::canonicalize(&steam).ok();
    let links = ["root", "steam"]
        .map(|l| steam.join(l))
        .into_iter()
        .filter(|p| p.symlink_metadata().is_ok_and(|m| m.is_symlink()));
    let mut roots: Vec<PathBuf> = Vec::new();
    for candidate in links.chain(OFFICIAL_ROOTS.map(|p| home.join(p))) {
        let Ok(root) = std::fs::canonicalize(&candidate) else {
            continue;
        };
        let hidden =
            root.starts_with(&steam) || steam_real.as_ref().is_some_and(|s| root.starts_with(s));
        if root.is_dir() && !hidden && !roots.contains(&root) {
            roots.push(root);
        }
    }
    roots
}

/// A pipe for a child's report: the read end, then the write end.
fn report_pipe() -> Result<(OwnedFd, OwnedFd), Failure> {
    rustix::pipe::pipe_with(PipeFlags::CLOEXEC | PipeFlags::NONBLOCK)
        .map_err(|e| prepare_err(None, e.into()))
}

/// The check at the daemon's start (§1.1.2): a forked child does steps 1
/// to 4 with `<data>/empty` as the mount point, and exits.
pub fn check(data: &Path) -> Result<(), Failure> {
    let sdk = data.join("steam-sdk");
    let empty = data.join("empty");
    make_dir(&sdk, 0o700)?;
    make_dir(&empty, 0o700)?;
    let plan = Plan::with_binds(vec![Bind::new(Step::Steam, &sdk, &empty, true)?]);
    let (rx, tx) = report_pipe()?;
    // SAFETY: the child makes system calls only and leaves with `_exit`,
    // as the child of a multi-threaded process must.
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(prepare_err(None, io::Error::last_os_error()));
    }
    if pid == 0 {
        let code = i32::from(plan.apply(tx.as_fd()).is_err());
        // SAFETY: ends the child without running anything of the parent's.
        unsafe { libc::_exit(code) }
    }
    drop(tx);
    let status = loop {
        match rustix::process::waitpid(
            rustix::process::Pid::from_raw(pid),
            rustix::process::WaitOptions::empty(),
        ) {
            Err(Errno::INTR) => {}
            r => break r,
        }
    };
    if let Some(f) = plan.read_failure(&rx) {
        return Err(f);
    }
    match status {
        Ok(Some((_, st))) if st.exit_status() == Some(0) => Ok(()),
        Ok(st) => Err(prepare_err(
            None,
            io::Error::other(format!("the check's child ended with {st:?}")),
        )),
        Err(e) => Err(prepare_err(None, e.into())),
    }
}

/// The pipe that carries a launch's failed step back.
pub(crate) struct Attached {
    plan: Arc<Plan>,
    rx: OwnedFd,
    tx: OwnedFd,
}

impl Attached {
    /// After `spawn` failed: the step that failed, when it was one.
    pub(crate) fn failure(self) -> Option<Failure> {
        drop(self.tx);
        self.plan.read_failure(&self.rx)
    }
}

/// Makes `cmd`'s child do the steps of `plan` before it runs the program.
pub(crate) fn attach(cmd: &mut Command, plan: &Arc<Plan>) -> Result<Attached, Failure> {
    let (rx, tx) = report_pipe()?;
    let raw = tx.as_raw_fd();
    let p = Arc::clone(plan);
    // SAFETY: `apply` makes system calls only and does not allocate, as
    // the code between `fork` and `exec` must not; `raw` stays open in the
    // child, since `Attached` keeps it open until `spawn` has returned.
    unsafe {
        cmd.pre_exec(move || p.apply(BorrowedFd::borrow_raw(raw)));
    }
    Ok(Attached {
        plan: Arc::clone(plan),
        rx,
        tx,
    })
}

/// What can be read about why the system refuses user namespaces
/// (§1.1.2), each as one phrase.
pub fn restrictions() -> Vec<String> {
    let read = |p: &str| std::fs::read_to_string(p).ok().map(|s| s.trim().to_owned());
    let mut out = Vec::new();
    if read("/proc/sys/kernel/unprivileged_userns_clone").as_deref() == Some("0") {
        out.push("kernel.unprivileged_userns_clone is 0".to_owned());
    }
    if read("/proc/sys/user/max_user_namespaces").as_deref() == Some("0") {
        out.push("user.max_user_namespaces is 0".to_owned());
    }
    if read("/proc/sys/kernel/apparmor_restrict_unprivileged_userns").as_deref() == Some("1") {
        let label =
            read("/proc/self/attr/apparmor/current").or_else(|| read("/proc/self/attr/current"));
        let confined = label.is_some_and(|l| !l.is_empty() && !l.starts_with("unconfined"));
        if !confined {
            out.push(
                "AppArmor restricts unprivileged user namespaces \
                 (kernel.apparmor_restrict_unprivileged_userns is 1) and ossteamd has no \
                 AppArmor profile"
                    .to_owned(),
            );
        }
    }
    if Path::new("/.flatpak-info").exists() {
        out.push(
            "ossteamd runs inside Flatpak, whose sandbox refuses new user namespaces".to_owned(),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use super::*;

    struct Dir(PathBuf);

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn dir(name: &str) -> Dir {
        let d = std::env::temp_dir().join(format!("ossteam-iso-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Dir(d.canonicalize().unwrap())
    }

    #[test]
    fn official_roots_are_found_once() {
        let d = dir("roots");
        let home = d.0.join("home");
        let local = home.join(".local/share/Steam");
        let flatpak = home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam");
        let debian = home.join(".steam/debian-installation");
        for p in [&local, &flatpak, &debian] {
            std::fs::create_dir_all(p).unwrap();
        }
        symlink(&local, home.join(".steam/root")).unwrap();
        symlink(&debian, home.join(".steam/steam")).unwrap();
        // A dangling Snap link is no root.
        std::fs::create_dir_all(home.join("snap/steam/common/.local/share")).unwrap();
        symlink(
            d.0.join("gone"),
            home.join("snap/steam/common/.local/share/Steam"),
        )
        .unwrap();
        assert_eq!(official_roots(&home), vec![local, flatpak]);
    }

    #[test]
    fn a_file_at_steam_is_refused() {
        let d = dir("file");
        std::fs::write(d.0.join(".steam"), b"").unwrap();
        let f = Plan::new(&d.0, &d.0.join("data")).unwrap_err();
        assert_eq!(f.step, Step::Prepare);
        assert_eq!(f.path.as_deref(), Some(d.0.join(".steam").as_path()));
    }

    #[test]
    fn the_plan_covers_userdata_and_config() {
        let d = dir("plan");
        let home = d.0.join("home");
        let data = d.0.join("data");
        let root = home.join(".local/share/Steam");
        std::fs::create_dir_all(root.join("userdata")).unwrap();
        std::fs::create_dir_all(root.join("config")).unwrap();
        let plan = Plan::new(&home, &data).unwrap();
        let points: Vec<&Path> = plan.mount_points().collect();
        assert_eq!(
            points,
            [
                home.join(".steam").as_path(),
                &root.join("userdata"),
                &root.join("config")
            ]
        );
        assert!(home.join(".steam").is_dir());
        for d in ["steam-sdk", "userdata", "empty"] {
            assert!(data.join(d).is_dir(), "{d}");
        }
    }

    #[test]
    fn kept_flags_keep_the_locked_ones() {
        assert_eq!(
            kept_flags(St::NOSUID | St::NODEV | St::RELATIME | St::RDONLY),
            Ms::NOSUID | Ms::NODEV | Ms::RELATIME
        );
        assert_eq!(kept_flags(St::empty()), Ms::STRICTATIME);
    }

    /// Runs `sh -c script` in a namespace made by `plan`, and returns its
    /// standard output; `None` when this system allows no user namespace.
    fn run_in(plan: Plan, script: &str) -> Option<String> {
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c").arg(script);
        let attached = attach(&mut cmd, &Arc::new(plan)).unwrap();
        match cmd.output() {
            Ok(out) => {
                assert!(out.status.success(), "{out:?}");
                Some(String::from_utf8(out.stdout).unwrap())
            }
            Err(e) => {
                let f = attached.failure();
                eprintln!("no user namespace here: {f:?} ({e})");
                None
            }
        }
    }

    #[test]
    fn a_launch_sees_the_private_steam() {
        let d = dir("launch");
        let home = d.0.join("home");
        let data = d.0.join("data");
        if let Err(e) = check(&data) {
            eprintln!("skipped: {e}");
            return;
        }
        let steam = home.join(".steam");
        let root = home.join(".local/share/Steam");
        std::fs::create_dir_all(root.join("userdata/42")).unwrap();
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::write(root.join("config/loginusers.vdf"), b"secret").unwrap();
        std::fs::create_dir_all(&steam).unwrap();
        symlink(&root, steam.join("root")).unwrap();
        std::fs::write(steam.join("steam.pid"), b"1").unwrap();
        std::fs::create_dir_all(data.join("steam-sdk/sdk64")).unwrap();
        std::fs::write(data.join("steam-sdk/steam.pid"), b"7").unwrap();
        std::fs::create_dir_all(data.join("userdata/9")).unwrap();

        let plan = Plan::new(&home, &data).unwrap();
        let s = steam.display();
        let r = root.display();
        let script = format!(
            "cat {s}/steam.pid; echo; ls {s}; ls {r}/userdata; ls -A {r}/config; \
             touch {s}/x 2>/dev/null || echo steam-ro; \
             touch {r}/config/x 2>/dev/null || echo config-ro; \
             touch {r}/userdata/new && echo userdata-rw; id -u"
        );
        let Some(out) = run_in(plan, &script) else {
            return;
        };
        let uid = rustix::process::getuid().as_raw();
        assert_eq!(
            out,
            format!("7\nsdk64\nsteam.pid\n9\nsteam-ro\nconfig-ro\nuserdata-rw\n{uid}\n")
        );
        // The host is untouched; ossteam's userdata got the write.
        assert_eq!(
            std::fs::read_to_string(steam.join("steam.pid")).unwrap(),
            "1"
        );
        assert!(root.join("config/loginusers.vdf").is_file());
        assert!(data.join("userdata/new").exists());
        assert!(!root.join("userdata/new").exists());
    }

    #[test]
    fn a_failed_step_is_reported() {
        let d = dir("fail");
        let data = d.0.join("data");
        if check(&data).is_err() {
            return;
        }
        // A mount point that is gone by the time the child mounts.
        let gone = d.0.join("gone");
        std::fs::create_dir_all(&gone).unwrap();
        let plan = Plan::with_binds(vec![
            Bind::new(Step::Userdata, &data.join("empty"), &gone, false).unwrap(),
        ]);
        std::fs::remove_dir(&gone).unwrap();
        let mut cmd = Command::new("/bin/true");
        let attached = attach(&mut cmd, &Arc::new(plan)).unwrap();
        assert!(cmd.spawn().is_err());
        let f = attached.failure().unwrap();
        assert_eq!(f.step, Step::Userdata);
        assert_eq!(f.path.as_deref(), Some(gone.as_path()));
        assert_eq!(f.source.raw_os_error(), Some(libc::ENOENT));
    }
}
