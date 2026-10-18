// SPDX-License-Identifier: GPL-2.0-or-later
//! `ossteam events`: follows the daemon's event stream (SPEC §8.4, §15.2).
//! With `--json` each event is one line of NDJSON (CLI-2).

use std::io::Write;

use clap::ValueEnum;
use ossteam_ipc::v1::{self, Event, LoginPromptKind, NoticeLevel, Topic, event};
use serde_json::{Value, json};

use crate::session::{account_json, persona_name};
use crate::{Cli, Failure, connect, connection_name, exit};

#[derive(Copy, Clone, ValueEnum)]
pub enum TopicArg {
    Daemon,
    Session,
    Library,
    Jobs,
    Games,
    Notices,
}

impl From<TopicArg> for Topic {
    fn from(t: TopicArg) -> Self {
        match t {
            TopicArg::Daemon => Topic::Daemon,
            TopicArg::Session => Topic::Session,
            TopicArg::Library => Topic::Library,
            TopicArg::Jobs => Topic::Jobs,
            TopicArg::Games => Topic::Games,
            TopicArg::Notices => Topic::Notices,
        }
    }
}

pub fn follow(cli: &Cli, topics: &[TopicArg]) -> Result<(), Failure> {
    let client = connect(cli, true)?;
    let topics: Vec<Topic> = if topics.is_empty() {
        TopicArg::value_variants()
            .iter()
            .map(|&t| t.into())
            .collect()
    } else {
        topics.iter().map(|&t| t.into()).collect()
    };
    // DAEMON is always subscribed, so a clean stop can be told apart from
    // a lost connection. Its event is shown only when asked for.
    let show_daemon = topics.contains(&Topic::Daemon);
    client.subscribe(&[topics.as_slice(), &[Topic::Daemon]].concat())?;
    let mut stopping = false;
    while let Some(ev) = client.next_event() {
        if matches!(ev.kind, Some(event::Kind::DaemonStopping(_))) {
            stopping = true;
            if !show_daemon {
                continue;
            }
        }
        if cli.json {
            println!("{}", to_json(&ev));
        } else if !cli.quiet {
            println!("{}", to_text(&ev));
        }
        let _ = std::io::stdout().flush();
    }
    if stopping {
        Ok(())
    } else {
        Err(Failure::new(
            exit::DAEMON_UNAVAILABLE,
            "the connection to the daemon was lost",
        ))
    }
}

fn prompt_kind(k: LoginPromptKind) -> &'static str {
    match k {
        LoginPromptKind::EmailCode => "email_code",
        LoginPromptKind::DeviceCode => "device_code",
        LoginPromptKind::DeviceConfirmation => "device_confirmation",
        LoginPromptKind::QrUrl => "qr_url",
        LoginPromptKind::Unspecified => "unknown",
    }
}

fn level(raw: i32) -> &'static str {
    match NoticeLevel::try_from(raw).unwrap_or(NoticeLevel::Unspecified) {
        NoticeLevel::Info => "info",
        NoticeLevel::Warning => "warning",
        NoticeLevel::Error => "error",
        NoticeLevel::Unspecified => "unknown",
    }
}

pub(crate) fn error_json(e: &v1::Error) -> Value {
    json!({ "code": e.code, "message": e.message, "detail": e.detail })
}

fn to_json(ev: &Event) -> Value {
    match &ev.kind {
        Some(event::Kind::Notice(n)) => {
            json!({ "event": "notice", "level": level(n.level), "text": n.text })
        }
        Some(event::Kind::DaemonStopping(_)) => json!({ "event": "daemon_stopping" }),
        Some(event::Kind::SessionChanged(s)) => json!({
            "event": "session_changed",
            "connection": connection_name(s.connection),
            "account": s.account.as_ref().map(account_json),
            "logged_on": s.logged_on,
            "reason": s.reason,
            "login_required": s.login_required,
            "persona_state": persona_name(s.persona_state),
        }),
        Some(event::Kind::LoginPrompt(p)) => json!({
            "event": "login_prompt",
            "kind": prompt_kind(p.kind()),
            "hint": p.hint,
            "qr_url": p.qr_url,
        }),
        Some(event::Kind::LoginEnded(e)) => json!({
            "event": "login_ended",
            "account": e.account.as_ref().map(account_json),
            "error": e.error.as_ref().map(error_json),
        }),
        Some(event::Kind::AppChanged(c)) => json!({
            "event": "app_changed",
            "app_ids": c.app_ids,
            "complete": c.complete,
        }),
        Some(event::Kind::SettingChanged(c)) => json!({
            "event": "setting_changed",
            "setting": c.setting.as_ref().map(crate::config::setting_json),
        }),
        Some(event::Kind::JobStateChanged(c)) => json!({
            "event": "job_state_changed",
            "job": c.job.as_ref().map(crate::content::job_json),
        }),
        Some(event::Kind::JobProgress(p)) => json!({
            "event": "job_progress",
            "job_id": p.job_id,
            "progress": p.progress.as_ref().map(crate::content::progress_json),
        }),
        Some(event::Kind::GameStarted(g)) => json!({
            "event": "game_started",
            "game": g.game.as_ref().map(crate::games::game_json),
        }),
        Some(event::Kind::GameExited(e)) => {
            let mut v = crate::games::exited_json(e);
            v["event"] = "game_exited".into();
            v
        }
        Some(event::Kind::ShimReport(r)) => {
            let mut v = crate::games::shim_report_json(r);
            v["event"] = "shim_report".into();
            v
        }
        // An event kind newer than this client.
        None => json!({ "event": "unknown" }),
    }
}

fn to_text(ev: &Event) -> String {
    match &ev.kind {
        Some(event::Kind::Notice(n)) => format!("notice   [{}] {}", level(n.level), n.text),
        Some(event::Kind::DaemonStopping(_)) => "daemon   stopping".into(),
        Some(event::Kind::SessionChanged(s)) => {
            let mut line = format!("session  {}", connection_name(s.connection));
            if let Some(a) = &s.account {
                let state = if s.logged_on {
                    "logged on"
                } else {
                    "not logged on"
                };
                line += &format!(", {} {state}", a.account_name);
            }
            if !s.reason.is_empty() {
                line += &format!(" ({})", s.reason);
            }
            if s.login_required {
                line += ", login required";
            }
            line
        }
        Some(event::Kind::LoginPrompt(p)) => {
            let mut line = format!("login    prompt {}", prompt_kind(p.kind()));
            for extra in [&p.hint, &p.qr_url] {
                if !extra.is_empty() {
                    line += &format!(" {extra}");
                }
            }
            line
        }
        Some(event::Kind::LoginEnded(e)) => match (&e.account, &e.error) {
            (_, Some(err)) => format!("login    failed: {}", err.message),
            (Some(a), None) => format!("login    succeeded: {} ({})", a.account_name, a.steam_id),
            (None, None) => "login    ended".into(),
        },
        Some(event::Kind::AppChanged(c)) => {
            const SHOWN: usize = 10;
            let mut ids: Vec<String> = c.app_ids.iter().take(SHOWN).map(u32::to_string).collect();
            if c.app_ids.len() > SHOWN {
                ids.push(format!("and {} more", c.app_ids.len() - SHOWN));
            }
            let n = c.app_ids.len();
            let mut line = format!("library  {n} app{} changed", if n == 1 { "" } else { "s" });
            if !ids.is_empty() {
                line += &format!(": {}", ids.join(" "));
            }
            if !c.complete {
                line += " (still loading)";
            }
            line
        }
        Some(event::Kind::SettingChanged(c)) => match &c.setting {
            Some(s) => format!("setting  {} = {}", s.key, s.value),
            None => "setting  changed".into(),
        },
        Some(event::Kind::JobStateChanged(c)) => match &c.job {
            Some(j) => format!("job      {}", crate::content::job_line(j)),
            None => "job      changed".into(),
        },
        Some(event::Kind::JobProgress(p)) => format!(
            "job      {} {}",
            p.job_id,
            p.progress
                .as_ref()
                .map(crate::content::progress_text)
                .unwrap_or_default()
        ),
        Some(event::Kind::GameStarted(g)) => match &g.game {
            Some(g) => format!("game     app {} started (pid {})", g.app_id, g.pid),
            None => "game     started".into(),
        },
        Some(event::Kind::GameExited(e)) => format!("game     {}", crate::games::exited_text(e)),
        Some(event::Kind::ShimReport(r)) => {
            let who = if r.launched {
                "app"
            } else {
                "a process claiming app"
            };
            format!(
                "game     {who} {} (pid {}): {}",
                r.app_id,
                r.pid,
                crate::games::shim_report_text(r).replace('\n', "\n         ")
            )
        }
        None => "unknown event".into(),
    }
}
