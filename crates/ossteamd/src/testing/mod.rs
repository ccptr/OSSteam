// SPDX-License-Identifier: GPL-2.0-or-later
//! Daemon integration tests against a fake CM (SPEC §19.1 level 3).
//!
//! The daemon runs inside the test process: the same core, workers and IPC
//! server as `ossteamd`, started through [`serve`](crate::serve), with two
//! parts swapped. The session worker reaches a [`FakeCm`] instead of the
//! Steam Directory, and tokens go to a file in the test directory instead
//! of the OS keyring. Tests drive it through `ossteam-ipc`, as leaf
//! clients do. All of this is compiled for tests only (FAITH-4).

mod fake_cm;
mod loopback;
mod m1a;
mod m1b;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ossteam_ipc::v1::{Event, PeerKind, Topic, event};
use ossteam_ipc::{Client, ClientInfo, Endpoint};

pub use fake_cm::{Account, FakeCm, Guard};
pub use ossteam_content::fake::FakeCdn;

use crate::secrets::{FALLBACK_FILE, Secrets};
use crate::{Env, paths, serve};

/// How long a test waits for anything the daemon does.
pub const WAIT: Duration = Duration::from_secs(20);

/// A directory for one test, removed when dropped. Daemons started on the
/// same root share their state, as a restarted daemon does.
pub struct Root(PathBuf);

impl Root {
    pub fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        // Short: Unix socket paths are limited to about 100 bytes.
        let root = std::env::temp_dir().join(format!("ossd-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create the test directory");
        Self(root)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    /// The token file the test daemon uses in place of the OS keyring.
    pub fn secrets_file(&self) -> PathBuf {
        self.0.join("data").join(FALLBACK_FILE)
    }

    fn endpoint(&self) -> Endpoint {
        #[cfg(unix)]
        {
            let dir = self.0.join("run");
            Endpoint {
                address: dir.join("daemon.sock").to_string_lossy().into_owned(),
                lock_file: dir.join("daemon.lock"),
                dir,
            }
        }
        #[cfg(windows)]
        {
            let name = self.0.file_name().expect("named").to_string_lossy();
            Endpoint {
                address: format!("ossteam-test-{name}"),
                lock_file: self.0.join("daemon.lock"),
            }
        }
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn file_secrets(_file_fallback: bool, data_dir: &Path) -> Secrets {
    Secrets::file(data_dir.join(FALLBACK_FILE))
}

/// A daemon running on a thread of the test process.
pub struct TestDaemon {
    endpoint: Endpoint,
    thread: Option<JoinHandle<()>>,
}

impl TestDaemon {
    pub fn start(root: &Root, cm: &FakeCm) -> Self {
        Self::start_with(root, cm, crate::pics::POLL_INTERVAL)
    }

    /// Starts with PICS change polling every `pics_poll`.
    pub fn start_with(root: &Root, cm: &FakeCm, pics_poll: Duration) -> Self {
        Self::start_full(root, cm, pics_poll, None, None)
    }

    /// Starts with content served by `cdn` (FAITH-4), and PICS change
    /// polling every `pics_poll`.
    pub fn start_with_cdn(
        root: &Root,
        cm: &FakeCm,
        pics_poll: Duration,
        cdn: Arc<FakeCdn>,
    ) -> Self {
        Self::start_full(root, cm, pics_poll, Some(cdn), None)
    }

    /// As [`Self::start_with_cdn`], with install scripts acting on
    /// `scripts`.
    pub fn start_with_scripts(
        root: &Root,
        cm: &FakeCm,
        pics_poll: Duration,
        cdn: Arc<FakeCdn>,
        scripts: Arc<dyn crate::content::script::System>,
    ) -> Self {
        Self::start_full(root, cm, pics_poll, Some(cdn), Some(scripts))
    }

    fn start_full(
        root: &Root,
        cm: &FakeCm,
        pics_poll: Duration,
        cdn: Option<Arc<FakeCdn>>,
        scripts: Option<Arc<dyn crate::content::script::System>>,
    ) -> Self {
        let r = root.path();
        let paths = paths::Paths {
            config: r.join("config"),
            data: r.join("data"),
            cache: r.join("cache"),
            state: r.join("state"),
        };
        let endpoint = root.endpoint();
        endpoint
            .ensure_dir()
            .expect("prepare the endpoint directory");
        let listener = endpoint.listen().expect("listen");
        let env = Env {
            paths,
            listener,
            endpoint: endpoint.clone(),
            dialer: cm.dialer(),
            open_secrets: file_secrets,
            pics_poll,
            content_fetch: cdn.map(|c| c as Arc<dyn ossteam_content::cdn::Fetch>),
            scripts,
        };
        let thread = std::thread::Builder::new()
            .name("test-daemon".into())
            .spawn(move || serve(env, |_| Ok(())).expect("the daemon runs"))
            .expect("spawn the daemon");
        Self {
            endpoint,
            thread: Some(thread),
        }
    }

    pub fn client(&self) -> Client {
        let info = ClientInfo {
            kind: PeerKind::Other,
            name: "it".into(),
            version: "0".into(),
        };
        Client::connect_endpoint(&self.endpoint, &info).expect("connect to the test daemon")
    }

    /// A client subscribed to `topics`.
    pub fn subscribed(&self, topics: &[Topic]) -> Client {
        let c = self.client();
        c.subscribe(topics).expect("subscribe");
        c
    }

    /// Stops the daemon as `ossteam daemon stop` does, and waits for it.
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        let Some(thread) = self.thread.take() else {
            return;
        };
        if let Ok(c) = Client::connect_endpoint(
            &self.endpoint,
            &ClientInfo {
                kind: PeerKind::Other,
                name: "it-stop".into(),
                version: "0".into(),
            },
        ) {
            let _ = c.shutdown(false);
        }
        let _ = thread.join();
    }
}

impl Drop for TestDaemon {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Polls `f` until it holds.
pub fn wait_until(what: &str, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Waits for an event that `f` picks, skipping the others.
pub fn wait_event<T>(c: &Client, what: &str, mut f: impl FnMut(&event::Kind) -> Option<T>) -> T {
    let deadline = Instant::now() + WAIT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match c.events().recv_timeout(left) {
            Ok(Event { kind: Some(k) }) => {
                if let Some(t) = f(&k) {
                    return t;
                }
            }
            Ok(_) => {}
            Err(_) => panic!("timed out waiting for {what}"),
        }
    }
}
