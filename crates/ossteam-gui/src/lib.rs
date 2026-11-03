// SPDX-License-Identifier: GPL-2.0-or-later
//! `ossteam-gui`, the graphical client (SPEC §16, `docs/spec/gui.md`). It
//! renders what the daemon reports and keeps no state of its own beyond
//! the view (GUI-2).
//!
//! The binary only calls [`run`]. The connection, the store and the model
//! are public for the daemon's tests, which drive them against a test
//! daemon without a window (`gui.md` §7).

mod args;
mod bench;
pub mod daemon;
mod format;
mod gui;
pub mod model;
mod pages;
mod queue;
mod rate;
pub mod store;
mod theme;
mod thumbnails;
mod widgets;

use std::process::ExitCode;
use std::time::Instant;

use ossteam_ui::app::{self, Options};
use ossteam_ui::render::Choice;

const USAGE: &str = "usage: ossteam-gui [--no-autostart] [--renderer auto|vulkan|software]";

/// Opens the window and runs until it closes.
pub fn run() -> ExitCode {
    let started = Instant::now();
    let (autostart, renderer) = match parse(std::env::args().skip(1)) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("ossteam-gui: {e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    // RUST_LOG, or the toolkit's start-up lines and every warning.
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn,ossteam_ui=info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();

    let options = Options {
        title: "ossteam".into(),
        app_id: Some("ossteam".into()),
        width: 1100.0,
        height: 720.0,
        min_width: 480.0,
        min_height: 360.0,
        renderer,
    };
    match app::run(options, gui::bindings(), move |sender| {
        gui::Gui::new(sender, started, autostart)
    }) {
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("ossteam-gui: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `--no-autostart` and `--renderer`.
fn parse(args: impl Iterator<Item = String>) -> Result<(bool, Choice), String> {
    let mut autostart = true;
    let mut renderer = Choice::Auto;
    let mut args = args;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--no-autostart" => autostart = false,
            "--renderer" => {
                let v = args.next().ok_or("--renderer needs a value")?;
                renderer = v.parse()?;
            }
            _ => match a.strip_prefix("--renderer=") {
                Some(v) => renderer = v.parse()?,
                None => return Err(format!("unknown argument {a:?}")),
            },
        }
    }
    Ok((autostart, renderer))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Result<(bool, Choice), String> {
        parse(a.iter().map(|s| s.to_string()))
    }

    #[test]
    fn arguments() {
        assert_eq!(args(&[]), Ok((true, Choice::Auto)));
        assert_eq!(
            args(&["--no-autostart", "--renderer", "software"]),
            Ok((false, Choice::Software))
        );
        assert_eq!(args(&["--renderer=vulkan"]), Ok((true, Choice::Vulkan)));
        assert!(args(&["--renderer"]).is_err());
        assert!(args(&["--renderer", "gl"]).is_err());
        assert!(args(&["--verbose"]).is_err());
    }
}
