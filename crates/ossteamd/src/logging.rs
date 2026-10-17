// SPDX-License-Identifier: GPL-2.0-or-later
//! Logging to the state directory, rotated daily and kept for seven days
//! (SPEC D-10). In the foreground the same records also go to stderr.

use std::path::Path;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{Builder, Rotation};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::writer::MakeWriterExt;

/// Installs the global subscriber. Keep the guard alive until exit so
/// buffered records are flushed.
pub fn init(state_dir: &Path, to_stderr: bool, verbose: u8) -> anyhow::Result<WorkerGuard> {
    std::fs::create_dir_all(state_dir)?;
    let appender = Builder::new()
        .rotation(Rotation::DAILY)
        .filename_prefix("ossteamd")
        .filename_suffix("log")
        .max_log_files(7)
        .build(state_dir)?;
    let (file, guard) = tracing_appender::non_blocking(appender);

    let default = match verbose {
        0 => "info",
        1 => "debug",
        _ => "trace",
    };
    let filter = EnvFilter::try_from_env("OSSTEAM_LOG").unwrap_or_else(|_| EnvFilter::new(default));
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false);
    if to_stderr {
        builder.with_writer(file.and(std::io::stderr)).init();
    } else {
        builder.with_writer(file).init();
    }
    Ok(guard)
}
