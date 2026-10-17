// SPDX-License-Identifier: GPL-2.0-or-later
//! M1a against the fake CM: the checks of `docs/acceptance/m1a.md` that do
//! not need Steam itself. They show the daemon handles each path as the
//! spec pages describe it. They do not replace the live runs, which check
//! that the spec pages describe Steam correctly.

use std::time::Duration;

use ossteam_ipc::v1::{
    self, ConnectionState, ErrorCode, LoginPromptKind, PersonaState, Topic, event,
};
use ossteam_ipc::{Client, ClientError};
use ossteam_pics::license::flags;
use ossteam_types::{Secret, SteamId};

use super::*;

const ALPHA: u32 = 10;
const ALPHA_DLC: u32 = 11;
const BETA: u32 = 20;
const EXPIRED: u32 = 30;

/// A fake CM with the test library: package 100 grants Alpha and its DLC,
/// 200 grants Beta, and 300 grants an app through an expired license.
fn steam() -> FakeCm {
    FakeCm::new()
        .package(100, &[ALPHA, ALPHA_DLC])
        .package(200, &[BETA])
        .package(300, &[EXPIRED])
        .app(ALPHA, "Alpha", "Game", None)
        .app(ALPHA_DLC, "Alpha Soundtrack", "DLC", Some(ALPHA))
        .app(BETA, "Beta", "Game", None)
        .app(EXPIRED, "Expired", "Game", None)
}

fn licensed(account: Account) -> Account {
    account
        .license(100, 0)
        .license(200, flags::RENEW)
        .license(300, flags::EXPIRED)
}

fn password(name: &str) -> Secret {
    Secret::new(format!("{name}-password"))
}

fn remote_code(r: Result<(), ClientError>) -> ErrorCode {
    match r {
        Err(ClientError::Remote(e)) => e.code,
        other => panic!("expected an error from the daemon, got {other:?}"),
    }
}

fn prompt(c: &Client) -> v1::LoginPrompt {
    wait_event(c, "a login prompt", |k| match k {
        event::Kind::LoginPrompt(p) => Some(p.clone()),
        _ => None,
    })
}

/// Waits for the login to end and returns the account it signed in.
fn ended(c: &Client) -> v1::Account {
    let e = wait_event(c, "the login to end", |k| match k {
        event::Kind::LoginEnded(e) => Some(e.clone()),
        _ => None,
    });
    match (e.account, e.error) {
        (Some(a), None) => a,
        (_, error) => panic!("the login failed: {error:?}"),
    }
}

fn login(d: &TestDaemon, name: &str, remember: bool) -> v1::Account {
    let c = d.subscribed(&[Topic::Session]);
    c.login_with_credentials(name, &password(name), remember)
        .expect("the login starts");
    ended(&c)
}

fn logged_on_as(c: &Client, steam_id: SteamId) -> bool {
    let s = c.get_status().expect("status");
    s.logged_on && s.account.is_some_and(|a| a.steam_id == steam_id.0)
}

/// The library as (name, DLC names), once it is complete.
fn library(c: &Client) -> Vec<(String, Vec<String>)> {
    let mut list = None;
    wait_until("a complete library", || {
        let r = c.list_apps("", false).expect("list apps");
        let done = r.complete && !r.apps.is_empty();
        list = Some(r);
        done
    });
    list.expect("set above")
        .apps
        .into_iter()
        .map(|a| (a.name, a.dlc.into_iter().map(|d| d.name).collect()))
        .collect()
}

fn expected_library() -> Vec<(String, Vec<String>)> {
    vec![
        ("Alpha".into(), vec!["Alpha Soundtrack".into()]),
        ("Beta".into(), vec![]),
    ]
}

#[test]
fn credential_login_lists_the_library_and_resumes_after_a_restart() {
    let alice = licensed(Account::new("alice", 1001));
    let cm = steam().account(alice.clone());
    let root = Root::new();

    let d = TestDaemon::start(&root, &cm);
    let account = login(&d, "alice", true);
    assert_eq!(account.steam_id, alice.steam_id.0);
    assert_eq!(account.account_name, "alice");
    let c = d.client();
    wait_until("the logon", || logged_on_as(&c, alice.steam_id));
    // The expired license grants nothing; the DLC is under its parent.
    assert_eq!(library(&c), expected_library());
    assert_eq!(
        c.get_status().unwrap().connection,
        ConnectionState::Connected as i32
    );
    let accounts = c.list_accounts().unwrap();
    assert_eq!(accounts.active, alice.steam_id.0);
    assert_eq!(accounts.accounts.len(), 1);
    drop(c);
    d.stop();

    // The stored refresh token logs on again, with no login.
    let d = TestDaemon::start(&root, &cm);
    let c = d.client();
    wait_until("the resumed logon", || logged_on_as(&c, alice.steam_id));
    assert_eq!(cm.logons(), ["alice", "alice"]);
    assert_eq!(library(&c), expected_library());
    let persona = c.get_status().unwrap().account.unwrap().persona_name;
    assert_eq!(persona, "alice persona");
}

#[test]
fn email_code_and_remembered_machine() {
    let frank = Account::new("frank", 1002).guard(Guard::EmailCode("12345".into()));
    let cm = steam().account(frank.clone());
    let root = Root::new();
    let d = TestDaemon::start(&root, &cm);
    let c = d.subscribed(&[Topic::Session]);

    c.login_with_credentials("frank", &password("frank"), true)
        .unwrap();
    let p = prompt(&c);
    assert_eq!(p.kind, LoginPromptKind::EmailCode as i32);
    assert_eq!(p.hint, "example.com");
    // A wrong code is refused and the login goes on (exit code 4).
    let wrong = c.submit_guard_code(&Secret::new("99999".into()), LoginPromptKind::EmailCode);
    assert_eq!(remote_code(wrong), ErrorCode::NotLoggedIn);
    c.submit_guard_code(&Secret::new("12345".into()), LoginPromptKind::EmailCode)
        .unwrap();
    assert_eq!(ended(&c).steam_id, frank.steam_id.0);

    // The guard data from that sign-in spares the next one the code.
    c.login_with_credentials("frank", &password("frank"), true)
        .unwrap();
    let e = wait_event(&c, "the second login", |k| match k {
        event::Kind::LoginPrompt(p) => Some(Err(p.clone())),
        event::Kind::LoginEnded(e) => Some(Ok(e.clone())),
        _ => None,
    });
    let e = e.expect("no prompt the second time");
    assert!(e.error.is_none(), "{:?}", e.error);
}

#[test]
fn mobile_authenticator_code() {
    let erin = Account::new("erin", 1003).guard(Guard::DeviceCode("ABCDE".into()));
    let cm = steam().account(erin.clone());
    let root = Root::new();
    let d = TestDaemon::start(&root, &cm);
    let c = d.subscribed(&[Topic::Session]);

    c.login_with_credentials("erin", &password("erin"), true)
        .unwrap();
    assert_eq!(prompt(&c).kind, LoginPromptKind::DeviceCode as i32);
    let wrong = c.submit_guard_code(&Secret::new("XXXXX".into()), LoginPromptKind::DeviceCode);
    assert_eq!(remote_code(wrong), ErrorCode::NotLoggedIn);
    // With one kind of code on offer, the kind may be left out.
    c.submit_guard_code(&Secret::new("ABCDE".into()), LoginPromptKind::Unspecified)
        .unwrap();
    assert_eq!(ended(&c).steam_id, erin.steam_id.0);
}

#[test]
fn approval_in_the_mobile_app() {
    let carol = Account::new("carol", 1004).guard(Guard::DeviceConfirmation);
    let cm = steam().account(carol.clone());
    let root = Root::new();
    let d = TestDaemon::start(&root, &cm);
    let c = d.subscribed(&[Topic::Session]);

    c.login_with_credentials("carol", &password("carol"), true)
        .unwrap();
    assert_eq!(prompt(&c).kind, LoginPromptKind::DeviceConfirmation as i32);
    cm.approve("carol");
    assert_eq!(ended(&c).steam_id, carol.steam_id.0);
    wait_until("the logon", || logged_on_as(&c, carol.steam_id));
}

#[test]
fn qr_login_with_a_rotated_challenge() {
    let dave = licensed(Account::new("dave", 1005));
    let cm = steam().account(dave.clone()).qr_rotations(1);
    let root = Root::new();
    let d = TestDaemon::start(&root, &cm);
    let c = d.subscribed(&[Topic::Session]);

    c.login_with_qr(true).unwrap();
    let first = prompt(&c);
    assert_eq!(first.kind, LoginPromptKind::QrUrl as i32);
    let second = prompt(&c);
    assert_eq!(second.kind, LoginPromptKind::QrUrl as i32);
    assert_ne!(first.qr_url, second.qr_url);
    cm.approve("dave");
    // The account is learnt from Steam at the end.
    let account = ended(&c);
    assert_eq!(account.steam_id, dave.steam_id.0);
    assert_eq!(account.account_name, "dave");
    wait_until("the logon", || logged_on_as(&c, dave.steam_id));
    assert_eq!(library(&c), expected_library());
}

#[test]
fn wrong_password_fails_with_not_logged_in() {
    let cm = steam().account(Account::new("alice", 1001));
    let root = Root::new();
    let d = TestDaemon::start(&root, &cm);
    let c = d.subscribed(&[Topic::Session]);

    let r = c.login_with_credentials("alice", &Secret::new("nope".into()), true);
    assert_eq!(remote_code(r), ErrorCode::NotLoggedIn);
    let s = c.get_status().unwrap();
    assert!(!s.logged_on);
    assert!(s.account.is_none());
    assert!(c.list_accounts().unwrap().accounts.is_empty());
}

#[test]
fn no_remember_leaves_no_stored_token() {
    let alice = licensed(Account::new("alice", 1001));
    let cm = steam().account(alice.clone());
    let root = Root::new();
    let d = TestDaemon::start(&root, &cm);

    login(&d, "alice", false);
    let c = d.client();
    wait_until("the logon", || logged_on_as(&c, alice.steam_id));
    assert!(c.list_accounts().unwrap().accounts.is_empty());
    assert!(cm.has_token("alice"), "Steam issued a token");
    // Every token the fake issues is a JWT, so it starts with `eyJ`.
    let stored = std::fs::read_to_string(root.secrets_file()).unwrap_or_default();
    assert!(!stored.contains("eyJ"), "a token was stored: {stored}");
    drop(c);
    d.stop();

    let d = TestDaemon::start(&root, &cm);
    let c = d.client();
    std::thread::sleep(Duration::from_millis(500));
    let s = c.get_status().unwrap();
    assert!(s.account.is_none() && !s.logged_on);
    assert_eq!(cm.logons(), ["alice"]);
}

#[test]
fn a_revoked_token_stops_retrying_and_asks_for_a_login() {
    let alice = licensed(Account::new("alice", 1001));
    let cm = steam().account(alice.clone());
    let root = Root::new();
    let d = TestDaemon::start(&root, &cm);
    login(&d, "alice", true);
    d.stop();

    cm.revoke("alice");
    let d = TestDaemon::start(&root, &cm);
    let c = d.client();
    wait_until("the token to be forgotten", || {
        let s = c.get_status().unwrap();
        s.account.is_none() && s.session_reason.contains("log in again")
    });
    assert!(c.list_accounts().unwrap().accounts.is_empty());
    let stored = std::fs::read_to_string(root.secrets_file()).unwrap_or_default();
    assert!(
        !stored.contains("eyJ"),
        "the token is still stored: {stored}"
    );
    let attempts = cm.logons().len();
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(cm.logons().len(), attempts, "the daemon kept retrying");
    drop(c);
    d.stop();

    // A later start has nothing to resume.
    let d = TestDaemon::start(&root, &cm);
    let c = d.client();
    std::thread::sleep(Duration::from_millis(500));
    assert!(c.get_status().unwrap().account.is_none());
    assert_eq!(cm.logons().len(), attempts);
}

#[test]
fn a_dropped_connection_reconnects() {
    let alice = licensed(Account::new("alice", 1001));
    let cm = steam().account(alice.clone());
    let root = Root::new();
    let d = TestDaemon::start(&root, &cm);
    login(&d, "alice", true);
    let c = d.subscribed(&[Topic::Session]);
    wait_until("the logon", || logged_on_as(&c, alice.steam_id));

    cm.drop_connections();
    wait_event(&c, "the daemon to notice", |k| match k {
        event::Kind::SessionChanged(s)
            if s.connection == ConnectionState::Connecting as i32 && !s.logged_on =>
        {
            Some(())
        }
        _ => None,
    });
    wait_event(&c, "the reconnection", |k| match k {
        event::Kind::SessionChanged(s)
            if s.connection == ConnectionState::Connected as i32 && s.logged_on =>
        {
            Some(())
        }
        _ => None,
    });
    assert_eq!(cm.logons(), ["alice", "alice"]);
    assert_eq!(library(&c), expected_library());
}

#[test]
fn two_remembered_accounts_and_a_switch() {
    let alice = licensed(Account::new("alice", 1001));
    let bob = Account::new("bob", 1006).license(200, 0);
    let cm = steam().account(alice.clone()).account(bob.clone());
    let root = Root::new();
    let d = TestDaemon::start(&root, &cm);

    login(&d, "alice", true);
    login(&d, "bob", true);
    let c = d.client();
    wait_until("bob's logon", || logged_on_as(&c, bob.steam_id));
    let accounts = c.list_accounts().unwrap();
    assert_eq!(accounts.active, bob.steam_id.0);
    let mut names: Vec<_> = accounts
        .accounts
        .iter()
        .map(|a| a.account_name.as_str())
        .collect();
    names.sort_unstable();
    assert_eq!(names, ["alice", "bob"]);
    assert_eq!(library(&c), [("Beta".to_owned(), vec![])]);

    let switched = c.switch_account(alice.steam_id.0).unwrap();
    assert_eq!(switched.steam_id, alice.steam_id.0);
    assert!(logged_on_as(&c, alice.steam_id));
    wait_until("alice's library", || {
        let r = c.list_apps("", false).unwrap();
        r.complete && r.apps.len() == 2
    });
    assert_eq!(library(&c), expected_library());
    assert_eq!(c.list_accounts().unwrap().active, alice.steam_id.0);
}

#[test]
fn change_polling_picks_up_an_app_update() {
    let alice = licensed(Account::new("alice", 1001));
    let cm = steam().account(alice.clone());
    let root = Root::new();
    let d = TestDaemon::start_with(&root, &cm, Duration::from_millis(200));
    login(&d, "alice", true);
    let c = d.subscribed(&[Topic::Library]);
    assert_eq!(library(&c), expected_library());

    cm.rename_app(ALPHA, "Alpha Remastered");
    // Events from the first fetch may still be queued; the one that
    // matters names Alpha, and by then GetApp has the new info.
    let name = |c: &Client| c.get_app(ALPHA).unwrap().app.unwrap().name;
    wait_event(&c, "the change to arrive", |k| match k {
        event::Kind::AppChanged(a)
            if a.app_ids.contains(&ALPHA) && name(&c) == "Alpha Remastered" =>
        {
            Some(())
        }
        _ => None,
    });
    assert_eq!(library(&c)[0].0, "Alpha Remastered");
}

#[test]
fn persona_state_reaches_steam() {
    let alice = licensed(Account::new("alice", 1001));
    let cm = steam().account(alice.clone());
    let root = Root::new();
    let d = TestDaemon::start(&root, &cm);
    login(&d, "alice", true);
    let c = d.client();
    wait_until("the logon", || logged_on_as(&c, alice.steam_id));

    // Steam's numbers: online 1, invisible 7, offline 0 (`pics.md` §8).
    for (state, steam) in [
        (PersonaState::Invisible, 7),
        (PersonaState::Offline, 0),
        (PersonaState::Online, 1),
    ] {
        c.set_persona_state(state).unwrap();
        cm.wait(WAIT, "the persona state", |s| {
            s.persona_states().last() == Some(&(alice.steam_id, steam))
        });
    }
    assert_eq!(
        c.get_status().unwrap().persona_state,
        PersonaState::Online as i32
    );
}
