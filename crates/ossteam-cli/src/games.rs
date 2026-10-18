// SPDX-License-Identifier: GPL-2.0-or-later
//! `launch`, `running` and `stop` (SPEC §15.2, `launch.md` §8).

use std::io::{IsTerminal, Write};
use std::time::{Duration, Instant};

use ossteam_ipc::Client;
use ossteam_ipc::v1::{self, Topic, event, game_exited};
use serde_json::{Value, json};

use crate::library::resolve;
use crate::{Cli, Failure, connect, exit};

/// How long `stop` waits for the game to end: the daemon's 10 s before it
/// kills, and some room for the kill.
const STOP_WAIT: Duration = Duration::from_secs(20);

pub fn launch(
    cli: &Cli,
    app: &str,
    option: Option<u32>,
    wait: bool,
    allow_stale: bool,
    args: &[String],
) -> Result<(), Failure> {
    let client = connect(cli, true)?;
    let id = resolve(&client, app)?;
    let option = match option {
        Some(o) => Some(o),
        None => choose(cli, &client, id)?,
    };
    if wait {
        client.subscribe(&[Topic::Games, Topic::Daemon])?;
    }
    let game = client.launch(id, option, args, allow_stale)?;
    if cli.json && !wait {
        println!("{}", game_json(&game));
    } else if !cli.quiet && !cli.json {
        println!(
            "launched app {} (option {}, pid {}); its output goes to {}",
            game.app_id, game.option, game.pid, game.log
        );
    }
    if wait {
        let exited = wait_exit(&client, id, None)?;
        if cli.json {
            println!(
                "{}",
                json!({ "game": game_json(&game), "exited": exited_json(&exited) })
            );
        } else if !cli.quiet {
            println!("{}", exited_text(&exited));
        }
    }
    Ok(())
}

/// The option to launch when the user gave none: `None` when exactly one
/// qualifies, otherwise the one the user picks (LAUNCH-1).
fn choose(cli: &Cli, client: &Client, app: u32) -> Result<Option<u32>, Failure> {
    let r = client.get_launch_options(app)?;
    let qualifying: Vec<&v1::LaunchOption> = r.options.iter().filter(|o| o.qualifies).collect();
    if qualifying.len() <= 1 {
        // The daemon reports the case of none itself.
        return Ok(None);
    }
    let list: Vec<String> = qualifying
        .iter()
        .map(|o| {
            let text = if o.description.is_empty() {
                o.executable.as_str()
            } else {
                o.description.as_str()
            };
            let kind = if o.r#type.is_empty() {
                String::new()
            } else {
                format!(" [{}]", o.r#type)
            };
            format!("  {}  {text}{kind}", o.index)
        })
        .collect();
    if cli.json || !std::io::stdin().is_terminal() {
        return Err(Failure::new(
            exit::OTHER,
            format!(
                "app {app} has several launch options; choose one with --option:\n{}",
                list.join("\n")
            ),
        ));
    }
    eprintln!("app {app} has several launch options:\n{}", list.join("\n"));
    loop {
        eprint!("option: ");
        let _ = std::io::stderr().flush();
        let mut line = String::new();
        let n = std::io::stdin()
            .read_line(&mut line)
            .map_err(|e| Failure::new(exit::OTHER, format!("reading standard input: {e}")))?;
        if n == 0 {
            return Err(Failure::new(exit::OTHER, "no launch option chosen"));
        }
        match line.trim().parse::<u32>() {
            Ok(i) if qualifying.iter().any(|o| o.index == i) => return Ok(Some(i)),
            _ => eprintln!("{:?} is not one of the options above", line.trim()),
        }
    }
}

pub fn running(cli: &Cli) -> Result<(), Failure> {
    let client = connect(cli, true)?;
    let games = client.list_running()?;
    if cli.json {
        println!(
            "{}",
            json!({ "games": games.iter().map(game_json).collect::<Vec<_>>() })
        );
        return Ok(());
    }
    if cli.quiet {
        return Ok(());
    }
    if games.is_empty() {
        println!("no games are running");
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    for g in &games {
        let name = client
            .get_app(g.app_id)
            .ok()
            .and_then(|r| r.app)
            .map(|a| a.name)
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| format!("app {}", g.app_id));
        println!(
            "{:>8}  {name}  pid {}, for {}",
            g.app_id,
            g.pid,
            crate::format_uptime(now.saturating_sub(g.started))
        );
    }
    Ok(())
}

pub fn stop(cli: &Cli, app: &str) -> Result<(), Failure> {
    let client = connect(cli, true)?;
    let id = resolve(&client, app)?;
    client.subscribe(&[Topic::Games, Topic::Daemon])?;
    client.stop_game(id)?;
    if !cli.quiet && !cli.json && std::io::stderr().is_terminal() {
        eprintln!("asked app {id} to close; waiting for it");
    }
    let exited = wait_exit(&client, id, Some(STOP_WAIT))?;
    if cli.json {
        println!("{}", exited_json(&exited));
    } else if !cli.quiet {
        println!("{}", exited_text(&exited));
    }
    Ok(())
}

/// Waits for the app's `GameExited`.
fn wait_exit(
    client: &Client,
    app: u32,
    limit: Option<Duration>,
) -> Result<v1::GameExited, Failure> {
    let deadline = limit.map(|l| Instant::now() + l);
    loop {
        let ev = match deadline {
            None => client.events().recv().ok(),
            Some(d) => client.events().recv_deadline(d).ok(),
        };
        let Some(ev) = ev else {
            if client.is_closed() {
                return Err(Failure::new(
                    exit::DAEMON_UNAVAILABLE,
                    "the daemon went away",
                ));
            }
            return Err(Failure::new(
                exit::OTHER,
                format!("app {app} did not exit in time"),
            ));
        };
        match ev.kind {
            Some(event::Kind::GameExited(e)) if e.app_id == app => return Ok(e),
            Some(event::Kind::DaemonStopping(_)) => {
                return Err(Failure::new(
                    exit::DAEMON_UNAVAILABLE,
                    "the daemon is stopping; the game keeps running without it",
                ));
            }
            _ => {}
        }
    }
}

pub fn game_json(g: &v1::RunningGame) -> Value {
    json!({
        "app_id": g.app_id,
        "option": g.option,
        "pid": g.pid,
        "started": g.started,
        "log": g.log,
    })
}

pub fn exited_json(e: &v1::GameExited) -> Value {
    let (code, signal) = match e.status {
        Some(game_exited::Status::ExitCode(c)) => (Some(c), None),
        Some(game_exited::Status::Signal(s)) => (None, Some(s)),
        None => (None, None),
    };
    json!({ "app_id": e.app_id, "exit_code": code, "signal": signal, "seconds": e.seconds })
}

pub fn exited_text(e: &v1::GameExited) -> String {
    let how = match e.status {
        Some(game_exited::Status::ExitCode(c)) => format!("with exit code {c}"),
        Some(game_exited::Status::Signal(s)) => format!("by signal {s}"),
        None => "for a reason this client does not know".into(),
    };
    format!(
        "app {} exited {how} after {}",
        e.app_id,
        crate::format_uptime(e.seconds)
    )
}
