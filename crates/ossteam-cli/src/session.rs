// SPDX-License-Identifier: GPL-2.0-or-later
//! `login`, `logout` and `account` (SPEC §15.2; `docs/spec/ipc.md` §5.2).

use std::io::{BufRead, IsTerminal, Write};

use crossbeam_channel::select;
use ossteam_ipc::v1::{self, LoginPromptKind, Topic, event};
use ossteam_ipc::{Client, ClientError, RemoteError};
use ossteam_types::Secret;
use serde_json::json;

use crate::{Cli, Failure, connect, exit};

pub fn login(
    cli: &Cli,
    qr: bool,
    username: Option<&str>,
    no_remember: bool,
) -> Result<(), Failure> {
    if !qr && !std::io::stdin().is_terminal() {
        return Err(Failure::new(
            exit::OTHER,
            "login reads the password from a terminal; run it in one, or use --qr",
        ));
    }
    let client = connect(cli, true)?;
    // Subscribe first, so no prompt can arrive unseen.
    client.subscribe(&[Topic::Session])?;
    let remember = !no_remember;
    if qr {
        client.login_with_qr(remember)?;
    } else {
        let username = match username {
            Some(u) => u.to_owned(),
            None => prompt_line("Steam account name: ")?,
        };
        let password = Secret::new(
            rpassword::prompt_password("Password: ")
                .map_err(|e| Failure::new(exit::OTHER, format!("reading the password: {e}")))?,
        );
        // `password` is zeroed when dropped, and the client library zeroes
        // its copies once they are sent (IPC-10).
        client.login_with_credentials(&username, &password, remember)?;
    }
    follow_login(cli, &client)
}

/// Answers prompts until the daemon reports the end of the login.
fn follow_login(cli: &Cli, client: &Client) -> Result<(), Failure> {
    let (lines_tx, lines) = crossbeam_channel::unbounded::<String>();
    let mut reading_codes = false;
    let mut code_kind = LoginPromptKind::Unspecified;
    loop {
        select! {
            recv(client.events()) -> ev => {
                let Ok(ev) = ev else {
                    return Err(Failure::new(
                        exit::DAEMON_UNAVAILABLE,
                        "the daemon closed the connection during the login",
                    ));
                };
                match ev.kind {
                    Some(event::Kind::LoginPrompt(p)) => {
                        let kind = p.kind();
                        show_prompt(cli, &p);
                        if matches!(kind, LoginPromptKind::EmailCode | LoginPromptKind::DeviceCode) {
                            code_kind = kind;
                            if !reading_codes {
                                reading_codes = true;
                                spawn_line_reader(lines_tx.clone());
                            }
                            eprint!("Code: ");
                            let _ = std::io::stderr().flush();
                        }
                    }
                    Some(event::Kind::LoginEnded(end)) => return login_ended(cli, end),
                    _ => {}
                }
            },
            recv(lines) -> line => {
                // `lines_tx` lives in this function, so this never fails.
                let Ok(line) = line else { continue };
                let code = Secret::new(line);
                if code.expose().trim().is_empty() {
                    eprint!("Code: ");
                    let _ = std::io::stderr().flush();
                    continue;
                }
                if let Err(e) = client.submit_guard_code(&code, code_kind) {
                    let _ = client.cancel_login();
                    return Err(e.into());
                }
                eprintln!("Checking the code...");
            },
        }
    }
}

fn show_prompt(cli: &Cli, p: &v1::LoginPrompt) {
    match p.kind() {
        LoginPromptKind::EmailCode if !p.hint.is_empty() => {
            eprintln!("Steam emailed a code to your address at {}.", p.hint);
        }
        LoginPromptKind::EmailCode => eprintln!("Steam emailed you a code."),
        LoginPromptKind::DeviceCode => {
            eprintln!("Enter the code from the Steam mobile app.");
        }
        LoginPromptKind::DeviceConfirmation if p.hint == "confirm in email" => {
            eprintln!("Confirm the sign-in through the link Steam emailed you.");
        }
        LoginPromptKind::DeviceConfirmation => {
            eprintln!("Approve the sign-in in the Steam mobile app.");
        }
        LoginPromptKind::QrUrl => {
            eprintln!("Scan this code with the Steam mobile app:");
            eprint!("{}", render_qr(&p.qr_url, crate::stderr_color(cli)));
            eprintln!("{}", p.qr_url);
        }
        LoginPromptKind::Unspecified => {}
    }
}

fn login_ended(cli: &Cli, end: v1::LoginEnded) -> Result<(), Failure> {
    if let Some(err) = end.error {
        return Err(ClientError::Remote(RemoteError::from(err)).into());
    }
    let Some(a) = end.account else {
        return Err(Failure::new(
            exit::OTHER,
            "the daemon reported the login over without an outcome",
        ));
    };
    if cli.json {
        println!("{}", json!({ "account": account_json(&a) }));
    } else if !cli.quiet {
        println!("logged in as {} ({})", a.account_name, a.steam_id);
    }
    Ok(())
}

pub fn logout(cli: &Cli) -> Result<(), Failure> {
    let client = connect(cli, true)?;
    client.logout()?;
    if cli.json {
        println!("{}", json!({ "logged_out": true }));
    } else if !cli.quiet {
        println!("logged out");
    }
    Ok(())
}

pub fn account_list(cli: &Cli) -> Result<(), Failure> {
    let client = connect(cli, true)?;
    let list = client.list_accounts()?;
    if cli.json {
        let accounts: Vec<_> = list
            .accounts
            .iter()
            .map(|a| {
                let mut v = account_json(a);
                v["active"] = json!(a.steam_id == list.active);
                v
            })
            .collect();
        println!("{}", json!({ "accounts": accounts }));
        return Ok(());
    }
    if cli.quiet {
        return Ok(());
    }
    if list.accounts.is_empty() {
        println!("no remembered accounts");
        return Ok(());
    }
    let width = list
        .accounts
        .iter()
        .map(|a| a.account_name.len())
        .max()
        .unwrap_or(0)
        .max("ACCOUNT".len());
    println!("  {:<17}  {:<width$}  PERSONA", "STEAM ID", "ACCOUNT");
    for a in &list.accounts {
        let mark = if a.steam_id == list.active { '*' } else { ' ' };
        println!(
            "{mark} {:<17}  {:<width$}  {}",
            a.steam_id, a.account_name, a.persona_name
        );
    }
    Ok(())
}

pub fn account_switch(cli: &Cli, who: &str) -> Result<(), Failure> {
    let client = connect(cli, true)?;
    let list = client.list_accounts()?;
    let target = list
        .accounts
        .iter()
        .find(|a| who.parse::<u64>().is_ok_and(|id| id == a.steam_id))
        .or_else(|| {
            // Steam account names are case-insensitive.
            list.accounts
                .iter()
                .find(|a| a.account_name.eq_ignore_ascii_case(who))
        })
        .ok_or_else(|| {
            Failure::new(
                exit::NOT_FOUND,
                format!("no remembered account matches {who:?}; see 'ossteam account list'"),
            )
        })?;
    let a = client.switch_account(target.steam_id)?;
    if cli.json {
        println!("{}", json!({ "account": account_json(&a) }));
    } else if !cli.quiet {
        println!("switched to {} ({})", a.account_name, a.steam_id);
    }
    Ok(())
}

pub fn account_json(a: &v1::Account) -> serde_json::Value {
    json!({
        "steam_id": a.steam_id.to_string(),
        "account_name": a.account_name,
        "persona_name": a.persona_name,
    })
}

fn prompt_line(prompt: &str) -> Result<String, Failure> {
    eprint!("{prompt}");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|e| Failure::new(exit::OTHER, format!("reading standard input: {e}")))?;
    let line = line.trim().to_owned();
    if line.is_empty() {
        return Err(Failure::new(exit::OTHER, "no account name given"));
    }
    Ok(line)
}

/// Reads lines from standard input on a thread of its own, so the login
/// can finish through the mobile app while a code prompt waits. The
/// thread ends with the process.
fn spawn_line_reader(tx: crossbeam_channel::Sender<String>) {
    let _ = std::thread::Builder::new()
        .name("stdin".into())
        .spawn(move || {
            for line in std::io::stdin().lock().lines() {
                let Ok(line) = line else { return };
                if tx.send(line).is_err() {
                    return;
                }
            }
        });
}

/// The QR code drawn with half-block characters, two modules per
/// character cell. With colour on, it is forced to black on white, so it
/// scans the same on light and dark terminals.
fn render_qr(url: &str, color: bool) -> String {
    use qrcode::render::unicode::Dense1x2;
    let Ok(code) = qrcode::QrCode::new(url.as_bytes()) else {
        return String::new();
    };
    let art = code.render::<Dense1x2>().build();
    let mut out = String::new();
    for line in art.lines() {
        if color {
            out.push_str("\x1b[30;47m");
            out.push_str(line);
            out.push_str("\x1b[0m\n");
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qr_renders_with_a_quiet_zone() {
        let plain = render_qr("https://s.team/q/1/1234567890", false);
        let lines: Vec<_> = plain.lines().collect();
        assert!(lines.len() > 10);
        // The quiet zone makes the first row blank.
        assert!(lines[0].chars().all(|c| c == ' '));
        assert!(!plain.contains('\x1b'));
        let colored = render_qr("https://s.team/q/1/1234567890", true);
        assert!(colored.starts_with("\x1b[30;47m"));
    }
}
