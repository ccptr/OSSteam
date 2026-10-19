// SPDX-License-Identifier: GPL-2.0-or-later
//! M1b against the fake CM and a fake CDN: the checks of
//! `docs/acceptance/m1b.md` that do not need Steam itself. A title with a
//! DLC depot installs, updates through a delta, resumes after the daemon
//! stops mid-download, verifies and repairs a damaged file, and
//! uninstalls.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use ossteam_content::fake::{CHUNK, Depot, Entry};
use ossteam_content::key::DepotKey;
use ossteam_ipc::v1::{self, ErrorCode, JobState, Topic, event};
use ossteam_ipc::{Client, ClientError};
use ossteam_types::{DepotId, ManifestId, Secret};
use ossteam_vdf::Object;

use super::*;

pub(super) const ALPHA: u32 = 10;
pub(super) const ALPHA_DLC: u32 = 11;
pub(super) const BASE: u32 = 101;
pub(super) const DLC: u32 = 102;
/// PICS polling for these tests: updates are found by a PICS change.
pub(super) const POLL: Duration = Duration::from_millis(200);

pub(super) fn key(depot: u32) -> DepotKey {
    DepotKey::new(&[depot as u8; 32]).expect("32 bytes")
}

/// Bytes that differ from chunk to chunk, so no two chunks share an id.
pub(super) fn data(seed: u8, len: usize) -> Vec<u8> {
    let mut x = u32::from(seed).wrapping_mul(2_654_435_761) | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x as u8
        })
        .collect()
}

pub(super) fn depot(id: u32, manifest: u64, files: &[(&str, &[u8])]) -> Depot {
    let entries: Vec<Entry<'_>> = files
        .iter()
        .map(|&(name, data)| Entry::File {
            name,
            data,
            flags: 0,
        })
        .collect();
    Depot::build(DepotId(id), ManifestId(manifest), key(id), &entries)
}

/// The inner object of `"x" { <body> }`.
pub(super) fn section(body: &str) -> Object {
    let o = ossteam_vdf::text::parse(&format!("\"x\" {{ {body} }}")).expect("valid KeyValues");
    o.get_object("x").expect("the section").clone()
}

/// Alpha's `depots` section with these public manifests.
pub(super) fn depots(base: u64, dlc: u64) -> Object {
    section(&format!(
        r#""{BASE}" {{ "manifests" {{ "public" {{ "gid" "{base}" "size" "1" }} }} }}
           "{DLC}" {{ "dlcappid" "{ALPHA_DLC}" "manifests" {{ "public" {{ "gid" "{dlc}" "size" "1" }} }} }}
           "branches" {{ "public" {{ "buildid" "{base}" }} }}"#
    ))
}

pub(super) fn steam(cdn: &Arc<FakeCdn>) -> FakeCm {
    let cm = FakeCm::new()
        .package(100, &[ALPHA, ALPHA_DLC])
        .app(ALPHA, "Alpha", "Game", None)
        .app(ALPHA_DLC, "Alpha Extra", "DLC", Some(ALPHA))
        .cdn(cdn.clone())
        .account(Account::new("alice", 1001).license(100, 0));
    cm.app_section(ALPHA, "config", section(r#""installdir" "Alpha""#));
    cm
}

pub(super) fn login(d: &TestDaemon) {
    let c = d.subscribed(&[Topic::Session]);
    c.login_with_credentials("alice", &Secret::new("alice-password".into()), true)
        .expect("the login starts");
    let ended = wait_event(&c, "the login to end", |k| match k {
        event::Kind::LoginEnded(e) => Some(e.error.clone()),
        _ => None,
    });
    assert!(ended.is_none(), "the login failed: {ended:?}");
    wait_until("the library", || {
        let r = c.list_apps("", false).expect("list apps");
        r.complete && !r.apps.is_empty()
    });
}

fn finished(s: JobState) -> bool {
    matches!(s, JobState::Done | JobState::Failed | JobState::Cancelled)
}

/// Waits for the job to end; `c` must be subscribed to JOBS.
fn ended(c: &Client, job_id: u64) -> v1::Job {
    wait_event(c, "the job to end", |k| match k {
        event::Kind::JobStateChanged(v1::JobStateChanged { job: Some(j) })
            if j.job_id == job_id && finished(j.state()) =>
        {
            Some(j.clone())
        }
        _ => None,
    })
}

pub(super) fn done(c: &Client, job: v1::Job) {
    let j = ended(c, job.job_id);
    assert_eq!(j.state(), JobState::Done, "the job failed: {:?}", j.error);
}

pub(super) fn installed(c: &Client, app: u32) -> Option<v1::InstalledApp> {
    c.get_app(app).expect("get app").installed
}

fn read(dir: &Path, name: &str) -> Vec<u8> {
    std::fs::read(dir.join(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn remote(r: Result<v1::Job, ClientError>) -> ErrorCode {
    match r {
        Err(ClientError::Remote(e)) => e.code,
        other => panic!("expected an error from the daemon, got {other:?}"),
    }
}

#[test]
fn install_update_verify_and_uninstall_with_dlc() {
    let cdn = Arc::new(FakeCdn::new());
    let cm = steam(&cdn);
    let game = data(1, 5 * CHUNK + 100);
    let config = data(2, 300);
    let extra = data(3, 2 * CHUNK);
    let v1 = depot(BASE, 1001, &[("bin/game", &game), ("config.txt", &config)]);
    let dlc = depot(DLC, 2001, &[("dlc/extra.pak", &extra)]);
    cm.publish(&v1, 11);
    cm.publish(&dlc, 21);
    cm.app_section(ALPHA, "depots", depots(1001, 2001));
    let root = Root::new();
    let d = TestDaemon::start_with_cdn(&root, &cm, POLL, cdn.clone());
    login(&d);
    let c = d.subscribed(&[Topic::Jobs]);
    // The update below is started by hand.
    c.set_setting("content.auto_update", Some("never"))
        .expect("setting");

    // Install: the game's depot and the DLC's.
    done(&c, c.install(ALPHA, "", "").expect("install queued"));
    let i = installed(&c, ALPHA).expect("installed");
    let dir = PathBuf::from(&i.path);
    assert!(dir.ends_with("common/Alpha"), "{}", dir.display());
    assert_eq!(read(&dir, "bin/game"), game);
    assert_eq!(read(&dir, "config.txt"), config);
    assert_eq!(read(&dir, "dlc/extra.pak"), extra);
    let mut depots_installed: Vec<(u32, u64)> = i
        .depots
        .iter()
        .map(|d| (d.depot_id, d.manifest_id))
        .collect();
    depots_installed.sort();
    assert_eq!(depots_installed, [(BASE, 1001), (DLC, 2001)]);
    assert!(!i.update_available);
    let dlc_installed = |c: &Client| {
        let app = c.get_app(ALPHA).expect("the app").app.expect("its summary");
        assert_eq!(app.dlc.len(), 1);
        app.dlc[0].installed
    };
    assert!(dlc_installed(&c), "the DLC's depot is installed");
    assert_eq!(i.size, (game.len() + config.len() + extra.len()) as u64);
    assert_eq!(remote(c.install(ALPHA, "", "")), ErrorCode::Conflict);
    let folders = c.list_library_folders().expect("folders");
    assert_eq!(folders.len(), 1);
    assert!(folders[0].is_default);
    assert_eq!(folders[0].app_ids, [ALPHA]);

    // A new build changes one chunk of the game and the config. The PICS
    // change marks the update; the delta fetches only what changed.
    let mut game2 = game.clone();
    game2[CHUNK + 7] ^= 0xff;
    let config2 = data(4, 280);
    let v2 = depot(
        BASE,
        1002,
        &[("bin/game", &game2), ("config.txt", &config2)],
    );
    cm.publish(&v2, 12);
    cm.app_section(ALPHA, "depots", depots(1002, 2001));
    wait_until("the update to be known", || {
        installed(&c, ALPHA).is_some_and(|i| i.update_available)
    });
    let before = cdn.chunk_requests();
    let jobs = c.update(&[ALPHA], false).expect("update queued");
    assert_eq!(jobs.len(), 1);
    done(&c, jobs.into_iter().next().unwrap());
    assert_eq!(cdn.chunk_requests() - before, 2, "one chunk of each file");
    assert_eq!(read(&dir, "bin/game"), game2);
    assert_eq!(read(&dir, "config.txt"), config2);
    assert_eq!(read(&dir, "dlc/extra.pak"), extra);
    let i = installed(&c, ALPHA).expect("installed");
    assert!(!i.update_available);
    assert!(
        i.depots
            .iter()
            .any(|d| d.depot_id == BASE && d.manifest_id == 1002)
    );

    // Verify repairs a damaged file and leaves the rest.
    let mut damaged = game2.clone();
    damaged[3 * CHUNK] ^= 1;
    std::fs::write(dir.join("bin/game"), &damaged).unwrap();
    let before = cdn.chunk_requests();
    done(&c, c.verify(ALPHA).expect("verify queued"));
    assert_eq!(read(&dir, "bin/game"), game2);
    assert_eq!(cdn.chunk_requests() - before, 1, "only the damaged chunk");

    // Uninstall removes the files and the record.
    done(&c, c.uninstall(ALPHA, false).expect("uninstall queued"));
    assert!(!dir.exists());
    assert!(installed(&c, ALPHA).is_none());
    assert!(!dlc_installed(&c));
    assert!(c.list_library_folders().unwrap()[0].app_ids.is_empty());
    assert_eq!(remote(c.verify(ALPHA)), ErrorCode::NotFound);
}

#[test]
fn a_stopped_download_resumes_after_a_restart() {
    let cdn = Arc::new(FakeCdn::new());
    let cm = steam(&cdn);
    // One chunk per file, so what was done before the stop is whole files.
    let files: Vec<(String, Vec<u8>)> = (0..40)
        .map(|n| (format!("data/{n:02}.bin"), data(n + 10, CHUNK)))
        .collect();
    let list: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(n, d)| (n.as_str(), d.as_slice()))
        .collect();
    cm.publish(&depot(BASE, 1001, &list), 11);
    cm.publish(&depot(DLC, 2001, &[("dlc.bin", &data(9, 10))]), 21);
    cm.app_section(ALPHA, "depots", depots(1001, 2001));
    let root = Root::new();

    let d = TestDaemon::start_with_cdn(&root, &cm, POLL, cdn.clone());
    login(&d);
    let c = d.subscribed(&[Topic::Jobs]);
    cdn.hold_after(15);
    let job = c.install(ALPHA, "", "").expect("install queued");
    let fetched = 15;
    let mut stopped = None;
    wait_until("part of the download", || {
        stopped = c
            .list_jobs()
            .expect("jobs")
            .into_iter()
            .find(|j| j.job_id == job.job_id)
            .and_then(|j| j.progress)
            .filter(|p| p.files_done >= fetched);
        stopped.is_some()
    });
    let stopped = stopped.unwrap();
    drop(c);
    d.stop();

    // The job is queued again at the start, runs after the logon from the
    // stored token, and fetches only what it had not.
    cdn.release();
    let before = cdn.chunk_requests();
    let d = TestDaemon::start_with_cdn(&root, &cm, POLL, cdn.clone());
    let c = d.subscribed(&[Topic::Jobs]);
    let listed = c.list_jobs().expect("jobs");
    assert!(
        listed
            .iter()
            .any(|j| j.job_id == job.job_id && !finished(j.state()))
    );
    let j = match listed.iter().find(|j| j.job_id == job.job_id) {
        Some(j) if j.state() == JobState::Done => j.clone(),
        _ => ended(&c, job.job_id),
    };
    assert_eq!(j.state(), JobState::Done, "{:?}", j.error);
    assert_eq!((cdn.chunk_requests() - before) as u64, 41 - fetched);
    // Its progress counts the download from the start, not from the restart.
    let p = j.progress.expect("progress");
    assert_eq!(
        (p.downloaded, p.download_total),
        (stopped.download_total, stopped.download_total),
        "{p:?}"
    );
    let i = installed(&c, ALPHA).expect("installed");
    // The DLC's depot too: the resumed job waited for the library, which
    // says the DLC is owned.
    assert_eq!(i.depots.len(), 2, "{:?}", i.depots);
    let dir = PathBuf::from(i.path);
    for (name, data) in &files {
        assert_eq!(&read(&dir, name), data);
    }
}

#[test]
fn a_new_build_is_installed_automatically() {
    let cdn = Arc::new(FakeCdn::new());
    let cm = steam(&cdn);
    cm.publish(&depot(BASE, 1001, &[("game", &data(1, 100))]), 11);
    cm.publish(&depot(DLC, 2001, &[("dlc", &data(2, 100))]), 21);
    cm.app_section(ALPHA, "depots", depots(1001, 2001));
    let root = Root::new();
    let d = TestDaemon::start_with_cdn(&root, &cm, POLL, cdn.clone());
    login(&d);
    let c = d.subscribed(&[Topic::Jobs]);
    let install = c.install(ALPHA, "", "").expect("install queued");
    done(&c, install.clone());

    let game2 = data(3, 100);
    cm.publish(&depot(BASE, 1002, &[("game", &game2)]), 12);
    cm.app_section(ALPHA, "depots", depots(1002, 2001));
    let update = wait_event(&c, "an update job", |k| match k {
        event::Kind::JobStateChanged(v1::JobStateChanged { job: Some(j) })
            if j.kind() == v1::JobKind::Update && j.app_id == ALPHA =>
        {
            Some(j.clone())
        }
        _ => None,
    });
    assert!(update.job_id > install.job_id);
    let j = if finished(update.state()) {
        update
    } else {
        ended(&c, update.job_id)
    };
    assert_eq!(j.state(), JobState::Done, "{:?}", j.error);
    let i = installed(&c, ALPHA).expect("installed");
    assert_eq!(read(Path::new(&i.path), "game"), game2);
    assert!(!i.update_available);
}

#[test]
fn a_refused_dlc_key_installs_the_rest() {
    let cdn = Arc::new(FakeCdn::new());
    let cm = steam(&cdn);
    let game = data(1, 100);
    cm.publish(&depot(BASE, 1001, &[("game", &game)]), 11);
    cm.publish(&depot(DLC, 2001, &[("dlc", &data(2, 100))]), 21);
    cm.refuse_key(DLC);
    cm.app_section(ALPHA, "depots", depots(1001, 2001));
    let root = Root::new();
    let d = TestDaemon::start_with_cdn(&root, &cm, POLL, cdn.clone());
    login(&d);
    let c = d.subscribed(&[Topic::Jobs]);
    done(&c, c.install(ALPHA, "", "").expect("install queued"));
    let i = installed(&c, ALPHA).expect("installed");
    let dir = PathBuf::from(&i.path);
    assert_eq!(read(&dir, "game"), game);
    assert!(!dir.join("dlc").exists());
    let ids: Vec<u32> = i.depots.iter().map(|d| d.depot_id).collect();
    assert_eq!(ids, [BASE]);
}

#[test]
fn a_depot_shared_from_an_app_outside_the_library() {
    // Redist is in no package, as 228980 is in none of an account's: its
    // info is fetched only because Alpha's depots name it.
    const REDIST: u32 = 20;
    const SHARED: u32 = 201;
    let shared_depots = |manifest: u64| {
        section(&format!(
            r#""{SHARED}" {{ "manifests" {{ "public" {{ "gid" "{manifest}" "size" "1" }} }} }}"#
        ))
    };
    let cdn = Arc::new(FakeCdn::new());
    let cm = steam(&cdn).app(REDIST, "Redist", "Config", None);
    cm.app_section(REDIST, "config", section(r#""installdir" "Redist""#));
    let game = data(1, 100);
    let vcredist = data(2, 100);
    cm.publish(&depot(BASE, 1001, &[("game", &game)]), 11);
    cm.publish(&depot(SHARED, 3001, &[("vcredist", &vcredist)]), 31);
    cm.app_section(REDIST, "depots", shared_depots(3001));
    cm.app_section(
        ALPHA,
        "depots",
        section(&format!(
            r#""{BASE}" {{ "manifests" {{ "public" {{ "gid" "1001" "size" "1" }} }} }}
               "{SHARED}" {{ "depotfromapp" "{REDIST}" "sharedinstall" "1" }}"#
        )),
    );
    let root = Root::new();
    let d = TestDaemon::start_with_cdn(&root, &cm, POLL, cdn.clone());
    login(&d);
    let c = d.subscribed(&[Topic::Jobs]);
    c.set_setting("content.auto_update", Some("never"))
        .expect("setting");
    let apps = c.list_apps("", false).expect("list apps").apps;
    assert!(apps.iter().all(|a| a.app_id != REDIST), "{apps:?}");

    // The shared depot goes to Redist's directory, next to Alpha's.
    done(&c, c.install(ALPHA, "", "").expect("install queued"));
    let i = installed(&c, ALPHA).expect("installed");
    let dir = PathBuf::from(&i.path);
    assert_eq!(read(&dir, "game"), game);
    let redist_dir = dir.parent().unwrap().join("Redist");
    assert_eq!(read(&redist_dir, "vcredist"), vcredist);

    // A new manifest in Redist's info alone is an update for Alpha.
    let vcredist2 = data(3, 100);
    cm.publish(&depot(SHARED, 3002, &[("vcredist", &vcredist2)]), 32);
    cm.app_section(REDIST, "depots", shared_depots(3002));
    wait_until("the update to be known", || {
        installed(&c, ALPHA).is_some_and(|i| i.update_available)
    });
    let jobs = c.update(&[ALPHA], false).expect("update queued");
    done(&c, jobs.into_iter().next().unwrap());
    assert_eq!(read(&redist_dir, "vcredist"), vcredist2);
    assert!(!installed(&c, ALPHA).expect("installed").update_available);
}

/// An install script in the game's depot (`install-scripts.md`), against
/// a registry and process table in memory.
const SCRIPT: &str = r#"
"InstallScript"
{
    "Run Process"
    {
        "Prereq"
        {
            "HasRunKey" "HKEY_CURRENT_USER\\Software\\Alpha"
            "process 1" "%INSTALLDIR%\\_CommonRedist\\prereq.exe"
            "AsCurrentUser" "1"
        }
    }
    "Run Process On Uninstall"
    {
        "Cleanup" { "process 1" "%INSTALLDIR%\\cleanup.exe" "AsCurrentUser" "1" }
        "Unregister" { "process 1" "%INSTALLDIR%\\unregister.exe" "command 1" "/quiet" }
    }
}
"#;

#[test]
fn install_scripts_at_verify_and_uninstall() {
    use crate::content::script::fake::FakeSystem;
    use crate::content::script::{Key, ValueRef};

    let cdn = Arc::new(FakeCdn::new());
    let cm = steam(&cdn);
    let game = data(1, 2 * CHUNK);
    let entries = [
        Entry::File {
            name: "game",
            data: &game,
            flags: 0,
        },
        Entry::File {
            name: "_CommonRedist/install.vdf",
            data: SCRIPT.as_bytes(),
            flags: ossteam_content::manifest::flags::INSTALL_SCRIPT,
        },
    ];
    let base = Depot::build(DepotId(BASE), ManifestId(1001), key(BASE), &entries);
    cm.publish(&base, 11);
    cm.publish(&depot(DLC, 2001, &[("dlc", &data(2, 10))]), 21);
    cm.app_section(ALPHA, "depots", depots(1001, 2001));
    let sys = Arc::new(FakeSystem::default());
    let root = Root::new();
    let d = TestDaemon::start_with_scripts(&root, &cm, POLL, cdn, sys.clone());
    login(&d);
    let c = d.subscribed(&[Topic::Jobs]);

    // Installing runs nothing.
    done(&c, c.install(ALPHA, "", "").expect("install queued"));
    let dir = PathBuf::from(&installed(&c, ALPHA).expect("installed").path);
    assert!(sys.state.lock().unwrap().ran.is_empty());

    // As if a start had run the entry: verify takes the record away, after
    // repairing the damaged script, so it runs again at the next start.
    let prereq = ValueRef {
        key: Key {
            machine: false,
            path: "Software\\Alpha".into(),
        },
        name: Some("Prereq".into()),
    };
    sys.set(&prereq, 1);
    std::fs::write(
        dir.join("_CommonRedist/install.vdf"),
        b"\"InstallScript\" {}",
    )
    .unwrap();
    done(&c, c.verify(ALPHA).expect("verify queued"));
    assert!(!sys.has(&prereq));
    assert_eq!(sys.state.lock().unwrap().prompts, 0);

    // Uninstall runs the uninstall entries, the elevated one behind one
    // prompt, before the files go.
    done(&c, c.uninstall(ALPHA, false).expect("uninstall queued"));
    assert!(!dir.exists());
    let s = sys.state.lock().unwrap();
    let ran: Vec<(String, &str, bool)> = s
        .ran
        .iter()
        .map(|(p, a, e)| (p.rsplit('\\').next().unwrap().to_owned(), a.as_str(), *e))
        .collect();
    assert_eq!(
        ran,
        [
            ("cleanup.exe".to_owned(), "", false),
            ("unregister.exe".to_owned(), "/quiet", true)
        ]
    );
    assert!(s.ran[0].0.starts_with(&*dir.to_string_lossy()));
    assert_eq!(s.prompts, 1);
}

#[test]
fn library_folders_and_job_controls() {
    let cdn = Arc::new(FakeCdn::new());
    let cm = steam(&cdn);
    cm.publish(&depot(BASE, 1001, &[("game", &data(1, 3 * CHUNK))]), 11);
    cm.publish(&depot(DLC, 2001, &[("dlc", &data(2, 10))]), 21);
    cm.app_section(ALPHA, "depots", depots(1001, 2001));
    let root = Root::new();
    let d = TestDaemon::start_with_cdn(&root, &cm, POLL, cdn.clone());
    let c = d.subscribed(&[Topic::Jobs]);

    // Folders need no account.
    let extra = root.path().join("games");
    let f = c.add_library_folder(&extra.to_string_lossy()).expect("add");
    assert!(!f.is_default && f.available);
    assert!(extra.join(".ossteam/library.toml").is_file());
    let err = c.add_library_folder(&extra.to_string_lossy()).unwrap_err();
    assert!(matches!(err, ClientError::Remote(e) if e.code == ErrorCode::Conflict));
    let err = c.add_library_folder("relative/path").unwrap_err();
    assert!(matches!(err, ClientError::Remote(e) if e.code == ErrorCode::InvalidArgument));
    assert_eq!(c.list_library_folders().unwrap().len(), 2);

    // Jobs need one.
    assert_eq!(remote(c.install(ALPHA, "", "")), ErrorCode::NotLoggedIn);
    login(&d);

    // Install into the added folder, paused while it waits.
    cdn.hold_after(0);
    let job = c
        .install(ALPHA, &extra.to_string_lossy(), "")
        .expect("install queued");
    let paused = c.pause_job(job.job_id).expect("pause");
    assert_eq!(paused.state(), JobState::Paused);
    assert_eq!(remote(c.pause_job(job.job_id)), ErrorCode::Conflict);
    let err = c
        .remove_library_folder(&extra.to_string_lossy())
        .unwrap_err();
    assert!(matches!(err, ClientError::Remote(e) if e.code == ErrorCode::Conflict));
    cdn.release();
    // A paused job that was running can be resumed once it has stopped.
    wait_until("the resume", || match c.resume_job(job.job_id) {
        Ok(_) => true,
        Err(ClientError::Remote(e)) if e.code == ErrorCode::Conflict => false,
        Err(e) => panic!("resume: {e}"),
    });
    done(&c, job);
    let i = installed(&c, ALPHA).expect("installed");
    assert!(Path::new(&i.path).starts_with(&extra), "{}", i.path);
    assert_eq!(i.folder, extra.to_string_lossy());
    assert_eq!(
        remote(c.cancel_job(job_id_of(&c, ALPHA))),
        ErrorCode::Conflict
    );

    // Uninstalled, the folder can go; its files stay.
    done(&c, c.uninstall(ALPHA, false).expect("uninstall"));
    c.remove_library_folder(&extra.to_string_lossy())
        .expect("remove");
    assert!(extra.join(".ossteam/library.toml").is_file());
    assert_eq!(c.list_library_folders().unwrap().len(), 1);
}

/// The id of the newest job of `app`.
fn job_id_of(c: &Client, app: u32) -> u64 {
    c.list_jobs()
        .expect("jobs")
        .iter()
        .filter(|j| j.app_id == app)
        .map(|j| j.job_id)
        .max()
        .expect("a job of the app")
}
