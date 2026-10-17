// SPDX-License-Identifier: GPL-2.0-or-later
//! `ossteamd`, the per-user daemon (SPEC §7).

mod assets;
mod config;
mod core;
mod db;
mod library;
mod logging;
mod paths;
mod pics;
mod secrets;
mod server;
mod session;

use std::fs::{File, OpenOptions, TryLockError};
use std::io::Write;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use anyhow::Context;
use clap::Parser;
use crossbeam_channel::{RecvTimeoutError, bounded, unbounded};
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
}

fn main() -> ExitCode {
    let args = Args::parse();
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

    let mut config = config::Config::load(&paths.config)?;
    let db = db::Db::open(&paths.data.join(db::FILE_NAME))?;
    config.apply_overrides(db.setting_overrides()?);
    let resume = match db.active_account()? {
        Some(id) => db.account(id)?,
        None => None,
    };
    let cell_id = db.cell_id()?;

    let listener = endpoint
        .listen()
        .with_context(|| format!("listening on {}", endpoint.address))?;
    tracing::info!(pid = std::process::id(), address = %endpoint.address, "ossteamd {} started", env!("CARGO_PKG_VERSION"));
    tracing::debug!(
        config = %paths.config.display(),
        data = %paths.data.display(),
        cache = %paths.cache.display(),
        state = %paths.state.display(),
        "directories"
    );

    let (cmd_tx, cmd_rx) = unbounded();
    let (alive_tx, alive_rx) = bounded::<()>(0);
    let server = server::Server::start(listener, endpoint.clone(), cmd_tx.clone(), alive_tx)
        .context("starting the IPC server")?;

    let pics_dir = paths.cache.join("pics");
    let pics = pics::Handle::spawn(Cache::new(&pics_dir), cmd_tx.clone())
        .context("starting the PICS worker")?;
    let session = session::Handle::spawn(
        session::Settings {
            cache_dir: paths.cache.clone(),
            data_dir: paths.data.clone(),
            file_fallback: config.file_fallback(),
            cell_id,
            resume,
            pics: pics.sender(),
        },
        cmd_tx.clone(),
    )
    .context("starting the session worker")?;

    let assets = assets::Handle::spawn(cmd_tx.clone()).context("starting the image downloader")?;

    let signal_tx = cmd_tx.clone();
    ctrlc::set_handler(move || {
        let _ = signal_tx.send(core::Command::Terminate);
    })
    .context("installing the signal handler")?;
    drop(cmd_tx);

    let exit = core::Core::new(core::Parts {
        db,
        config,
        session,
        pics,
        pics_cache: Cache::new(pics_dir),
        assets,
        assets_dir: paths.cache.join("assets"),
    })
    .run(cmd_rx);
    tracing::info!(?exit, "shutting down");

    // Shutdown (SPEC D-5): stop accepting (dropping the listener removes the
    // socket file), then let writers flush what the core queued: the
    // Shutdown response and DaemonStopping. The lock is released when the
    // process exits.
    server.stop();
    let deadline = Instant::now() + FLUSH_TIMEOUT;
    match alive_rx.recv_deadline(deadline) {
        Err(RecvTimeoutError::Disconnected) | Ok(()) => {}
        Err(RecvTimeoutError::Timeout) => tracing::warn!("some clients did not drain in time"),
    }
    tracing::info!("stopped");
    Ok(ExitCode::SUCCESS)
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
