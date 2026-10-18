// SPDX-License-Identifier: GPL-2.0-or-later
//! `ossteam`, the command-line client (SPEC §15). It talks only to the
//! daemon (LEAF-1).

mod config;
mod content;
mod events;
mod games;
mod library;
mod session;

use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use ossteam_ipc::v1::{self, ConnectionState, ErrorCode, PeerKind};
use ossteam_ipc::{Client, ClientError, ClientInfo, ConnectOptions};
use serde_json::json;

/// Exit codes (SPEC §15.3).
mod exit {
    pub const OTHER: u8 = 1;
    pub const DAEMON_UNAVAILABLE: u8 = 3;
    pub const NOT_LOGGED_IN: u8 = 4;
    pub const NOT_FOUND: u8 = 5;
    pub const NOT_OWNED: u8 = 6;
    pub const UNSUPPORTED: u8 = 7;
    pub const CONFLICT: u8 = 8;
    pub const NETWORK: u8 = 9;
}

/// How long `daemon stop` waits for the daemon to exit after it accepts the
/// request.
const STOP_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Parser)]
#[command(
    name = "ossteam",
    version,
    about = "Open-source Steam client",
    propagate_version = true
)]
struct Cli {
    /// Print one JSON document (NDJSON for streaming commands).
    #[arg(long, global = true)]
    json: bool,
    /// Print only errors.
    #[arg(short, long, global = true)]
    quiet: bool,
    /// Do not start the daemon if it is not running.
    #[arg(long, global = true)]
    no_autostart: bool,
    #[arg(long, global = true, value_enum, default_value_t = Color::Auto)]
    color: Color,
    /// More diagnostic output.
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    verbose: u8,
    #[command(subcommand)]
    command: Command,
}

#[derive(Copy, Clone, ValueEnum)]
enum Color {
    Auto,
    Always,
    Never,
}

#[derive(Subcommand)]
enum Command {
    /// Sign in to Steam. Prompts on the terminal for the password and any
    /// Steam Guard code.
    Login {
        /// Sign in by scanning a QR code with the Steam mobile app.
        #[arg(long, conflicts_with = "username")]
        qr: bool,
        /// The Steam account name; asked for when not given.
        #[arg(long)]
        username: Option<String>,
        /// Do not store the sign-in: it ends when the daemon stops.
        #[arg(long)]
        no_remember: bool,
    },
    /// Log off and forget the active account's stored sign-in.
    Logout,
    /// Remembered accounts.
    Account {
        #[command(subcommand)]
        action: Option<AccountAction>,
    },
    /// Daemon, connection, account, jobs and running games.
    Status,
    /// Set what friends see.
    Persona {
        #[arg(value_enum)]
        state: session::PersonaArg,
    },
    /// List the apps the account's licenses grant, with DLC under its game.
    Library {
        /// Only installed apps.
        #[arg(long)]
        installed: bool,
        /// Only apps whose name contains this text, or whose id it is.
        #[arg(long)]
        filter: Option<String>,
    },
    /// Show an app's info: depots, branches, install state and launch
    /// options.
    Info {
        /// App id, or the exact name of an app in the library.
        app: String,
    },
    /// Install an app and its DLC into a library folder.
    Install {
        /// App id, or the exact name of an app in the library.
        app: String,
        /// The library folder; the default folder when not given.
        #[arg(long)]
        folder: Option<PathBuf>,
        /// The branch to install from; "public" when not given.
        #[arg(long)]
        branch: Option<String>,
        /// Wait until the install has finished.
        #[arg(long)]
        wait: bool,
    },
    /// Update installed apps.
    #[command(group = clap::ArgGroup::new("which").required(true).args(["apps", "all"]))]
    Update {
        /// App ids or names.
        apps: Vec<String>,
        /// Every installed app that has an update.
        #[arg(long)]
        all: bool,
        /// Wait until the updates have finished.
        #[arg(long)]
        wait: bool,
    },
    /// Check an installed app's files and repair the ones that differ.
    Verify {
        app: String,
        /// Wait until the verify has finished.
        #[arg(long)]
        wait: bool,
    },
    /// Delete an installed app's files.
    Uninstall {
        app: String,
        /// Do not ask first.
        #[arg(long, short)]
        yes: bool,
        /// Linux: also delete the app's compatibility tool prefix, which
        /// holds its saves and settings under Proton.
        #[arg(long)]
        remove_compat_data: bool,
        /// Wait until the files are deleted.
        #[arg(long)]
        wait: bool,
    },
    /// Install, update, verify and uninstall jobs.
    Jobs {
        #[command(subcommand)]
        action: Option<JobsAction>,
    },
    /// Start an installed app.
    Launch {
        /// App id, or the exact name of an app in the library.
        app: String,
        /// The launch option's index, as `info` lists them. Asked for when
        /// the app has several.
        #[arg(long)]
        option: Option<u32>,
        /// Wait until the game has exited.
        #[arg(long)]
        wait: bool,
        /// Launch even though an update is available.
        #[arg(long)]
        allow_stale: bool,
        /// Arguments added to the launch option's own.
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// List the games launched through ossteam that are running.
    Running,
    /// Ask a running game to close, and kill it if it does not.
    Stop {
        /// App id, or the exact name of an app in the library.
        app: String,
    },
    /// Library folders, where apps are installed.
    Folders {
        #[command(subcommand)]
        action: Option<FoldersAction>,
    },
    /// Show and change daemon settings. Values set here override
    /// ossteamd.toml.
    Config {
        #[command(subcommand)]
        action: Option<ConfigAction>,
    },
    /// Print daemon events as they happen.
    Events {
        /// Only these topics (repeatable). All topics by default.
        #[arg(long = "topic", value_enum)]
        topics: Vec<events::TopicArg>,
    },
    /// Control the ossteamd daemon.
    Daemon {
        #[command(subcommand)]
        action: Option<DaemonAction>,
    },
    /// Print a shell completion script.
    Completions { shell: clap_complete::Shell },
}

#[derive(Subcommand)]
enum AccountAction {
    /// List remembered accounts (the default). `*` marks the active one.
    List,
    /// Log off the active account and log on as another remembered one.
    Switch {
        /// Steam id or account name.
        account: String,
    },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// List every setting with its value and where it comes from (the
    /// default).
    List,
    /// Print one setting's value.
    Get { key: String },
    /// Override a setting.
    Set { key: String, value: String },
    /// Remove an override, so ossteamd.toml or the default applies again.
    Unset { key: String },
}

#[derive(Subcommand)]
enum JobsAction {
    /// List the active account's jobs, unfinished ones first (the default).
    List,
    /// Pause a job; a running one stops and continues later from where it
    /// was.
    Pause { job: u64 },
    /// Queue a paused or failed job again.
    Resume { job: u64 },
    /// End a job that has not finished.
    Cancel { job: u64 },
    /// Change a job's priority. Higher runs first.
    Priority {
        job: u64,
        #[arg(allow_negative_numbers = true)]
        priority: i32,
    },
    /// Print job events as they happen.
    Watch,
}

#[derive(Subcommand)]
enum FoldersAction {
    /// List the library folders (the default).
    List,
    /// Add a library folder; the directory is created if needed.
    Add { path: PathBuf },
    /// Remove a library folder from the list. Its files stay.
    Remove { path: PathBuf },
}

#[derive(Subcommand)]
enum DaemonAction {
    /// Start the daemon if it is not running.
    Start,
    /// Stop the daemon.
    Stop {
        /// Stop even while games launched through ossteam are running.
        #[arg(long)]
        force: bool,
    },
    /// Show whether the daemon is running (the default).
    Status,
}

struct Failure {
    code: u8,
    /// `None` when the command already reported the problem itself.
    message: Option<String>,
}

impl Failure {
    fn new(code: u8, message: impl Into<String>) -> Self {
        Self {
            code,
            message: Some(message.into()),
        }
    }
}

impl From<ClientError> for Failure {
    fn from(e: ClientError) -> Self {
        let code = match &e {
            ClientError::Unavailable { .. } | ClientError::StartFailed(_) => {
                exit::DAEMON_UNAVAILABLE
            }
            ClientError::Remote(r) => exit_code(r.code),
            _ => exit::OTHER,
        };
        let mut message = e.to_string();
        if let ClientError::Remote(r) = &e
            && !r.detail.is_empty()
        {
            message = format!("{message} ({})", r.detail);
        }
        Self {
            code,
            message: Some(message),
        }
    }
}

/// The exit code for a daemon error (SPEC §15.3).
fn exit_code(code: ErrorCode) -> u8 {
    match code {
        ErrorCode::NotLoggedIn => exit::NOT_LOGGED_IN,
        ErrorCode::NotFound => exit::NOT_FOUND,
        ErrorCode::NotOwned => exit::NOT_OWNED,
        ErrorCode::Unsupported => exit::UNSUPPORTED,
        ErrorCode::Conflict => exit::CONFLICT,
        ErrorCode::Network => exit::NETWORK,
        _ => exit::OTHER,
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(f) => {
            if let Some(message) = f.message {
                if cli.json {
                    println!(
                        "{}",
                        json!({ "error": { "code": f.code, "message": message } })
                    );
                } else {
                    eprintln!("ossteam: {message}");
                }
            }
            ExitCode::from(f.code)
        }
    }
}

fn connect(cli: &Cli, autostart: bool) -> Result<Client, ClientError> {
    let info = ClientInfo {
        kind: PeerKind::Cli,
        name: "ossteam".into(),
        version: env!("CARGO_PKG_VERSION").into(),
    };
    let opts = ConnectOptions {
        autostart: autostart && !cli.no_autostart,
        ..ConnectOptions::new(info)
    };
    Client::connect(&opts)
}

/// Whether to colour what goes to stderr (CLI-4).
fn stderr_color(cli: &Cli) -> bool {
    match cli.color {
        Color::Always => true,
        Color::Never => false,
        Color::Auto => std::io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none(),
    }
}

fn run(cli: &Cli) -> Result<(), Failure> {
    match &cli.command {
        Command::Login {
            qr,
            username,
            no_remember,
        } => session::login(cli, *qr, username.as_deref(), *no_remember),
        Command::Logout => session::logout(cli),
        Command::Account { action } => match action.as_ref().unwrap_or(&AccountAction::List) {
            AccountAction::List => session::account_list(cli),
            AccountAction::Switch { account } => session::account_switch(cli, account),
        },
        Command::Persona { state } => session::persona(cli, *state),
        Command::Library { installed, filter } => {
            library::list(cli, *installed, filter.as_deref().unwrap_or_default())
        }
        Command::Info { app } => library::info(cli, app),
        Command::Install {
            app,
            folder,
            branch,
            wait,
        } => content::install(cli, app, folder.as_deref(), branch.as_deref(), *wait),
        Command::Update { apps, all, wait } => content::update(cli, apps, *all, *wait),
        Command::Verify { app, wait } => content::verify(cli, app, *wait),
        Command::Uninstall {
            app,
            yes,
            remove_compat_data,
            wait,
        } => content::uninstall(cli, app, *yes, *remove_compat_data, *wait),
        Command::Jobs { action } => match action.as_ref().unwrap_or(&JobsAction::List) {
            JobsAction::List => content::list_jobs(cli),
            JobsAction::Pause { job } => content::control(cli, *job, content::Control::Pause),
            JobsAction::Resume { job } => content::control(cli, *job, content::Control::Resume),
            JobsAction::Cancel { job } => content::control(cli, *job, content::Control::Cancel),
            JobsAction::Priority { job, priority } => {
                content::control(cli, *job, content::Control::Priority(*priority))
            }
            JobsAction::Watch => events::follow(cli, &[events::TopicArg::Jobs]),
        },
        Command::Launch {
            app,
            option,
            wait,
            allow_stale,
            args,
        } => games::launch(cli, app, *option, *wait, *allow_stale, args),
        Command::Running => games::running(cli),
        Command::Stop { app } => games::stop(cli, app),
        Command::Folders { action } => match action.as_ref().unwrap_or(&FoldersAction::List) {
            FoldersAction::List => content::list_folders(cli),
            FoldersAction::Add { path } => content::add_folder(cli, path),
            FoldersAction::Remove { path } => content::remove_folder(cli, path),
        },
        Command::Config { action } => match action.as_ref().unwrap_or(&ConfigAction::List) {
            ConfigAction::List => config::list(cli),
            ConfigAction::Get { key } => config::get(cli, key),
            ConfigAction::Set { key, value } => config::set(cli, key, Some(value)),
            ConfigAction::Unset { key } => config::set(cli, key, None),
        },
        Command::Events { topics } => events::follow(cli, topics),
        Command::Completions { shell } => {
            let mut cmd = Cli::command();
            clap_complete::generate(*shell, &mut cmd, "ossteam", &mut std::io::stdout());
            Ok(())
        }
        Command::Status => {
            let client = connect(cli, true)?;
            print_status(cli, &client.get_status()?);
            Ok(())
        }
        Command::Daemon { action } => match action.as_ref().unwrap_or(&DaemonAction::Status) {
            DaemonAction::Status => daemon_status(cli),
            DaemonAction::Start => daemon_start(cli),
            DaemonAction::Stop { force } => daemon_stop(cli, *force),
        },
    }
}

fn daemon_status(cli: &Cli) -> Result<(), Failure> {
    let client = match connect(cli, false) {
        Ok(c) => c,
        Err(ClientError::Unavailable { address, source }) => {
            if cli.json {
                println!(
                    "{}",
                    json!({ "running": false, "address": address, "reason": source.to_string() })
                );
            } else if !cli.quiet {
                match source.kind() {
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused => {
                        println!("ossteamd is not running");
                    }
                    _ => println!("ossteamd is not reachable at {address}: {source}"),
                }
            }
            return Err(Failure {
                code: exit::DAEMON_UNAVAILABLE,
                message: None,
            });
        }
        Err(e) => return Err(e.into()),
    };
    let status = client.get_status()?;
    let version = client.get_version()?;
    if cli.json {
        println!(
            "{}",
            json!({
                "running": true,
                "pid": status.pid,
                "version": status.daemon_version,
                "protocol": format!("{}.{}", version.protocol.unwrap_or_default().major, version.protocol.unwrap_or_default().minor),
                "uptime_secs": status.uptime_secs,
                "connected_clients": status.connected_clients,
            })
        );
    } else if !cli.quiet {
        let p = version.protocol.unwrap_or_default();
        println!("ossteamd {} is running", status.daemon_version);
        println!("  pid       {}", status.pid);
        println!("  uptime    {}", format_uptime(status.uptime_secs));
        println!("  protocol  {}.{}", p.major, p.minor);
        println!("  clients   {}", status.connected_clients);
    }
    Ok(())
}

fn daemon_start(cli: &Cli) -> Result<(), Failure> {
    if cli.no_autostart {
        return Err(Failure::new(
            exit::OTHER,
            "--no-autostart contradicts 'daemon start'",
        ));
    }
    let client = connect(cli, true)?;
    let status = client.get_status()?;
    let started = client.spawned_daemon_pid() == Some(status.pid);
    if cli.json {
        println!(
            "{}",
            json!({ "started": started, "pid": status.pid, "version": status.daemon_version })
        );
    } else if !cli.quiet {
        if started {
            println!(
                "started ossteamd {} (pid {})",
                status.daemon_version, status.pid
            );
        } else {
            println!(
                "ossteamd {} is already running (pid {})",
                status.daemon_version, status.pid
            );
        }
    }
    Ok(())
}

fn daemon_stop(cli: &Cli, force: bool) -> Result<(), Failure> {
    let client = match connect(cli, false) {
        Ok(c) => c,
        Err(ClientError::Unavailable { .. }) => {
            if cli.json {
                println!("{}", json!({ "stopped": false, "running": false }));
            } else if !cli.quiet {
                println!("ossteamd is not running");
            }
            return Ok(());
        }
        Err(e) => return Err(e.into()),
    };
    let pid = client.get_status()?.pid;
    client.shutdown(force)?;
    // The daemon closes the connection as it exits; wait for that so a
    // following `daemon start` gets a fresh daemon.
    if !client.wait_closed(STOP_TIMEOUT) {
        return Err(Failure::new(
            exit::OTHER,
            format!(
                "ossteamd (pid {pid}) accepted the request but did not exit within {STOP_TIMEOUT:?}"
            ),
        ));
    }
    if cli.json {
        println!("{}", json!({ "stopped": true, "pid": pid }));
    } else if !cli.quiet {
        println!("stopped ossteamd (pid {pid})");
    }
    Ok(())
}

fn connection_name(raw: i32) -> &'static str {
    match ConnectionState::try_from(raw).unwrap_or(ConnectionState::Unspecified) {
        ConnectionState::Disconnected => "disconnected",
        ConnectionState::Connecting => "connecting",
        ConnectionState::Connected => "connected",
        ConnectionState::Unspecified => "unknown",
    }
}

fn print_status(cli: &Cli, s: &v1::GetStatusResponse) {
    let connection = connection_name(s.connection);
    if cli.json {
        let account = s.account.as_ref().map(session::account_json);
        println!(
            "{}",
            json!({
                "daemon": { "version": s.daemon_version, "pid": s.pid, "uptime_secs": s.uptime_secs },
                "connection": connection,
                "account": account,
                "logged_on": s.logged_on,
                "reason": (!s.session_reason.is_empty()).then_some(&s.session_reason),
                "persona_state": s.account.as_ref().map(|_| session::persona_name(s.persona_state)),
                "active_jobs": s.active_jobs,
                "running_games": s.running_games,
            })
        );
        return;
    }
    if cli.quiet {
        return;
    }
    let mut out = std::io::stdout().lock();
    let _ = writeln!(
        out,
        "daemon      ossteamd {} (pid {}, up {})",
        s.daemon_version,
        s.pid,
        format_uptime(s.uptime_secs)
    );
    let _ = writeln!(out, "connection  {connection}");
    match &s.account {
        Some(a) => {
            let name = if a.persona_name.is_empty() {
                format!("{} ({})", a.account_name, a.steam_id)
            } else {
                format!("{} ({}, {})", a.persona_name, a.account_name, a.steam_id)
            };
            let state = if s.logged_on {
                "logged on"
            } else {
                "not logged on"
            };
            let _ = writeln!(out, "account     {name}, {state}");
            let _ = writeln!(
                out,
                "persona     {}",
                session::persona_name(s.persona_state)
            );
        }
        None => {
            let _ = writeln!(out, "account     not logged in");
        }
    }
    if !s.session_reason.is_empty() {
        let _ = writeln!(out, "note        {}", s.session_reason);
    }
    let _ = writeln!(out, "jobs        {}", s.active_jobs);
    let _ = writeln!(out, "games       {}", s.running_games);
}

fn format_uptime(secs: u64) -> String {
    let (d, h, m, s) = (secs / 86400, secs / 3600 % 24, secs / 60 % 60, secs % 60);
    match (d, h, m) {
        (0, 0, 0) => format!("{s}s"),
        (0, 0, _) => format!("{m}m {s}s"),
        (0, _, _) => format!("{h}h {m}m"),
        _ => format!("{d}d {h}h"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn uptime_formatting() {
        assert_eq!(format_uptime(5), "5s");
        assert_eq!(format_uptime(65), "1m 5s");
        assert_eq!(format_uptime(3 * 3600 + 120), "3h 2m");
        assert_eq!(format_uptime(2 * 86400 + 3600), "2d 1h");
    }
}
