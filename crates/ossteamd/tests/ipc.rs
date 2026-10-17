// SPDX-License-Identifier: GPL-2.0-or-later
//! Daemon integration tests: a real `ossteamd` driven through `ossteam-ipc`
//! (SPEC §19.1 level 3).
//!
//! Unix only for now: the daemon is isolated by pointing `XDG_RUNTIME_DIR`
//! (Linux) or `TMPDIR` (macOS) and `HOME` at a temporary directory. The
//! Windows endpoint is derived from the user's SID and cannot be isolated
//! that way yet.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use ossteam_ipc::frame::{read_frame, write_frame};
use ossteam_ipc::v1::{self, ErrorCode, PeerKind, Topic, envelope::Body, event, request, response};
use ossteam_ipc::{Client, ClientError, ClientInfo, ConnectOptions, Endpoint};

struct Daemon {
    child: Child,
    root: PathBuf,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn env_for(root: &Path) -> Vec<(&'static str, PathBuf)> {
    vec![
        ("XDG_RUNTIME_DIR", root.join("run")),
        ("TMPDIR", root.join("run")),
        ("HOME", root.join("home")),
        ("XDG_STATE_HOME", root.join("state")),
        ("XDG_CONFIG_HOME", root.join("config")),
        ("XDG_DATA_HOME", root.join("data")),
        ("XDG_CACHE_HOME", root.join("cache")),
    ]
}

fn daemon_command(root: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ossteamd"));
    cmd.envs(env_for(root))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    cmd
}

/// Starts a daemon in a fresh directory and points this process's endpoint
/// lookup at it. All tests share one process, so they run one at a time
/// through `TEST_LOCK`.
fn start() -> Daemon {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    // Resolved once: later tests change TMPDIR, which temp_dir() reads.
    static BASE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    let base = BASE.get_or_init(std::env::temp_dir);
    let root = base.join(format!("ossteamd-it-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for (_, dir) in env_for(&root) {
        std::fs::create_dir_all(&dir).unwrap();
    }
    for (key, value) in env_for(&root) {
        // SAFETY: tests in this file hold TEST_LOCK, and no other thread
        // reads the environment while it is changed.
        unsafe { std::env::set_var(key, value) };
    }
    let child = daemon_command(&root).spawn().expect("spawn ossteamd");
    let daemon = Daemon { child, root };
    let deadline = Instant::now() + Duration::from_secs(10);
    while Endpoint::client().unwrap().connect().is_err() {
        assert!(Instant::now() < deadline, "daemon did not start listening");
        std::thread::sleep(Duration::from_millis(20));
    }
    daemon
}

static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn client() -> Client {
    let info = ClientInfo {
        kind: PeerKind::Other,
        name: "it".into(),
        version: "0".into(),
    };
    Client::connect(&ConnectOptions {
        autostart: false,
        ..ConnectOptions::new(info)
    })
    .unwrap()
}

fn raw_hello(major: u32) -> v1::Envelope {
    v1::Envelope {
        id: 0,
        body: Some(Body::Hello(v1::Hello {
            protocol: Some(v1::ProtocolVersion { major, minor: 0 }),
            kind: PeerKind::Other as i32,
            name: "raw".into(),
            version: "0".into(),
        })),
    }
}

#[test]
fn status_version_and_subscriptions() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let daemon = start();
    let c = client();
    assert_eq!(c.server_hello().kind, PeerKind::Daemon as i32);

    let status = c.get_status().unwrap();
    assert_eq!(status.pid, daemon.child.id());
    assert_eq!(status.connection, v1::ConnectionState::Disconnected as i32);
    assert!(status.account.is_none());
    assert!(status.connected_clients >= 1);

    let version = c.get_version().unwrap();
    assert_eq!(version.protocol, Some(ossteam_ipc::PROTOCOL_VERSION));
    assert_eq!(version.os, std::env::consts::OS);

    assert_eq!(
        c.subscribe(&[Topic::Daemon, Topic::Notices]).unwrap(),
        [Topic::Daemon, Topic::Notices]
    );
    assert_eq!(c.unsubscribe(&[Topic::Notices]).unwrap(), [Topic::Daemon]);

    let bad = c.call(
        request::Method::Subscribe(v1::SubscribeRequest {
            topics: vec![Topic::Unspecified as i32],
        }),
        Duration::from_secs(5),
    );
    assert!(matches!(bad, Err(ClientError::Remote(e)) if e.code == ErrorCode::InvalidArgument));
}

#[test]
fn unknown_method_is_unsupported_and_connection_survives() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _daemon = start();
    let mut s = Endpoint::client().unwrap().connect().unwrap();
    write_frame(&mut s, &raw_hello(1)).unwrap();
    assert!(matches!(
        read_frame(&mut s).unwrap().body,
        Some(Body::Hello(_))
    ));

    // A request whose method this daemon does not know decodes as None.
    let unknown = v1::Envelope {
        id: 9,
        body: Some(Body::Request(v1::Request { method: None })),
    };
    write_frame(&mut s, &unknown).unwrap();
    let reply = read_frame(&mut s).unwrap();
    assert_eq!(reply.id, 9);
    assert!(matches!(reply.body, Some(Body::Error(e)) if e.code == ErrorCode::Unsupported as i32));

    // Id 0 is reserved for events.
    let zero = v1::Envelope {
        id: 0,
        body: Some(Body::Request(v1::Request {
            method: Some(request::Method::GetStatus(v1::GetStatusRequest {})),
        })),
    };
    write_frame(&mut s, &zero).unwrap();
    let reply = read_frame(&mut s).unwrap();
    assert!(matches!(reply.body, Some(Body::Error(e)) if e.code == ErrorCode::Protocol as i32));

    let status = v1::Envelope {
        id: 10,
        body: Some(Body::Request(v1::Request {
            method: Some(request::Method::GetStatus(v1::GetStatusRequest {})),
        })),
    };
    write_frame(&mut s, &status).unwrap();
    let reply = read_frame(&mut s).unwrap();
    assert_eq!(reply.id, 10);
    assert!(matches!(
        reply.body,
        Some(Body::Response(v1::Response {
            result: Some(response::Result::GetStatus(_))
        }))
    ));
}

#[test]
fn incompatible_major_version_is_rejected() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _daemon = start();
    let mut s = Endpoint::client().unwrap().connect().unwrap();
    write_frame(&mut s, &raw_hello(2)).unwrap();
    let reply = read_frame(&mut s).unwrap();
    match reply.body {
        Some(Body::Error(e)) => {
            assert_eq!(e.code, ErrorCode::IncompatibleVersion as i32);
            assert!(e.detail.contains("1.0"), "{}", e.detail);
        }
        other => panic!("expected an error, got {other:?}"),
    }
}

#[test]
fn shutdown_notifies_subscribers_and_exits() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut daemon = start();
    let watcher = client();
    watcher.subscribe(&[Topic::Daemon]).unwrap();

    // A second instance sees the healthy first one and exits with 0.
    let second = daemon_command(&daemon.root).status().unwrap();
    assert!(second.success());

    client().shutdown(false).unwrap();
    let ev = watcher
        .events()
        .recv_timeout(Duration::from_secs(5))
        .unwrap();
    assert!(matches!(ev.kind, Some(event::Kind::DaemonStopping(_))));
    assert!(watcher.wait_closed(Duration::from_secs(10)));

    let status = daemon.child.wait().unwrap();
    assert!(status.success());
    let ep = Endpoint::client().unwrap();
    assert!(
        !std::path::Path::new(&ep.address).exists(),
        "socket file left behind"
    );
}

fn remote_code<T: std::fmt::Debug>(r: Result<T, ClientError>) -> ErrorCode {
    match r {
        Err(ClientError::Remote(e)) => e.code,
        other => panic!("expected a remote error, got {other:?}"),
    }
}

/// The session methods that need neither the network nor a keyring: with
/// no account and no login, each fails truthfully.
#[test]
fn session_methods_without_an_account() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _daemon = start();
    let c = client();

    let status = c.get_status().unwrap();
    assert!(!status.logged_on);
    assert_eq!(status.connection, v1::ConnectionState::Disconnected as i32);

    let accounts = c.list_accounts().unwrap();
    assert!(accounts.accounts.is_empty());
    assert_eq!(accounts.active, 0);

    assert_eq!(remote_code(c.logout()), ErrorCode::NotLoggedIn);
    assert_eq!(
        remote_code(c.switch_account(76561197960287930)),
        ErrorCode::NotFound
    );
    let code = ossteam_types::Secret::new("ABCDE".into());
    assert_eq!(
        remote_code(c.submit_guard_code(&code, v1::LoginPromptKind::Unspecified)),
        ErrorCode::Conflict
    );
    let empty = ossteam_types::Secret::new(" ".into());
    assert_eq!(
        remote_code(c.submit_guard_code(&empty, v1::LoginPromptKind::EmailCode)),
        ErrorCode::InvalidArgument
    );
    let password = ossteam_types::Secret::new("pw".into());
    assert_eq!(
        remote_code(c.login_with_credentials("  ", &password, false)),
        ErrorCode::InvalidArgument
    );
    // Cancelling when nothing is in progress succeeds.
    c.cancel_login().unwrap();
}
