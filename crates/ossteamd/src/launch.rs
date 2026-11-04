// SPDX-License-Identifier: GPL-2.0-or-later
//! Running games (`launch.md`): where their files go, and one supervisor
//! thread per game that waits for its tree and reports the end to the
//! core (CONC-1, LAUNCH-4).

use std::path::PathBuf;

use crossbeam_channel::Sender;
use ossteam_launch::process::{Exit, Game};
use ossteam_types::AppId;

use crate::core::Command;

/// What launching depends on outside the daemon's own directories.
#[derive(Debug, Clone)]
pub struct Settings {
    /// The user's home directory, which holds `~/.steam` on Linux
    /// (`steamworks-abi.md` §1.1).
    pub home: PathBuf,
    /// Where the client libraries are (`steamworks-abi.md` §1.4); `None`
    /// when it cannot be determined.
    pub library_dir: Option<PathBuf>,
    /// The program that runs `--reaper` on Linux: this executable.
    pub reaper: PathBuf,
    /// Linux: whether launches may get a private `~/.steam`
    /// (`steamworks-abi.md` §1.1.2). Only tests turn it off, to see
    /// launches without one.
    pub private_steam: bool,
}

impl Settings {
    /// The real ones: the home directory, the library directory and this
    /// executable.
    pub fn resolve() -> std::io::Result<Self> {
        let home = directories::BaseDirs::new()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no home directory"))?
            .home_dir()
            .to_owned();
        Ok(Self {
            home,
            library_dir: ossteam_launch::discovery::library_dir(),
            reaper: std::env::current_exe()?,
            private_steam: true,
        })
    }
}

/// From a supervisor thread: the game's last process is gone.
pub struct Exited {
    pub app: AppId,
    /// The pid the daemon started, so a report about an earlier run of
    /// the same app is recognised.
    pub pid: u32,
    pub exit: std::io::Result<Exit>,
}

/// Waits for `game` on a thread of its own and reports its end.
pub fn supervise(app: AppId, game: Game, core: Sender<Command>) -> std::io::Result<()> {
    let pid = game.pid();
    std::thread::Builder::new()
        .name(format!("game-{app}"))
        .spawn(move || {
            let exit = game.wait();
            let _ = core.send(Command::Game(Exited { app, pid, exit }));
        })
        .map(drop)
}
