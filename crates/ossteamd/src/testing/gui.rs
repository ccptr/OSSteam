// SPDX-License-Identifier: GPL-2.0-or-later
//! The GUI's connection, store and model against this daemon and the fake
//! CM (`gui.md` §7), without a window: what the GUI's threads send its
//! main thread comes through a channel, and the test applies it to the
//! model as the main thread does. The test calls the daemon as the Login
//! and App pages do, and after each step the store must hold what a leaf
//! client reads from the same daemon (LEAF-3).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use ossteam_content::fake::CHUNK;
use ossteam_gui::daemon::{self, CallError, Update};
use ossteam_gui::model::{Model, Reply};
use ossteam_gui::store::{LoginOutcome, Store};
use ossteam_ipc::v1::{self, JobState, Topic, event};
use ossteam_ipc::{Client, ClientError};
use ossteam_types::Secret;

use super::m1b::{ALPHA, ALPHA_DLC, BASE, DLC, POLL, data, depot, depots, steam};
use super::*;

/// What the GUI's threads send its main thread.
enum Msg {
    Update(Update),
    Reply(Reply),
    /// A page's call ended (its result goes to the page).
    Called,
}

/// The GUI's main thread, without the window.
struct Gui {
    model: Model,
    messages: Receiver<Msg>,
    sender: Sender<Msg>,
    /// Why the connection ended, once it has.
    closed: Option<String>,
}

impl Gui {
    /// Applies what arrives until `f` holds, keeping the model's timers.
    fn wait(&mut self, what: &str, mut f: impl FnMut(&mut Self) -> bool) {
        let deadline = Instant::now() + WAIT;
        while !f(self) {
            let now = Instant::now();
            assert!(now < deadline, "timed out waiting for {what}");
            let until = self.model.deadline().map_or(deadline, |t| t.min(deadline));
            match self
                .messages
                .recv_timeout(until.saturating_duration_since(now))
            {
                Ok(Msg::Reply(r)) => self.model.handle(r),
                Ok(Msg::Update(Update::Event(kind))) => self.model.apply(kind),
                Ok(Msg::Update(Update::Closed(reason))) => self.closed = Some(reason),
                Ok(Msg::Update(Update::Ready(..))) => panic!("a second Ready"),
                Ok(Msg::Called) => {}
                Err(RecvTimeoutError::Timeout) => self.model.tick(Instant::now()),
                Err(RecvTimeoutError::Disconnected) => panic!("the GUI's threads are gone"),
            }
        }
    }

    /// Waits until `f` holds for the store.
    fn settle(&mut self, what: &str, f: impl Fn(&Store) -> bool) {
        self.wait(what, |g| f(&g.model.store));
    }

    /// Makes a call through the model's daemon handle, as a page does.
    fn try_call<T: Send + 'static>(
        &mut self,
        what: &str,
        f: impl FnOnce(&Client) -> Result<T, ClientError> + Send + 'static,
    ) -> Result<T, CallError> {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let wake = self.sender.clone();
        self.model.daemon.call(f, move |r| {
            let _ = tx.send(r);
            let _ = wake.send(Msg::Called);
        });
        let mut out = None;
        self.wait(what, |_| {
            out = out.take().or_else(|| rx.try_recv().ok());
            out.is_some()
        });
        out.expect("the call ended")
    }

    fn call<T: Send + 'static>(
        &mut self,
        what: &str,
        f: impl FnOnce(&Client) -> Result<T, ClientError> + Send + 'static,
    ) -> T {
        self.try_call(what, f)
            .unwrap_or_else(|e| panic!("{what}: {e:?}"))
    }

    /// Starts a login as the Login page does: the store takes prompts from
    /// the moment the request goes, and stops the login if it cannot start.
    fn sign_in(&mut self, password: &str) -> Result<(), CallError> {
        self.model.store.login_started();
        let password = Secret::new(password.into());
        let r = self.try_call("the login to start", move |c| {
            c.login_with_credentials("alice", &password, true)
        });
        if r.is_err() {
            self.model.store.login_stopped(None);
        }
        r.map(|_| ())
    }
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

    // The GUI's start without the window: the connection thread's
    // updates and the model's replies come through one channel.
    let (tx, messages) = crossbeam_channel::unbounded();
    let updates = tx.clone();
    daemon::connect_endpoint(d.endpoint().clone(), move |u| {
        updates.send(Msg::Update(u)).is_ok()
    });
    let (handle, initial) = match messages.recv_timeout(WAIT) {
        Ok(Msg::Update(Update::Ready(handle, initial))) => (handle, initial),
        Ok(Msg::Update(Update::Closed(reason))) => panic!("the connection closed: {reason}"),
        _ => panic!("no Ready first"),
    };
    let replies = tx.clone();
    let post = Arc::new(move |r| {
        let _ = replies.send(Msg::Reply(r));
    });
    let mut gui = Gui {
        model: Model::new(handle, *initial, post),
        messages,
        sender: tx,
        closed: None,
    };

    // No account: the Login page, and no library or jobs.
    {
        let s = &gui.model.store;
        assert!(s.needs_login());
        assert!(s.account().is_none());
        assert!(s.library.is_none());
        assert!(s.jobs.is_empty());
        assert_eq!(s.accounts, cli.list_accounts().expect("accounts"));
    }

    // A wrong password: the Login page shows the error `ossteam login`
    // would, and stays.
    let refused = gui.sign_in("not-alice-password").expect_err("refused");
    let cli_refused = cli
        .login_with_credentials("alice", &Secret::new("not-alice-password".into()), true)
        .map(|_| ())
        .map_err(CallError::from)
        .expect_err("refused");
    assert_eq!(refused, cli_refused);
    {
        let s = &gui.model.store;
        assert_eq!(s.login.ended, None);
        assert!(!s.login.running);
        assert!(s.needs_login());
    }

    // The right one: the account, then its library, as the CLI reads them.
    gui.sign_in("alice-password").expect("the login starts");
    let account = logged_on(&cli);
    gui.settle("the library", |s| {
        let apps = cli.list_apps("", false).expect("list apps");
        apps.complete && s.library.as_deref() == Some(&apps)
    });
    gui.settle("the account", |s| {
        let status = cli.get_status().expect("status");
        s.status.account == status.account
            && s.status.logged_on
            && s.accounts == cli.list_accounts().expect("accounts")
    });
    {
        let s = &gui.model.store;
        assert_eq!(s.login.ended, Some(LoginOutcome::LoggedOn(account.clone())));
        assert!(!s.needs_login());
        let library = s.library.as_deref().expect("a library");
        let ids: Vec<u32> = library.apps.iter().map(|a| a.app_id).collect();
        assert_eq!(ids, [ALPHA]);
    }

    // The App page, then Install from it: the job runs to the end, and the
    // page, the queue and the folders end as the CLI reads them.
    gui.model.open_app(ALPHA);
    gui.settle("the App page's details", |s| {
        app_info(s, ALPHA) == Some(&cli.get_app(ALPHA).expect("get app"))
    });
    {
        let info = app_info(&gui.model.store, ALPHA).expect("details");
        let dlc = info
            .depots
            .iter()
            .find(|d| d.depot_id == DLC)
            .expect("the DLC's depot");
        assert_eq!(dlc.dlc_app_id, ALPHA_DLC);
    }
    let job = gui.call("install", |c| c.install(ALPHA, "", ""));
    gui.model.store.add_job(job.clone());
    gui.settle("the install job to end", |s| {
        s.jobs
            .get(&job.job_id)
            .is_some_and(|j| j.state() == JobState::Done)
    });
    gui.settle("the installed app", |s| {
        let info = cli.get_app(ALPHA).expect("get app");
        info.installed.is_some()
            && app_info(s, ALPHA) == Some(&info)
            && s.jobs == jobs(&cli)
            && folders(&s.folders) == folders(&cli.list_library_folders().expect("folders"))
    });
    {
        let m = &gui.model;
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
    }

    // Uninstall from the App page.
    let job = gui.call("uninstall", |c| c.uninstall(ALPHA, false));
    gui.model.store.add_job(job.clone());
    gui.settle("the uninstalled app", |s| {
        let info = cli.get_app(ALPHA).expect("get app");
        info.installed.is_none()
            && app_info(s, ALPHA) == Some(&info)
            && s.jobs == jobs(&cli)
            && folders(&s.folders) == folders(&cli.list_library_folders().expect("folders"))
    });

    // `ossteam daemon stop`: the window says why the connection ended.
    drop(cli);
    d.stop();
    gui.wait("the connection to close", |g| g.closed.is_some());
    assert_eq!(gui.closed.as_deref(), Some("ossteamd stopped"));
}
