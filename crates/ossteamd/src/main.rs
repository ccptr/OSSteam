// SPDX-License-Identifier: GPL-2.0-or-later
//! `ossteamd`, the per-user daemon (SPEC §7).

mod assets;
mod config;
mod content;
mod core;
mod db;
mod gameipc;
mod launch;
mod library;
mod logging;
mod paths;
mod pics;
mod secrets;
mod server;
mod session;
mod stats;
#[cfg(test)]
mod testing;
mod tickets;

use std::fs::{File, OpenOptions, TryLockError};
use std::io::Write;
use std::path::Path;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use anyhow::Context;
use clap::Parser;
use crossbeam_channel::{RecvTimeoutError, Sender, bounded, unbounded};
use interprocess::local_socket::Listener;
use ossteam_ipc::v1::PeerKind;
use ossteam_ipc::{Client, ClientInfo, ConnectOptions, Endpoint};
use ossteam_pics::Cache;

/// How long a second instance waits for the first to answer before it
/// declares the first unhealthy.
const PEER_HEALTH_TIMEOUT: Duration = Duration::from_secs(3);
/// How long shutdown waits for queued frames to reach clients.
const FLUSH_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Parser)]
#[command(name = "ossteamd", version, about = "ossteam per-user daemon")]
struct Args {
    /// Detach from the terminal and log only to the log file. Leaf clients
    /// pass this when they start the daemon.
    #[arg(long)]
    background: bool,
    /// More log output (-v debug, -vv trace). OSSTEAM_LOG overrides this.
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,
    /// Run as the elevated helper for install scripts, with the steps the
    /// daemon encoded (`install-scripts.md` §4).
    #[arg(long, hide = true, value_name = "REQUEST")]
    script_helper: Option<String>,
}

fn main() -> ExitCode {
    // The reaper's arguments end in the game's command line, which clap
    // must not see (`launch.md` §4).
    #[cfg(target_os = "linux")]
    {
        let raw: Vec<std::ffi::OsString> = std::env::args_os().collect();
        if raw.get(1).is_some_and(|a| a == "--reaper") {
            ossteam_launch::process::reaper_main(&raw[2..]);
        }
    }
    let args = Args::parse();
    if let Some(request) = &args.script_helper {
        return match content::script::helper_main(request) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("ossteamd: {e:#}");
                ExitCode::FAILURE
            }
        };
    }
    if args.background {
        detach();
    }
    match run(&args) {
        Ok(code) => code,
        Err(e) => {
            tracing::error!("{e:#}");
            if !args.background {
                eprintln!("ossteamd: {e:#}");
            }
            ExitCode::FAILURE
        }
    }
}

#[cfg(unix)]
fn detach() {
    // Leave the starting client's session so its terminal closing does not
    // send us SIGHUP. Fails harmlessly if we already lead a session.
    let _ = rustix::process::setsid();
}

#[cfg(not(unix))]
fn detach() {}

fn run(args: &Args) -> anyhow::Result<ExitCode> {
    let paths = paths::Paths::resolve().context("resolving directories")?;
    let _log_guard = logging::init(&paths.state, !args.background, args.verbose)
        .with_context(|| format!("setting up logging in {}", paths.state.display()))?;

    let endpoint = Endpoint::client().context("locating the IPC endpoint")?;
    endpoint
        .ensure_dir()
        .with_context(|| format!("preparing the runtime directory for {}", endpoint.address))?;

    let Some(_lock) = acquire_lock(&endpoint)? else {
        return Ok(if peer_is_healthy() {
            tracing::info!("another ossteamd is already running");
            ExitCode::SUCCESS
        } else if acquire_lock(&endpoint)?.is_some() {
            // The other instance exited while we were checking on it. Do not
            // take over: it may have been stopped on purpose.
            tracing::info!("the other ossteamd stopped while this one was starting; exiting");
            ExitCode::FAILURE
        } else {
            tracing::error!(
                "{} is locked by another process that does not answer",
                endpoint.lock_file.display()
            );
            ExitCode::FAILURE
        });
    };

    let listener = endpoint
        .listen()
        .with_context(|| format!("listening on {}", endpoint.address))?;
    let game_address =
        ossteam_gameipc::endpoint::Address::locate().context("locating the game endpoint")?;
    let game_endpoint = gameipc::endpoint(&endpoint, &game_address);
    let game_listener = game_endpoint
        .listen()
        .with_context(|| format!("listening on {}", game_endpoint.address))?;
    let game_abstract = listen_abstract(&game_address);
    tracing::info!(pid = std::process::id(), address = %endpoint.address, "ossteamd {} started", env!("CARGO_PKG_VERSION"));
    tracing::debug!(
        config = %paths.config.display(),
        data = %paths.data.display(),
        cache = %paths.cache.display(),
        state = %paths.state.display(),
        "directories"
    );

    let dialer = Box::new(session::Servers::new(paths.cache.clone()));
    serve(
        Env {
            paths,
            listener,
            endpoint,
            game_listener,
            game_endpoint,
            game_abstract,
            dialer,
            open_secrets: secrets::Secrets::open,
            pics_poll: pics::POLL_INTERVAL,
            content_fetch: None,
            scripts: content::script::system(),
            launch: launch::Settings::resolve().context("resolving the home directory")?,
        },
        |cmd_tx| {
            let signal_tx = cmd_tx.clone();
            ctrlc::set_handler(move || {
                let _ = signal_tx.send(core::Command::Terminate);
            })
            .context("installing the signal handler")
        },
    )?;
    Ok(ExitCode::SUCCESS)
}

/// Linux: the abstract game socket. Without it, games in a Steam Linux
/// Runtime container find no client, but everything else works.
fn listen_abstract(addr: &ossteam_gameipc::endpoint::Address) -> Option<gameipc::Abstract> {
    #[cfg(target_os = "linux")]
    if let Some(name) = &addr.abstract_name {
        return gameipc::listen_abstract(name)
            .inspect_err(|e| {
                tracing::warn!(
                    "cannot listen on the abstract socket {name}; games in a Steam Linux \
                     Runtime container will find no client: {e}"
                );
            })
            .ok();
    }
    let _ = addr;
    None
}

/// What [`serve`] runs on. The daemon passes the real CM dialer and the OS
/// keyring; only tests pass anything else (FAITH-4).
struct Env {
    paths: paths::Paths,
    listener: Listener,
    endpoint: Endpoint,
    /// The game endpoint (`steamworks-abi.md` §3.1).
    game_listener: Listener,
    game_endpoint: Endpoint,
    /// Linux: the game endpoint for containers (`compat-tools.md` §9).
    game_abstract: Option<gameipc::Abstract>,
    dialer: Box<dyn session::Dialer>,
    open_secrets: fn(bool, &Path) -> secrets::Secrets,
    pics_poll: Duration,
    /// Where content comes from; `None` is the CDN over HTTPS.
    content_fetch: Option<std::sync::Arc<dyn ossteam_content::cdn::Fetch>>,
    /// What install scripts act on (`install-scripts.md`).
    scripts: Option<std::sync::Arc<dyn content::script::System>>,
    launch: launch::Settings,
}

/// Starts the workers and the IPC server and runs the core until it stops
/// (SPEC §7.1). `started` runs once the core's command channel exists.
fn serve(
    env: Env,
    started: impl FnOnce(&Sender<core::Command>) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let Env {
        paths,
        listener,
        endpoint,
        game_listener,
        game_endpoint,
        game_abstract,
        dialer,
        open_secrets,
        pics_poll,
        content_fetch,
        scripts,
        launch,
    } = env;
    let mut config = config::Config::load(&paths.config)?;
    let db = db::Db::open(&paths.data.join(db::FILE_NAME))?;
    config.apply_overrides(db.setting_overrides()?);
    let resume = match db.active_account()? {
        Some(id) => db.account(id)?,
        None => None,
    };
    let cell_id = db.cell_id()?;

    let (cmd_tx, cmd_rx) = unbounded();
    let (alive_tx, alive_rx) = bounded::<()>(0);
    let server = server::Server::start(listener, endpoint, cmd_tx.clone(), alive_tx)
        .context("starting the IPC server")?;
    let game_server =
        gameipc::Server::start(game_listener, game_endpoint, game_abstract, cmd_tx.clone())
            .context("starting the game endpoint")?;

    let pics_dir = paths.cache.join("pics");
    let pics = pics::Handle::spawn(Cache::new(&pics_dir), pics_poll, cmd_tx.clone())
        .context("starting the PICS worker")?;
    let session = session::Handle::spawn(
        session::Settings {
            data_dir: paths.data.clone(),
            file_fallback: config.file_fallback(),
            cell_id,
            open_secrets,
            resume,
            pics: pics.sender(),
        },
        dialer,
        cmd_tx.clone(),
    )
    .context("starting the session worker")?;

    let assets = assets::Handle::spawn(cmd_tx.clone()).context("starting the image downloader")?;
    let stats = stats::Handle::spawn(cmd_tx.clone(), paths.cache.join("achievement-icons"))
        .context("starting the stats worker")?;
    let content_jobs = match content_fetch {
        None => content::Handle::new(cmd_tx.clone(), config.bandwidth_limit(), scripts),
        Some(fetch) => content::Handle::with_fetch(
            cmd_tx.clone(),
            fetch,
            std::sync::Arc::new(ossteam_content::throttle::Throttle::new(
                config.bandwidth_limit(),
            )),
            scripts,
        ),
    };

    started(&cmd_tx)?;

    let exit = core::Core::new(core::Parts {
        db,
        config,
        session,
        pics,
        pics_cache: Cache::new(pics_dir),
        assets,
        assets_dir: paths.cache.join("assets"),
        stats,
        content_jobs,
        default_folder: paths.data.join("library"),
        launch,
        data_dir: paths.data.clone(),
        state_dir: paths.state.clone(),
        config_dir: paths.config.clone(),
        commands: cmd_tx,
    })
    .run(cmd_rx);
    tracing::info!(?exit, "shutting down");

    // Shutdown (SPEC D-5): stop accepting (dropping the listener removes the
    // socket file), then let writers flush what the core queued: the
    // Shutdown response and DaemonStopping. The lock is released when the
    // process exits.
    server.stop();
    game_server.stop();
    let deadline = Instant::now() + FLUSH_TIMEOUT;
    match alive_rx.recv_deadline(deadline) {
        Err(RecvTimeoutError::Disconnected) | Ok(()) => {}
        Err(RecvTimeoutError::Timeout) => tracing::warn!("some clients did not drain in time"),
    }
    tracing::info!("stopped");
    Ok(())
}

/// Takes the single-instance lock (SPEC D-1). Returns `None` when another
/// process holds it.
fn acquire_lock(endpoint: &Endpoint) -> anyhow::Result<Option<File>> {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&endpoint.lock_file)
        .with_context(|| format!("opening {}", endpoint.lock_file.display()))?;
    match file.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => return Ok(None),
        Err(TryLockError::Error(e)) => {
            return Err(e).with_context(|| format!("locking {}", endpoint.lock_file.display()));
        }
    }
    // The PID is informational, for people looking at the file.
    file.set_len(0)?;
    writeln!(file, "{}", std::process::id())?;
    Ok(Some(file))
}

/// Whether the daemon holding the lock answers a status request. It may
/// have just started and not be listening yet, so this retries briefly.
fn peer_is_healthy() -> bool {
    let info = ClientInfo {
        kind: PeerKind::Daemon,
        name: "ossteamd".into(),
        version: env!("CARGO_PKG_VERSION").into(),
    };
    let opts = ConnectOptions {
        autostart: false,
        ..ConnectOptions::new(info)
    };
    let deadline = Instant::now() + PEER_HEALTH_TIMEOUT;
    loop {
        if let Ok(client) = Client::connect(&opts)
            && client.get_status().is_ok()
        {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
