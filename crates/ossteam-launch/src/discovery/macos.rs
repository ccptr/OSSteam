// SPDX-License-Identifier: GPL-2.0-or-later
//! The Mach service `com.valvesoftware.steam.ipctool` and
//! `<data>/steam-sdk/osx/steamclient.dylib` (`steamworks-abi.md` §1.3).

use std::ffi::{CStr, CString};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;

use mach2::bootstrap::{
    BOOTSTRAP_NAME_IN_USE, BOOTSTRAP_SERVICE_ACTIVE, bootstrap_check_in, bootstrap_look_up,
    bootstrap_port, bootstrap_strerror,
};
use mach2::kern_return::{KERN_SUCCESS, kern_return_t};
use mach2::mach_port::{mach_port_deallocate, mach_port_mod_refs};
use mach2::message::{
    MACH_MSG_SUCCESS, MACH_MSG_TYPE_MOVE_SEND, MACH_MSG_TYPE_MOVE_SEND_ONCE, MACH_MSGH_BITS,
    MACH_MSGH_BITS_COMPLEX, MACH_MSGH_BITS_REMOTE_MASK, MACH_RCV_MSG, MACH_RCV_TIMED_OUT,
    MACH_RCV_TIMEOUT, MACH_RCV_TOO_LARGE, MACH_SEND_MSG, MACH_SEND_TIMEOUT, mach_msg,
    mach_msg_destroy, mach_msg_header_t,
};
use mach2::port::{MACH_PORT_NULL, MACH_PORT_RIGHT_RECEIVE, mach_port_t};
use mach2::traps::mach_task_self;

use super::files::{make_dir, replace_copy};
use super::{Error, Libraries};

/// The service `steam_api` looks up.
const SERVICE: &str = "com.valvesoftware.steam.ipctool";
/// The protocol version, every message's `msgh_id` (§1.3.1).
const VERSION: u32 = 104;
const VERSION_REQUEST: u32 = 100_001;
const KILL_REQUEST: u32 = 100_000;
const PATH_REQUEST: u32 = 14;
/// A GetSteamPath reply's path, with its NUL.
const PATH_LEN: usize = 512;
/// How often the server thread looks at its stop flag.
const POLL_MS: u32 = 250;
const REPLY_TIMEOUT_MS: u32 = 1000;

#[repr(C)]
#[derive(Clone, Copy)]
struct Request {
    header: mach_msg_header_t,
    kind: u32,
    arg: u32,
}

#[repr(C)]
struct VersionReply {
    header: mach_msg_header_t,
    kind: u32,
    version: u32,
}

#[repr(C)]
struct PathReply {
    header: mach_msg_header_t,
    kind: u32,
    pid: u32,
    path: [u8; PATH_LEN],
}

const _: () = assert!(
    size_of::<Request>() == 32 && size_of::<VersionReply>() == 32 && size_of::<PathReply>() == 544
);

/// Room for a request and its trailer. The kernel drops anything larger.
#[repr(C, align(8))]
struct Received([u8; 256]);

pub struct Entries {
    /// `<data>/steam-sdk/osx`.
    sdk: PathBuf,
    service: CString,
    server: Mutex<Option<Server>>,
}

/// The thread that holds the service's receive right.
struct Server {
    stop: Arc<AtomicBool>,
    thread: JoinHandle<()>,
}

impl Entries {
    /// `home` is unused: the entries are not files in it. `data` is the
    /// daemon's data directory.
    pub fn new(_home: &Path, data: &Path) -> Self {
        Self::with_service(data, SERVICE)
    }

    fn with_service(data: &Path, service: &str) -> Self {
        Self {
            sdk: data.join("steam-sdk").join("osx"),
            service: CString::new(service).expect("no NUL in a service name"),
            server: Mutex::new(None),
        }
    }

    fn server(&self) -> MutexGuard<'_, Option<Server>> {
        self.server.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn conflict(&self) -> Error {
        Error::Conflict(format!(
            "the Mach service {}",
            self.service.to_string_lossy()
        ))
    }

    /// Fails with `Conflict` when someone else serves the service
    /// (DISC-3).
    pub fn check(&self) -> Result<(), Error> {
        if self.server().is_some() {
            return Ok(());
        }
        self.check_unserved()
    }

    fn check_unserved(&self) -> Result<(), Error> {
        let mut port = MACH_PORT_NULL;
        // SAFETY: a NUL-terminated name and a place for the send right.
        let kr = unsafe { bootstrap_look_up(bootstrap_port, self.service.as_ptr(), &mut port) };
        if kr != KERN_SUCCESS {
            return Ok(());
        }
        // SAFETY: the send right the look-up gave us.
        unsafe { mach_port_deallocate(mach_task_self(), port) };
        Err(self.conflict())
    }

    /// Makes or refreshes the copy of the library, and serves the service
    /// for the daemon `pid`. Nothing is touched when someone else serves
    /// it.
    pub fn create(&self, libs: &Libraries, pid: u32, _account_id: u32) -> Result<(), Error> {
        let mut server = self.server();
        if server.is_none() {
            self.check_unserved()?;
        }
        make_dir(&self.sdk, 0o700)?;
        let copy = self.sdk.join("steamclient.dylib");
        replace_copy(&libs.lib64, &copy)?;
        if server.is_some() {
            return Ok(());
        }
        let path = path_field(&copy)?;
        let mut port = MACH_PORT_NULL;
        // SAFETY: a NUL-terminated name and a place for the receive right.
        let kr = unsafe { bootstrap_check_in(bootstrap_port, self.service.as_ptr(), &mut port) };
        if kr == BOOTSTRAP_SERVICE_ACTIVE as kern_return_t
            || kr == BOOTSTRAP_NAME_IN_USE as kern_return_t
        {
            return Err(self.conflict());
        }
        if kr != KERN_SUCCESS {
            return Err(Error::io(
                format!(
                    "checking in the Mach service {}",
                    self.service.to_string_lossy()
                ),
                io::Error::other(bootstrap_error(kr)),
            ));
        }
        let stop = Arc::new(AtomicBool::new(false));
        let thread = std::thread::Builder::new().name("ipctool".into()).spawn({
            let stop = stop.clone();
            move || serve(port, &path, pid, &stop)
        });
        match thread {
            Ok(thread) => {
                *server = Some(Server { stop, thread });
                Ok(())
            }
            Err(e) => {
                destroy_receive_right(port);
                Err(Error::io("starting the discovery service", e))
            }
        }
    }

    /// Stops serving the service. The copy stays, as the Linux
    /// directories do.
    pub fn remove(&self) -> Result<(), Error> {
        let server = self.server().take();
        if let Some(s) = server {
            s.stop.store(true, Ordering::Relaxed);
            let _ = s.thread.join();
        }
        Ok(())
    }

    /// Nothing to do: the receive right goes with a crashed daemon
    /// (DISC-2).
    pub fn remove_stale(&self, _own_pid: u32) -> Result<(), Error> {
        Ok(())
    }
}

impl Drop for Entries {
    fn drop(&mut self) {
        let _ = self.remove();
    }
}

fn bootstrap_error(kr: kern_return_t) -> String {
    // SAFETY: bootstrap_strerror returns a static string for any code.
    let s = unsafe { CStr::from_ptr(bootstrap_strerror(kr)) };
    format!("{} ({kr})", s.to_string_lossy())
}

/// `path` as GetSteamPath carries it, NUL-terminated.
fn path_field(path: &Path) -> Result<[u8; PATH_LEN], Error> {
    let bytes = path.as_os_str().as_bytes();
    if bytes.len() >= PATH_LEN || bytes.contains(&0) {
        return Err(Error::io(
            format!("serving {}", path.display()),
            io::Error::other(format!(
                "steam_api reads at most {} bytes of a path",
                PATH_LEN - 1
            )),
        ));
    }
    let mut field = [0; PATH_LEN];
    field[..bytes.len()].copy_from_slice(bytes);
    Ok(field)
}

fn destroy_receive_right(port: mach_port_t) {
    // SAFETY: the caller owns the receive right `port`.
    unsafe { mach_port_mod_refs(mach_task_self(), port, MACH_PORT_RIGHT_RECEIVE, -1) };
}

/// Answers requests on `port` until `stop` is set, then destroys the
/// receive right, which takes the name away (§1.3.3).
fn serve(port: mach_port_t, path: &[u8; PATH_LEN], pid: u32, stop: &AtomicBool) {
    let mut buf = Received([0; 256]);
    while !stop.load(Ordering::Relaxed) {
        let msg = buf.0.as_mut_ptr().cast::<mach_msg_header_t>();
        // SAFETY: `buf` is writable, as large as we say and aligned for a
        // header.
        let r = unsafe {
            mach_msg(
                msg,
                MACH_RCV_MSG | MACH_RCV_TIMEOUT,
                0,
                buf.0.len() as u32,
                port,
                POLL_MS,
                MACH_PORT_NULL,
            )
        };
        match r {
            // SAFETY: a message was just received into `buf`.
            MACH_MSG_SUCCESS => unsafe { answer(msg, path, pid) },
            MACH_RCV_TIMED_OUT | MACH_RCV_TOO_LARGE => {}
            e => {
                tracing::warn!("the discovery service stopped: mach_msg returned {e:#x}");
                break;
            }
        }
    }
    destroy_receive_right(port);
}

/// Answers the message in `msg` (§1.3.3).
///
/// # Safety
///
/// `msg` holds a message just received, in a [`Received`].
unsafe fn answer(msg: *mut mach_msg_header_t, path: &[u8; PATH_LEN], pid: u32) {
    // SAFETY: the caller's promise.
    let header = unsafe { msg.read() };
    // steam_api makes a send right for its reply port (§1.3.1); a send-once
    // right is answered too. A right arrives as MACH_MSG_TYPE_PORT_SEND or
    // _PORT_SEND_ONCE, which have the values of the MOVE dispositions that
    // hand it back.
    let disposition = header.msgh_bits & MACH_MSGH_BITS_REMOTE_MASK;
    let reply_to = ((disposition == MACH_MSG_TYPE_MOVE_SEND
        || disposition == MACH_MSG_TYPE_MOVE_SEND_ONCE)
        && header.msgh_remote_port != MACH_PORT_NULL)
        .then_some((header.msgh_remote_port, disposition));
    let simple = header.msgh_bits & MACH_MSGH_BITS_COMPLEX == 0;
    if !simple || (header.msgh_size as usize) < size_of::<Request>() {
        // SAFETY: as above.
        unsafe { mach_msg_destroy(msg) };
        return;
    }
    // SAFETY: as above, and the message is at least this long.
    let request = unsafe { msg.cast::<Request>().read() };
    match (request.kind, reply_to) {
        (VERSION_REQUEST, Some(to)) => send(VersionReply {
            header: reply_header::<VersionReply>(&header, to),
            kind: request.kind,
            version: VERSION,
        }),
        (PATH_REQUEST, Some(to)) => send(PathReply {
            header: reply_header::<PathReply>(&header, to),
            kind: request.kind,
            pid,
            path: *path,
        }),
        (kind, _) => {
            if kind == KILL_REQUEST {
                tracing::info!(
                    "the steam_api of pid {} asked for the ipcserver to restart; ossteam has none",
                    request.arg
                );
            }
            // SAFETY: as above.
            unsafe { mach_msg_destroy(msg) };
        }
    }
}

/// `to` is the reply port and how to hand back the right to it.
fn reply_header<T>(request: &mach_msg_header_t, to: (mach_port_t, u32)) -> mach_msg_header_t {
    let (to, disposition) = to;
    mach_msg_header_t {
        msgh_bits: MACH_MSGH_BITS(disposition, 0),
        msgh_size: size_of::<T>() as u32,
        msgh_remote_port: to,
        msgh_local_port: MACH_PORT_NULL,
        msgh_voucher_port: MACH_PORT_NULL,
        msgh_id: request.msgh_id,
    }
}

/// Sends `reply`, a `repr(C)` struct that starts with its header.
fn send<T>(mut reply: T) {
    let msg = (&raw mut reply).cast::<mach_msg_header_t>();
    // SAFETY: `reply` is a whole message of size_of::<T>() bytes.
    let r = unsafe {
        mach_msg(
            msg,
            MACH_SEND_MSG | MACH_SEND_TIMEOUT,
            size_of::<T>() as u32,
            0,
            MACH_PORT_NULL,
            REPLY_TIMEOUT_MS,
            MACH_PORT_NULL,
        )
    };
    if r != MACH_MSG_SUCCESS {
        tracing::debug!("a discovery reply was not sent: {r:#x}");
        // SAFETY: an unsent message still holds its rights.
        unsafe { mach_msg_destroy(msg) };
    }
}

#[cfg(test)]
mod tests {
    use mach2::mach_port::mach_port_allocate;
    use mach2::message::{
        MACH_MSG_TYPE_COPY_SEND, MACH_MSG_TYPE_MAKE_SEND, MACH_MSG_TYPE_MAKE_SEND_ONCE,
    };

    use super::*;

    struct Dirs {
        root: PathBuf,
        data: PathBuf,
        libs: Libraries,
        service: String,
    }

    impl Drop for Dirs {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn dirs(name: &str) -> Dirs {
        let root = std::env::temp_dir().join(format!("ossteam-disc-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join(super::super::LIBRARY), b"universal").unwrap();
        let libs = super::super::find_libraries(&bin).unwrap();
        assert_eq!(libs.lib32, None);
        Dirs {
            data: root.join("data"),
            root,
            libs,
            service: format!("dev.ossteam.test.{}.{name}", std::process::id()),
        }
    }

    /// The bits `steam_api` sends: copy the server's send right, make a
    /// send right for the reply port (§1.3.1).
    const STEAM_API_BITS: u32 = 0x1413;

    #[test]
    fn steam_api_makes_a_send_right_for_its_reply_port() {
        assert_eq!(
            MACH_MSGH_BITS(MACH_MSG_TYPE_COPY_SEND, MACH_MSG_TYPE_MAKE_SEND),
            STEAM_API_BITS
        );
    }

    /// What `steam_api` does (§1.3.2): looks the service up and sends
    /// `kind` with the client's version, then receives into a buffer of
    /// `T` and the trailer. `None` when there is no service or no reply.
    fn ask<T>(service: &str, kind: u32) -> Option<T> {
        ask_with::<T>(service, kind, STEAM_API_BITS)
    }

    fn ask_with<T>(service: &str, kind: u32, bits: u32) -> Option<T> {
        let name = CString::new(service).unwrap();
        let mut server = MACH_PORT_NULL;
        let mut reply_port = MACH_PORT_NULL;
        // SAFETY: plain Mach calls on our own rights and buffers.
        unsafe {
            let task = mach_task_self();
            if bootstrap_look_up(bootstrap_port, name.as_ptr(), &mut server) != KERN_SUCCESS {
                return None;
            }
            assert_eq!(
                mach_port_allocate(task, MACH_PORT_RIGHT_RECEIVE, &mut reply_port),
                KERN_SUCCESS
            );
            let mut request = Request {
                header: mach_msg_header_t {
                    msgh_bits: bits,
                    msgh_size: 32,
                    msgh_remote_port: server,
                    msgh_local_port: reply_port,
                    msgh_voucher_port: MACH_PORT_NULL,
                    msgh_id: VERSION as i32,
                },
                kind,
                arg: VERSION,
            };
            let sent = mach_msg(
                &mut request.header,
                MACH_SEND_MSG | MACH_SEND_TIMEOUT,
                32,
                0,
                MACH_PORT_NULL,
                2000,
                MACH_PORT_NULL,
            );
            assert_eq!(sent, MACH_MSG_SUCCESS);
            #[repr(C, align(8))]
            struct Buf<T>(T, [u8; 8]);
            let mut buf = std::mem::MaybeUninit::<Buf<T>>::zeroed();
            let got = mach_msg(
                buf.as_mut_ptr().cast(),
                MACH_RCV_MSG | MACH_RCV_TIMEOUT,
                0,
                size_of::<Buf<T>>() as u32,
                reply_port,
                1000,
                MACH_PORT_NULL,
            );
            destroy_receive_right(reply_port);
            mach_port_deallocate(task, server);
            if got != MACH_MSG_SUCCESS {
                return None;
            }
            let header = buf.as_ptr().cast::<mach_msg_header_t>().read();
            assert_eq!(header.msgh_id, VERSION as i32);
            assert_eq!(header.msgh_size as usize, size_of::<T>());
            Some(buf.assume_init().0)
        }
    }

    fn path_of(reply: &PathReply) -> PathBuf {
        let end = reply.path.iter().position(|&b| b == 0).unwrap();
        PathBuf::from(std::ffi::OsStr::from_bytes(&reply.path[..end]))
    }

    #[test]
    fn serves_steam_api() {
        let d = dirs("serve");
        let e = Entries::with_service(&d.data, &d.service);
        assert!(ask::<VersionReply>(&d.service, VERSION_REQUEST).is_none());
        e.check().unwrap();
        e.create(&d.libs, 1234, 0).unwrap();
        e.check().unwrap();

        let v = ask::<VersionReply>(&d.service, VERSION_REQUEST).unwrap();
        assert_eq!((v.kind, v.version), (VERSION_REQUEST, VERSION));
        let p = ask::<PathReply>(&d.service, PATH_REQUEST).unwrap();
        assert_eq!((p.kind, p.pid), (PATH_REQUEST, 1234));
        let copy = path_of(&p);
        assert_eq!(copy, d.data.join("steam-sdk/osx/steamclient.dylib"));
        assert_eq!(std::fs::read(&copy).unwrap(), b"universal");
        // A client that asks for a send-once reply right is answered too.
        let once = MACH_MSGH_BITS(MACH_MSG_TYPE_COPY_SEND, MACH_MSG_TYPE_MAKE_SEND_ONCE);
        let p = ask_with::<PathReply>(&d.service, PATH_REQUEST, once).unwrap();
        assert_eq!((p.kind, p.pid), (PATH_REQUEST, 1234));
        // Kill gets no reply, and the service goes on.
        assert!(ask::<VersionReply>(&d.service, KILL_REQUEST).is_none());
        assert!(ask::<PathReply>(&d.service, PATH_REQUEST).is_some());

        // Again: the copy is refreshed, the service stays.
        std::fs::write(&d.libs.lib64, b"universal v2").unwrap();
        e.create(&d.libs, 1234, 0).unwrap();
        assert_eq!(std::fs::read(&copy).unwrap(), b"universal v2");

        e.remove().unwrap();
        assert!(ask::<PathReply>(&d.service, PATH_REQUEST).is_none());
        e.check().unwrap();
        // And back, as after a logout and a login.
        e.create(&d.libs, 1234, 0).unwrap();
        assert!(ask::<PathReply>(&d.service, PATH_REQUEST).is_some());
        e.remove().unwrap();
    }

    #[test]
    fn a_service_someone_else_serves_is_left_alone() {
        let d = dirs("official");
        let official = Entries::with_service(&d.root.join("official"), &d.service);
        official.create(&d.libs, 99, 0).unwrap();
        let e = Entries::with_service(&d.data, &d.service);
        let name = format!("the Mach service {}", d.service);
        assert!(matches!(e.check(), Err(Error::Conflict(m)) if m == name));
        assert!(matches!(e.create(&d.libs, 1, 0), Err(Error::Conflict(_))));
        e.remove().unwrap();
        e.remove_stale(1).unwrap();
        let p = ask::<PathReply>(&d.service, PATH_REQUEST).unwrap();
        assert_eq!(p.pid, 99);
        assert!(!d.data.join("steam-sdk/osx").exists());
    }

    #[test]
    fn a_path_steam_api_cannot_read_is_refused() {
        assert!(path_field(Path::new(&"/a".repeat(255))).is_ok());
        assert!(path_field(Path::new(&"/a".repeat(256))).is_err());
    }
}
