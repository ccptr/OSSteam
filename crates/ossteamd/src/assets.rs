// SPDX-License-Identifier: GPL-2.0-or-later
//! App images (`pics.md` §9, `ipc.md` §5.3), fetched from Steam's asset CDN
//! into `<cache>/assets`. Downloads block, so a few threads of their own
//! run them. The core answers requests for images already cached itself,
//! and sends each missing image here once however many clients ask for it.

use std::io;
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use ossteam_types::AppId;

use crate::core::Command;

/// The base the official client uses for library images (`pics.md` §9).
pub const BASE_URL: &str = "https://shared.steamstatic.com/store_item_assets/steam/apps";

const THREADS: usize = 4;
/// Library images are well under 1 MiB; anything far larger is not one.
const MAX_SIZE: u64 = 16 << 20;
/// How long shutdown waits for downloads in progress.
const STOP_WAIT: Duration = Duration::from_secs(2);

/// Where an image comes from and where it is cached.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Asset {
    pub url: String,
    pub path: PathBuf,
    /// `<assets>/<app>/<mtime>`, which holds this version's images.
    version_dir: PathBuf,
}

impl Asset {
    /// `rel` is a path the app's info gives, already checked to be plain
    /// and relative. Images are versioned by `mtime`, so each version has
    /// its own directory and a cached file never changes.
    pub fn new(dir: &Path, app: AppId, mtime: u64, rel: &str) -> Self {
        let version_dir = dir.join(app.0.to_string()).join(mtime.to_string());
        let mut path = version_dir.clone();
        path.extend(rel.split('/'));
        Self {
            url: format!("{BASE_URL}/{}/{rel}?t={mtime}", app.0),
            path,
            version_dir,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Fetched,
    /// The CDN answered 404.
    Missing,
    Failed(String),
}

/// A finished download, reported to the core.
pub struct Done {
    pub asset: Asset,
    pub outcome: Outcome,
}

pub struct Handle {
    tx: Option<Sender<Asset>>,
    /// Kept to drop queued downloads at shutdown.
    queue: Receiver<Asset>,
    /// Disconnects when every thread has exited.
    done: Receiver<()>,
    threads: Vec<JoinHandle<()>>,
}

impl Handle {
    pub fn spawn(core: Sender<Command>) -> io::Result<Self> {
        let (tx, rx) = crossbeam_channel::unbounded::<Asset>();
        let (done_tx, done) = crossbeam_channel::bounded::<()>(0);
        let agent = ossteam_net::tls::http_agent();
        let threads = (0..THREADS)
            .map(|n| {
                let (rx, core, agent, done_tx) =
                    (rx.clone(), core.clone(), agent.clone(), done_tx.clone());
                std::thread::Builder::new()
                    .name(format!("assets-{n}"))
                    .spawn(move || {
                        let _done = done_tx;
                        for asset in rx {
                            let outcome = fetch(&agent, &asset, n);
                            if let Outcome::Failed(e) = &outcome {
                                tracing::warn!(url = %asset.url, "image download failed: {e}");
                            }
                            let _ = core.send(Command::Asset(Done { asset, outcome }));
                        }
                    })
            })
            .collect::<io::Result<_>>()?;
        Ok(Self {
            tx: Some(tx),
            queue: rx,
            done,
            threads,
        })
    }

    pub fn fetch(&self, asset: Asset) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(asset);
        }
    }

    /// Drops queued downloads and waits briefly for those in progress.
    /// Later calls do nothing.
    pub fn shutdown(&mut self) {
        let Some(tx) = self.tx.take() else {
            return;
        };
        drop(tx);
        // Threads stop after their current download.
        while self.queue.try_recv().is_ok() {}
        match self.done.recv_timeout(STOP_WAIT) {
            Err(RecvTimeoutError::Disconnected) | Ok(()) => {
                for t in self.threads.drain(..) {
                    let _ = t.join();
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                tracing::warn!("image downloads still running at shutdown");
            }
        }
    }
}

fn fetch(agent: &ureq::Agent, asset: &Asset, thread: usize) -> Outcome {
    let body = match agent.get(&asset.url).call() {
        Ok(r) => r.into_body().with_config().limit(MAX_SIZE).read_to_vec(),
        Err(ureq::Error::StatusCode(404)) => return Outcome::Missing,
        Err(e) => return Outcome::Failed(e.to_string()),
    };
    let bytes = match body {
        Ok(b) => b,
        Err(e) => return Outcome::Failed(e.to_string()),
    };
    match store(asset, &bytes, thread) {
        Ok(()) => Outcome::Fetched,
        Err(e) => Outcome::Failed(format!("writing {}: {e}", asset.path.display())),
    }
}

/// Writes the image under a temporary name and renames it into place, so
/// a reader never sees half a file. Then removes the app's other versions;
/// failing to only leaves old files behind.
fn store(asset: &Asset, bytes: &[u8], thread: usize) -> io::Result<()> {
    let path = &asset.path;
    let dir = path.parent().expect("asset paths have a parent");
    std::fs::create_dir_all(dir)?;
    let name = path.file_name().expect("asset paths have a name");
    let tmp = dir.join(format!(".{}.{thread}.part", name.to_string_lossy()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    if let (Some(app_dir), Some(version)) =
        (asset.version_dir.parent(), asset.version_dir.file_name())
        && let Ok(entries) = std::fs::read_dir(app_dir)
    {
        for e in entries.flatten() {
            if e.file_name() != version {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_and_paths() {
        let a = Asset::new(Path::new("/c/assets"), AppId(730), 17, "abc/header.jpg");
        assert_eq!(
            a.url,
            "https://shared.steamstatic.com/store_item_assets/steam/apps/730/abc/header.jpg?t=17"
        );
        assert_eq!(
            a.path,
            Path::new("/c/assets")
                .join("730")
                .join("17")
                .join("abc")
                .join("header.jpg")
        );
        assert_eq!(a.version_dir, Path::new("/c/assets").join("730").join("17"));
    }

    #[test]
    fn storing_replaces_older_versions() {
        let root = std::env::temp_dir().join(format!("ossteamd-assets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let old = Asset::new(&root, AppId(10), 1, "library_600x900.jpg");
        let new = Asset::new(&root, AppId(10), 2, "h/library_600x900.jpg");
        let other_app = Asset::new(&root, AppId(20), 1, "logo.png");
        store(&old, b"old", 0).unwrap();
        store(&other_app, b"x", 0).unwrap();
        assert_eq!(std::fs::read(&old.path).unwrap(), b"old");

        store(&new, b"new", 1).unwrap();
        assert_eq!(std::fs::read(&new.path).unwrap(), b"new");
        assert!(!old.path.exists());
        assert!(other_app.path.exists());
        // No temporary files are left.
        let names: Vec<_> = std::fs::read_dir(new.path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["library_600x900.jpg"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Fetches a real image from the CDN: run with `--ignored` to check
    /// the URL scheme still holds (`pics.md` §9).
    #[test]
    #[ignore = "needs the network"]
    fn fetches_from_the_cdn() {
        let root = std::env::temp_dir().join(format!("ossteamd-cdn-{}", std::process::id()));
        let agent = ossteam_net::tls::http_agent();
        let logo = Asset::new(&root, AppId(730), 0, "logo.png");
        assert_eq!(fetch(&agent, &logo, 0), Outcome::Fetched);
        assert!(std::fs::read(&logo.path).unwrap().starts_with(b"\x89PNG"));
        let none = Asset::new(&root, AppId(730), 0, "no_such_image.jpg");
        assert_eq!(fetch(&agent, &none, 0), Outcome::Missing);
        let _ = std::fs::remove_dir_all(&root);
    }
}
