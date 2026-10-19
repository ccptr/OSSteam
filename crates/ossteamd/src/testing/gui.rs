// SPDX-License-Identifier: GPL-2.0-or-later
//! The GUI's connection, store and model against this daemon and the fake
//! CM (`gui.md` §7), on gpui's test platform instead of a window. The test
//! calls the daemon as the Login and App pages do, and after each step the
//! store must hold what a leaf client reads from the same daemon (LEAF-3).

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{AppContext as _, Entity, TestAppContext};
use ossteam_content::fake::CHUNK;
use ossteam_gui::daemon::{self, CallError, Update};
use ossteam_gui::model::Model;
use ossteam_gui::store::{LoginOutcome, Store};
use ossteam_ipc::v1::{self, JobState, Topic, event};
use ossteam_ipc::{Client, ClientError};
use ossteam_types::Secret;

use super::m1b::{ALPHA, ALPHA_DLC, BASE, DLC, POLL, data, depot, depots, steam};
use super::*;

/// Runs the app's tasks until `f` holds. Replies and events arrive from
/// other threads in real time; the model's timers run on the test clock,
/// which is moved on as much.
fn wait(cx: &mut TestAppContext, what: &str, mut f: impl FnMut(&mut TestAppContext) -> bool) {
    const STEP: Duration = Duration::from_millis(20);
    let deadline = Instant::now() + WAIT;
    loop {
        cx.run_until_parked();
        if f(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(STEP);
        cx.executor().advance_clock(STEP);
    }
}

/// Waits until `f` holds for the store.
fn settle(cx: &mut TestAppContext, model: &Entity<Model>, what: &str, f: impl Fn(&Store) -> bool) {
    wait(cx, what, |cx| model.read_with(cx, |m, _| f(&m.store)));
}

/// Makes a call through the model's daemon handle, as a page does.
fn try_call<T: Send + 'static>(
    cx: &mut TestAppContext,
    model: &Entity<Model>,
    what: &str,
    f: impl FnOnce(&Client) -> Result<T, ClientError> + Send + 'static,
) -> Result<T, CallError> {
    let call = model.update(cx, |m, _| m.daemon.call(f));
    let out = Rc::new(RefCell::new(None));
    let o = out.clone();
    cx.spawn(|_| async move { *o.borrow_mut() = Some(call.await) })
        .detach();
    wait(cx, what, |_| out.borrow().is_some());
    out.take().expect("the call ended")
}

fn call<T: Send + 'static>(
    cx: &mut TestAppContext,
    model: &Entity<Model>,
    what: &str,
    f: impl FnOnce(&Client) -> Result<T, ClientError> + Send + 'static,
) -> T {
    try_call(cx, model, what, f).unwrap_or_else(|e| panic!("{what}: {e:?}"))
}

/// Starts a login as the Login page does: the store takes prompts from
/// the moment the request goes, and stops the login if it cannot start.
fn sign_in(
    cx: &mut TestAppContext,
    model: &Entity<Model>,
    password: &str,
) -> Result<(), CallError> {
    model.update(cx, |m, cx| {
        m.store.login_started();
        cx.notify();
    });
    let password = Secret::new(password.into());
    let r = try_call(cx, model, "the login to start", move |c| {
        c.login_with_credentials("alice", &password, true)
    });
    if r.is_err() {
        model.update(cx, |m, cx| {
            m.store.login_stopped(None);
            cx.notify();
        });
    }
    r.map(|_| ())
}

/// The account of the next login that succeeds.
fn logged_on(c: &Client) -> v1::Account {
    wait_event(c, "the login to end", |k| match k {
        event::Kind::LoginEnded(v1::LoginEnded {
            account: Some(a), ..
        }) => Some(a.clone()),
        _ => None,
    })
}

fn jobs(c: &Client) -> BTreeMap<u64, v1::Job> {
    let jobs = c.list_jobs().expect("list jobs");
    jobs.into_iter().map(|j| (j.job_id, j)).collect()
}

/// Library folders without their free space, which changes as anything
/// writes to the file system.
fn folders(f: &[v1::LibraryFolder]) -> Vec<v1::LibraryFolder> {
    f.iter()
        .map(|f| v1::LibraryFolder {
            free_bytes: 0,
            ..f.clone()
        })
        .collect()
}

/// What the App page shows: `GetApp` as the store holds it.
fn app_info(s: &Store, app: u32) -> Option<&v1::GetAppResponse> {
    s.apps.get(&app)?.info.as_ref()?.as_ref().ok()
}

#[test]
fn connection_and_store_follow_the_daemon() {
    let cdn = Arc::new(FakeCdn::new());
    let cm = steam(&cdn);
    let game = data(1, 5 * CHUNK + 100);
    let extra = data(3, 2 * CHUNK);
    cm.publish(&depot(BASE, 1001, &[("bin/game", &game)]), 11);
    cm.publish(&depot(DLC, 2001, &[("dlc/extra.pak", &extra)]), 21);
    cm.app_section(ALPHA, "depots", depots(1001, 2001));
    let root = Root::new();
    let d = TestDaemon::start_with_cdn(&root, &cm, POLL, cdn.clone());
    let cli = d.subscribed(&[Topic::Session]);

    let mut cx = TestAppContext::single();
    cx.executor().allow_parking();
    // Root::connect without the window: the connection thread's updates
    // go into the model.
    let updates = daemon::connect_endpoint(d.endpoint().clone());
    let (handle, initial) = match updates.recv_blocking() {
        Ok(Update::Ready(handle, initial)) => (handle, initial),
        Ok(Update::Closed(reason)) => panic!("the connection closed: {reason}"),
        Ok(Update::Event(_)) | Err(_) => panic!("no Ready first"),
    };
    let model = cx.new(|_| Model::new(handle, *initial));
    let closed = Rc::new(RefCell::new(None));
    let _drain = {
        let model = model.clone();
        let closed = closed.clone();
        cx.spawn(|mut cx| async move {
            while let Ok(update) = updates.recv().await {
                match update {
                    Update::Event(kind) => {
                        model.update(&mut cx, |m, cx| m.apply(kind, cx));
                    }
                    Update::Closed(reason) => *closed.borrow_mut() = Some(reason),
                    Update::Ready(..) => panic!("a second Ready"),
                }
            }
        })
    };

    // No account: the Login page, and no library or jobs.
    model.read_with(&cx, |m, _| {
        let s = &m.store;
        assert!(s.needs_login());
        assert!(s.account().is_none());
        assert!(s.library.is_none());
        assert!(s.jobs.is_empty());
        assert_eq!(s.accounts, cli.list_accounts().expect("accounts"));
    });

    // A wrong password: the Login page shows the error `ossteam login`
    // would, and stays.
    let refused = sign_in(&mut cx, &model, "not-alice-password").expect_err("refused");
    let cli_refused = cli
        .login_with_credentials("alice", &Secret::new("not-alice-password".into()), true)
        .map(|_| ())
        .map_err(CallError::from)
        .expect_err("refused");
    assert_eq!(refused, cli_refused);
    model.read_with(&cx, |m, _| {
        let s = &m.store;
        assert_eq!(s.login.ended, None);
        assert!(!s.login.running);
        assert!(s.needs_login());
    });

    // The right one: the account, then its library, as the CLI reads them.
    sign_in(&mut cx, &model, "alice-password").expect("the login starts");
    let account = logged_on(&cli);
    settle(&mut cx, &model, "the library", |s| {
        let apps = cli.list_apps("", false).expect("list apps");
        apps.complete && s.library.as_deref() == Some(&apps)
    });
    settle(&mut cx, &model, "the account", |s| {
        let status = cli.get_status().expect("status");
        s.status.account == status.account
            && s.status.logged_on
            && s.accounts == cli.list_accounts().expect("accounts")
    });
    model.read_with(&cx, |m, _| {
        let s = &m.store;
        assert_eq!(s.login.ended, Some(LoginOutcome::LoggedOn(account.clone())));
        assert!(!s.needs_login());
        let library = s.library.as_deref().expect("a library");
        let ids: Vec<u32> = library.apps.iter().map(|a| a.app_id).collect();
        assert_eq!(ids, [ALPHA]);
    });

    // The App page, then Install from it: the job runs to the end, and the
    // page, the queue and the folders end as the CLI reads them.
    model.update(&mut cx, |m, cx| m.open_app(ALPHA, cx));
    settle(&mut cx, &model, "the App page's details", |s| {
        app_info(s, ALPHA) == Some(&cli.get_app(ALPHA).expect("get app"))
    });
    model.read_with(&cx, |m, _| {
        let info = app_info(&m.store, ALPHA).expect("details");
        let dlc = info
            .depots
            .iter()
            .find(|d| d.depot_id == DLC)
            .expect("the DLC's depot");
        assert_eq!(dlc.dlc_app_id, ALPHA_DLC);
    });
    let job = call(&mut cx, &model, "install", |c| c.install(ALPHA, "", ""));
    model.update(&mut cx, |m, cx| {
        m.store.add_job(job.clone());
        cx.notify();
    });
    settle(&mut cx, &model, "the install job to end", |s| {
        s.jobs
            .get(&job.job_id)
            .is_some_and(|j| j.state() == JobState::Done)
    });
    settle(&mut cx, &model, "the installed app", |s| {
        let info = cli.get_app(ALPHA).expect("get app");
        info.installed.is_some()
            && app_info(s, ALPHA) == Some(&info)
            && s.jobs == jobs(&cli)
            && folders(&s.folders) == folders(&cli.list_library_folders().expect("folders"))
    });
    model.read_with(&cx, |m, _| {
        let j = &m.store.jobs[&job.job_id];
        let p = j.progress.as_ref().expect("progress");
        let size = (game.len() + extra.len()) as u64;
        assert_eq!((p.bytes_done, p.bytes_total), (size, size), "{p:?}");
        let installed = app_info(&m.store, ALPHA).and_then(|i| i.installed.as_ref());
        let mut depots: Vec<u32> = installed
            .expect("installed")
            .depots
            .iter()
            .map(|d| d.depot_id)
            .collect();
        depots.sort();
        assert_eq!(depots, [BASE, DLC]);
    });

    // Uninstall from the App page.
    let job = call(&mut cx, &model, "uninstall", |c| c.uninstall(ALPHA, false));
    model.update(&mut cx, |m, cx| {
        m.store.add_job(job.clone());
        cx.notify();
    });
    settle(&mut cx, &model, "the uninstalled app", |s| {
        let info = cli.get_app(ALPHA).expect("get app");
        info.installed.is_none()
            && app_info(s, ALPHA) == Some(&info)
            && s.jobs == jobs(&cli)
            && folders(&s.folders) == folders(&cli.list_library_folders().expect("folders"))
    });

    // `ossteam daemon stop`: the window says why the connection ended.
    drop(cli);
    d.stop();
    wait(&mut cx, "the connection to close", |_| {
        closed.borrow().is_some()
    });
    assert_eq!(closed.borrow().as_deref(), Some("ossteamd stopped"));
}
