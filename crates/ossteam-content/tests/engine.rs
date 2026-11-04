// SPDX-License-Identifier: GPL-2.0-or-later
//! The install engine against the fake CDN: install, delta update, resume,
//! verify, retries and path safety (`content.md` §7 to §11).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use ossteam_content::cdn::{Rotation, Server};
use ossteam_content::download::Downloader;
use ossteam_content::engine::{self, DepotWork, EngineError, Hooks, Job, Mode, Progress, Removal};
use ossteam_content::fake::{CHUNK, Depot, Entry, FakeCdn};
use ossteam_content::manifest::flags;
use ossteam_content::{DepotKey, Manifest};
use ossteam_types::{CancelToken, DepotId, ManifestId};

const DEPOT: DepotId = DepotId(481);

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let p =
            std::env::temp_dir().join(format!("ossteam-engine-{}-{name}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn key() -> DepotKey {
    DepotKey::new(&[0x42; 32]).unwrap()
}

fn data(seed: u8, len: usize) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
        .collect()
}

fn servers() -> Rotation {
    let s = |h: &str| Server {
        kind: "CDN".into(),
        host: h.into(),
        vhost: h.into(),
    };
    Rotation::new(vec![s("a.cdn"), s("b.cdn")]).unwrap()
}

fn downloader(cdn: &Arc<FakeCdn>) -> Downloader {
    Downloader::new(cdn.clone(), servers(), Some(cdn.clone()))
}

/// Fetches the published manifest as the daemon would and readies it.
fn fetch_manifest(cdn: &Arc<FakeCdn>, depot: &Depot) -> Manifest {
    let mut m = downloader(cdn)
        .manifest(DEPOT, depot.manifest.id, 77, &CancelToken::new())
        .unwrap();
    m.decrypt_names(&depot.key).unwrap();
    m.check().unwrap();
    m
}

struct Lib {
    _root: Dir,
    install: PathBuf,
    staging: PathBuf,
}

fn lib(name: &str) -> Lib {
    let root = Dir::new(name);
    let install = root.0.join("apps").join("Game");
    let staging = root.0.join(".ossteam").join("staging").join("480");
    Lib {
        _root: root,
        install,
        staging,
    }
}

fn job(l: &Lib, mode: Mode, manifest: Manifest, installed: Option<Manifest>) -> Job {
    Job {
        mode,
        depots: vec![DepotWork {
            key: key(),
            install_dir: l.install.clone(),
            manifest,
            installed,
        }],
        removed: Vec::new(),
        staging: l.staging.clone(),
    }
}

fn run(
    job: &Job,
    cdn: &Arc<FakeCdn>,
    journal: Option<&[u8]>,
) -> Result<engine::Outcome, EngineError> {
    let cancel = CancelToken::new();
    let p = engine::prepare(job, journal, &Hooks::none(), &cancel)?;
    p.run(&downloader(cdn), 4, &Hooks::none(), &cancel)
}

fn read(p: &Path) -> Vec<u8> {
    std::fs::read(p).unwrap()
}

fn v1() -> (Vec<u8>, Vec<u8>, Depot) {
    let big = data(1, CHUNK * 5 + 100);
    let small = data(2, 300);
    let mut d = Depot::build(
        DEPOT,
        ManifestId(1001),
        key(),
        &[
            Entry::Dir("bin"),
            Entry::File {
                name: "bin\\game",
                data: &big,
                flags: flags::EXECUTABLE,
            },
            Entry::File {
                name: "data/small.txt",
                data: &small,
                flags: 0,
            },
            Entry::File {
                name: "empty",
                data: b"",
                flags: 0,
            },
            Entry::File {
                name: "settings.cfg",
                data: b"defaults",
                flags: flags::USER_CONFIG,
            },
            Entry::File {
                name: "old.txt",
                data: b"going away",
                flags: 0,
            },
            Entry::Dir("olddir"),
        ],
    );
    d.encrypt_names = true;
    (big, small, d)
}

#[test]
fn installs_a_depot() {
    let cdn = Arc::new(FakeCdn::new());
    let (big, small, d) = v1();
    cdn.publish(&d, 77);
    let l = lib("install");
    let m = fetch_manifest(&cdn, &d);
    let out = run(&job(&l, Mode::Install, m, None), &cdn, None).unwrap();

    assert_eq!(read(&l.install.join("bin/game")), big);
    assert_eq!(read(&l.install.join("data/small.txt")), small);
    assert_eq!(read(&l.install.join("empty")), b"");
    assert_eq!(read(&l.install.join("settings.cfg")), b"defaults");
    assert!(l.install.join("olddir").is_dir());
    assert_eq!(out.files_written, 5);
    assert!(!l.staging.exists(), "staging is removed");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &str| {
            std::fs::metadata(l.install.join(p))
                .unwrap()
                .permissions()
                .mode()
        };
        assert_eq!(mode("bin/game") & 0o111, 0o111);
        // Every file, as the official client makes them (content.md §8).
        assert_eq!(mode("data/small.txt") & 0o111, 0o111);
    }
}

/// Files copied into place beforehand are kept, not downloaded or
/// written again (`content.md` §9); a copy that differs is rebuilt.
#[test]
fn a_new_install_keeps_files_already_in_place() {
    let cdn = Arc::new(FakeCdn::new());
    let (big, small, d) = v1();
    cdn.publish(&d, 77);
    let l = lib("in-place");
    std::fs::create_dir_all(l.install.join("bin")).unwrap();
    std::fs::create_dir_all(l.install.join("data")).unwrap();
    std::fs::write(l.install.join("bin/game"), &big).unwrap();
    let mut wrong = small.clone();
    wrong[0] ^= 1;
    std::fs::write(l.install.join("data/small.txt"), &wrong).unwrap();
    let m = fetch_manifest(&cdn, &d);
    let before = cdn.chunk_requests();
    let out = run(&job(&l, Mode::Install, m, None), &cdn, None).unwrap();

    assert_eq!(read(&l.install.join("bin/game")), big);
    assert_eq!(read(&l.install.join("data/small.txt")), small);
    // All but bin/game: small.txt, empty, settings.cfg and old.txt.
    assert_eq!(out.files_written, 4);
    // The big file's six chunks are neither fetched nor copied.
    assert_eq!(out.chunks_copied, 0);
    assert!(cdn.chunk_requests() - before <= 4);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(l.install.join("bin/game"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o111, 0o111, "a kept file gets its permissions");
    }
}

#[test]
fn updates_by_delta_and_deletes_what_is_gone() {
    let cdn = Arc::new(FakeCdn::new());
    let (big, small, d1) = v1();
    cdn.publish(&d1, 77);
    let l = lib("update");
    let m1 = fetch_manifest(&cdn, &d1);
    run(&job(&l, Mode::Install, m1.clone(), None), &cdn, None).unwrap();
    std::fs::write(l.install.join("settings.cfg"), b"the user's").unwrap();
    std::fs::write(l.install.join("olddir/mine"), b"the user's file").unwrap();

    // v2: one chunk of the big file changes, the small file moves, a file
    // is added, and old.txt and olddir go away.
    let mut big2 = big.clone();
    big2[CHUNK * 2 + 10] ^= 0xff;
    let added = data(9, CHUNK + 1);
    let d2 = Depot::build(
        DEPOT,
        ManifestId(1002),
        key(),
        &[
            Entry::Dir("bin"),
            Entry::File {
                name: "bin\\game",
                data: &big2,
                flags: flags::EXECUTABLE,
            },
            Entry::File {
                name: "moved/small.txt",
                data: &small,
                flags: 0,
            },
            Entry::File {
                name: "added.bin",
                data: &added,
                flags: 0,
            },
            Entry::File {
                name: "empty",
                data: b"",
                flags: 0,
            },
            Entry::File {
                name: "settings.cfg",
                data: b"new defaults",
                flags: flags::USER_CONFIG,
            },
        ],
    );
    cdn.publish(&d2, 77);
    let m2 = fetch_manifest(&cdn, &d2);
    let before = cdn.chunk_requests();
    let out = run(&job(&l, Mode::Install, m2, Some(m1)), &cdn, None).unwrap();

    assert_eq!(read(&l.install.join("bin/game")), big2);
    assert_eq!(read(&l.install.join("moved/small.txt")), small);
    assert_eq!(read(&l.install.join("added.bin")), added);
    assert_eq!(read(&l.install.join("settings.cfg")), b"the user's");
    assert!(!l.install.join("data/small.txt").exists());
    assert!(!l.install.join("old.txt").exists());
    assert!(
        l.install.join("olddir/mine").exists(),
        "a directory with user files stays"
    );
    // Only the changed chunk and the two of the added file are downloaded.
    assert_eq!(cdn.chunk_requests() - before, 3);
    assert_eq!(out.chunks_copied, 6);
    assert_eq!(out.files_deleted, 2);
}

#[test]
fn resumes_from_the_journal() {
    let cdn = Arc::new(FakeCdn::new());
    let (big, _, d) = v1();
    cdn.publish(&d, 77);
    let l = lib("resume");
    let m = fetch_manifest(&cdn, &d);
    // One chunk of the big file is unavailable, so the first run fails
    // after staging the others.
    let last = m
        .files
        .iter()
        .find(|f| f.name == "bin\\game")
        .unwrap()
        .chunks[5]
        .clone();
    let path = ossteam_content::cdn::chunk_path(DEPOT, &last, None);
    cdn.fail(&path, 503, usize::MAX);
    let j = job(&l, Mode::Install, m, None);
    let journal = Mutex::new(Vec::new());
    let save = |b: Vec<u8>| *journal.lock().unwrap() = b;
    let last_progress = Mutex::new(Progress::default());
    let keep = |p: &Progress| *last_progress.lock().unwrap() = p.clone();
    let hooks = Hooks {
        progress: &keep,
        journal: &save,
    };
    let cancel = CancelToken::new();
    let err = engine::prepare(&j, None, &hooks, &cancel)
        .unwrap()
        .run(&downloader(&cdn), 1, &hooks, &cancel)
        .unwrap_err();
    assert!(matches!(err, EngineError::Download(_)), "{err}");
    assert!(!l.install.join("bin/game").exists());
    let first = last_progress.lock().unwrap().clone();
    assert!(
        0 < first.downloaded && first.downloaded < first.download_total,
        "{first:?}"
    );

    cdn.fail(&path, 503, 0);
    let before = cdn.chunk_requests();
    let saved = journal.lock().unwrap().clone();
    let p = engine::prepare(&j, Some(&saved), &Hooks::none(), &cancel).unwrap();
    // Staged chunks need not be written again.
    assert!(p.bytes_to_write < big.len() as u64, "{}", p.bytes_to_write);
    let seen = Mutex::new(Vec::new());
    let watch = |p: &Progress| seen.lock().unwrap().push(p.clone());
    let hooks = Hooks {
        progress: &watch,
        journal: &|_| {},
    };
    p.run(&downloader(&cdn), 1, &hooks, &cancel).unwrap();
    // The download goes on from where the first run left it.
    let seen = seen.into_inner().unwrap();
    assert!(
        seen.iter().all(|p| p.downloaded >= first.downloaded),
        "{seen:?}"
    );
    let end = seen.last().unwrap();
    assert_eq!(end.download_total, first.download_total, "{end:?}");
    assert_eq!(end.downloaded, end.download_total, "{end:?}");
    assert_eq!(read(&l.install.join("bin/game")), big);
    assert!(
        cdn.chunk_requests() - before < 6,
        "staged chunks are not fetched again"
    );

    // A journal for other manifests is ignored.
    let other = engine::Journal::decode(&saved).unwrap();
    assert!(engine::Journal::decode(&saved[..saved.len() - 1]).is_none());
    // The format before the downloaded count is still read.
    let old = [&b"OSJ1"[..], &saved[12..]].concat();
    assert!(engine::Journal::decode(&old).is_some());
    assert_eq!(engine::Journal::decode(&other.encode()), Some(other));
}

#[test]
fn verify_repairs_only_what_is_wrong() {
    let cdn = Arc::new(FakeCdn::new());
    let (big, small, d) = v1();
    cdn.publish(&d, 77);
    let l = lib("verify");
    let m = fetch_manifest(&cdn, &d);
    run(&job(&l, Mode::Install, m.clone(), None), &cdn, None).unwrap();

    let game = l.install.join("bin/game");
    let mut bad = read(&game);
    // `data` repeats every 256 bytes, so the first five chunks of the
    // game are one chunk: a bad one is repaired from a good copy. The
    // short last chunk is unique and has to be downloaded.
    bad[CHUNK * 3 + 1] ^= 1;
    bad[CHUNK * 5 + 1] ^= 1;
    std::fs::write(&game, &bad).unwrap();
    std::fs::remove_file(l.install.join("data/small.txt")).unwrap();
    std::fs::write(l.install.join("settings.cfg"), b"changed by the user").unwrap();

    let before = cdn.chunk_requests();
    let out = run(&job(&l, Mode::Verify, m.clone(), Some(m)), &cdn, None).unwrap();
    assert_eq!(out.files_written, 2);
    assert_eq!(read(&game), big);
    assert_eq!(read(&l.install.join("data/small.txt")), small);
    assert_eq!(
        read(&l.install.join("settings.cfg")),
        b"changed by the user"
    );
    // The last chunk of the game and the deleted file's one chunk.
    assert_eq!(cdn.chunk_requests() - before, 2);
}

#[test]
fn retries_other_servers_and_fetches_tokens() {
    let cdn = Arc::new(FakeCdn::new());
    let (big, _, d) = v1();
    cdn.publish(&d, 77);
    let l = lib("retry");
    let m = fetch_manifest(&cdn, &d);
    let chunks = &m
        .files
        .iter()
        .find(|f| f.name == "bin\\game")
        .unwrap()
        .chunks;
    cdn.fail(
        &ossteam_content::cdn::chunk_path(DEPOT, &chunks[0], None),
        500,
        2,
    );
    cdn.corrupt(
        &ossteam_content::cdn::chunk_path(DEPOT, &chunks[1], None),
        1,
    );
    cdn.require_token("b.cdn", "s3cret");
    run(&job(&l, Mode::Install, m, None), &cdn, None).unwrap();
    assert_eq!(read(&l.install.join("bin/game")), big);
    assert_eq!(cdn.tokens_issued(), 1, "one token per depot and host");
}

#[test]
fn gives_up_after_five_attempts() {
    let cdn = Arc::new(FakeCdn::new());
    let (_, _, d) = v1();
    cdn.publish(&d, 77);
    let l = lib("giveup");
    let m = fetch_manifest(&cdn, &d);
    let c = m
        .files
        .iter()
        .find(|f| f.name == "data/small.txt")
        .unwrap()
        .chunks[0]
        .clone();
    cdn.corrupt(
        &ossteam_content::cdn::chunk_path(DEPOT, &c, None),
        usize::MAX,
    );
    let err = run(&job(&l, Mode::Install, m, None), &cdn, None).unwrap_err();
    let EngineError::Download(e) = err else {
        panic!("{err}")
    };
    assert!(e.to_string().contains("5 times"), "{e}");
}

#[test]
fn removed_depots_are_deleted() {
    let cdn = Arc::new(FakeCdn::new());
    let (_, _, d) = v1();
    cdn.publish(&d, 77);
    let l = lib("removed");
    let m = fetch_manifest(&cdn, &d);
    run(&job(&l, Mode::Install, m.clone(), None), &cdn, None).unwrap();
    let j = Job {
        mode: Mode::Install,
        depots: Vec::new(),
        removed: vec![Removal {
            install_dir: l.install.clone(),
            manifest: m,
        }],
        staging: l.staging.clone(),
    };
    let out = run(&j, &cdn, None).unwrap();
    assert!(!l.install.join("bin/game").exists());
    assert!(!l.install.join("bin").exists());
    assert!(
        l.install.join("settings.cfg").exists(),
        "user configuration stays"
    );
    assert_eq!(out.files_deleted, 4);
}

#[cfg(unix)]
#[test]
fn links_are_made_and_never_followed() {
    let cdn = Arc::new(FakeCdn::new());
    let outside = Dir::new("outside");
    let d = Depot::build(
        DEPOT,
        ManifestId(5),
        key(),
        &[
            Entry::Link {
                name: "escape",
                target: outside.0.to_str().unwrap(),
            },
            Entry::File {
                name: "run.sh",
                data: b"#!/bin/sh\n",
                flags: flags::EXECUTABLE,
            },
            Entry::Link {
                name: "start",
                target: "run.sh",
            },
        ],
    );
    cdn.publish(&d, 77);
    let l = lib("links");
    let m = fetch_manifest(&cdn, &d);
    let out = run(&job(&l, Mode::Install, m, None), &cdn, None).unwrap();
    assert_eq!(out.links, 2);
    assert_eq!(read(&l.install.join("start")), b"#!/bin/sh\n");

    // A later manifest that writes under the link is refused, and nothing
    // lands outside.
    let evil = Depot::build(
        DEPOT,
        ManifestId(6),
        key(),
        &[Entry::File {
            name: "escape/pwned",
            data: b"x",
            flags: 0,
        }],
    );
    cdn.publish(&evil, 77);
    let m = fetch_manifest(&cdn, &evil);
    let err = run(&job(&l, Mode::Install, m, None), &cdn, None).unwrap_err();
    assert!(matches!(err, EngineError::Unsafe(_)), "{err}");
    assert!(!outside.0.join("pwned").exists());
}

#[test]
fn cancellation_stops_the_job() {
    let cdn = Arc::new(FakeCdn::new());
    let (_, _, d) = v1();
    cdn.publish(&d, 77);
    let l = lib("cancel");
    let m = fetch_manifest(&cdn, &d);
    let j = job(&l, Mode::Install, m, None);
    let cancel = CancelToken::new();
    let p = engine::prepare(&j, None, &Hooks::none(), &cancel).unwrap();
    cancel.cancel();
    let err = p
        .run(&downloader(&cdn), 2, &Hooks::none(), &cancel)
        .unwrap_err();
    assert!(matches!(err, EngineError::Cancelled), "{err}");
}
