// SPDX-License-Identifier: GPL-2.0-or-later
//! Taking over an official Steam client's entries in the real `~/.steam`
//! while it is not running, handing them back, and yielding them when it
//! starts (DISC-5, `steamworks-abi.md` §1.1.5). Linux only.

use std::path::PathBuf;

#[cfg(not(target_os = "linux"))]
use super::Core;

/// The record of the official entries, in the configuration directory,
/// which no game's namespace hides.
pub const RECORD: &str = "official-entries.toml";

#[derive(Default)]
pub struct TakeOver {
    /// `<config>/official-entries.toml`.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub record: PathBuf,
    /// Whether the daemon holds the official client's entries.
    pub held: bool,
    /// Watches `~/.steam` while the setting is on and an account active.
    #[cfg(target_os = "linux")]
    watcher: Option<ossteam_launch::discovery::takeover::Watcher>,
}

impl TakeOver {
    pub fn new(config: &std::path::Path) -> Self {
        Self {
            record: config.join(RECORD),
            ..Self::default()
        }
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use ossteam_ipc::v1::{self, Topic, event};
    use ossteam_launch::discovery;
    use ossteam_launch::discovery::takeover::{TakeOverError, Watcher};

    use super::super::{Command, Core};
    use crate::config;

    /// The run-time notice when the official client takes the entries
    /// back (§1.1.6).
    const YIELDED: &str = "The official Steam client took ~/.steam back. What games ossteam \
         started without a private ~/.steam start from now on finds the official client and its \
         account.";

    /// The info notices of a take-over and a hand-back (§1.1.6).
    const TOOK_OVER: &str = "ossteam took over the official Steam client's ~/.steam entries \
         while that client is not running (discovery.take_over_official). It hands them back \
         when no account is active, when ossteamd stops and when the setting is turned off, and \
         yields them when that client starts.";
    const HANDED_BACK: &str = "ossteam handed the official Steam client's ~/.steam entries back.";

    fn warn_on(what: &str) {
        tracing::warn!(
            "discovery.take_over_official is on{what}: {}",
            config::TAKE_OVER_RISK_LINE
        );
        eprintln!(
            "ossteamd: warning: discovery.take_over_official is on: {}",
            config::TAKE_OVER_RISK_LINE
        );
    }

    impl Core {
        /// At start, before anything else: hands back what an earlier
        /// daemon held when it crashed (§1.5), and warns when the setting
        /// is on (§1.1.6).
        pub(in crate::core) fn take_over_at_start(&mut self) {
            match self.games.entries.hand_back(&self.games.take_over.record) {
                Ok(true) => tracing::warn!(
                    "handed back the official Steam client's ~/.steam entries, which an earlier \
                     ossteamd held"
                ),
                Ok(false) => {}
                Err(e) => tracing::warn!("handing back the official ~/.steam entries failed: {e}"),
            }
            if self.config.take_over_official() {
                warn_on("");
            }
        }

        /// `discovery.take_over_official` was set (§1.1.6).
        pub(in crate::core) fn take_over_setting_changed(&mut self, before: bool) {
            let on = self.config.take_over_official();
            if on && !before {
                warn_on(" now");
                self.broadcast(
                    Topic::Notices,
                    event::Kind::Notice(v1::Notice {
                        level: v1::NoticeLevel::Warning as i32,
                        text: format!(
                            "discovery.take_over_official is on: {}",
                            config::TAKE_OVER_RISK_LINE
                        ),
                    }),
                );
            }
            self.sync_take_over();
        }

        /// Hands the official entries back, if the daemon holds them, where
        /// §1.5 removes the real entries.
        pub(in crate::core) fn hand_back(&mut self, why: &str) {
            let t = &mut self.games.take_over;
            t.watcher = None;
            if !std::mem::take(&mut t.held) {
                return;
            }
            match self.games.entries.hand_back(&t.record) {
                Ok(_) => {
                    tracing::warn!(
                        "handed back the official Steam client's ~/.steam entries ({why})"
                    );
                    self.notice(v1::NoticeLevel::Info, HANDED_BACK);
                }
                Err(e) => tracing::warn!("handing back the official ~/.steam entries failed: {e}"),
            }
        }

        /// The official client, or the user, changed an entry the daemon
        /// held: it writes nothing more to `~/.steam` (§1.1.5).
        fn yield_entries(&mut self) {
            let t = &mut self.games.take_over;
            t.held = false;
            if let Err(e) = std::fs::remove_file(&t.record)
                && e.kind() != std::io::ErrorKind::NotFound
            {
                tracing::warn!("removing {} failed: {e}", t.record.display());
            }
            tracing::warn!(
                "the official Steam client took ~/.steam back; ossteam yields the entries"
            );
            self.notice(v1::NoticeLevel::Warning, YIELDED);
        }

        fn notice(&mut self, level: v1::NoticeLevel, text: &str) {
            self.broadcast(
                Topic::Notices,
                event::Kind::Notice(v1::Notice {
                    level: level as i32,
                    text: text.into(),
                }),
            );
        }

        /// Takes over, yields or hands back as the setting, the session
        /// and `~/.steam` now stand (§1.1.5). Called when any of them
        /// changes, and on the watcher's ticks.
        pub(in crate::core) fn sync_take_over(&mut self) {
            let want = self.config.take_over_official() && self.session_state.account.is_some();
            let pid = std::process::id();
            if self.games.take_over.held {
                if !self.games.entries.held_by(pid).unwrap_or(false) {
                    self.yield_entries();
                } else if !want {
                    self.hand_back("the setting is off or no account is active");
                    return;
                } else {
                    return;
                }
            }
            if !want {
                self.games.take_over.watcher = None;
                return;
            }
            if !self.games.entries_made {
                // The private `~/.steam`, which the take-over links to, is
                // made with the entries (§1.5).
                return;
            }
            let t = &mut self.games.take_over;
            if t.watcher.as_ref().is_none_or(Watcher::finished) {
                t.watcher = None;
                let tx = self.games.commands.clone();
                let tick = self.games.settings.take_over_tick;
                match Watcher::start(self.games.entries.steam_dir(), tick, move |_| {
                    tx.send(Command::SteamDir).is_ok()
                }) {
                    Ok(w) => t.watcher = Some(w),
                    Err(e) => {
                        tracing::warn!("cannot watch ~/.steam, so no take-over: {e}");
                        return;
                    }
                }
            }
            if !matches!(
                self.games.entries.check(),
                Err(discovery::Error::Conflict(_))
            ) {
                // Free, or ossteam's already.
                return;
            }
            let g = &self.games;
            let Some(watcher) = g.take_over.watcher.as_ref() else {
                return;
            };
            // Taken once: the links stop being the official client's as
            // the take-over goes on.
            let dirs = match g.entries.official_dirs() {
                Ok(d) => d,
                Err(e) => {
                    tracing::warn!("taking over ~/.steam failed: {e}");
                    return;
                }
            };
            let mut conditions = || -> Result<(), String> {
                // The official client, its steam.sh or a game it started:
                // the watcher wakes the daemon when it exits, so the
                // take-over follows the client's close at once.
                if let Some(p) = g.entries.official_process(&dirs) {
                    watcher.watch_exit(p.pid);
                    return Err(p.to_string());
                }
                // The race with `steam.sh`. Changes the official client
                // made before it closed don't count (`watch_exit`).
                let quiet = watcher.quiet_for();
                if quiet < g.settings.take_over_quiet {
                    return Err(format!("~/.steam changed {} s ago", quiet.as_secs()));
                }
                Ok(())
            };
            match g
                .entries
                .take_over(&g.take_over.record, pid, &mut conditions)
            {
                Ok(()) => {
                    self.games.take_over.held = true;
                    tracing::warn!(
                        "took over the official Steam client's ~/.steam entries \
                         (discovery.take_over_official); recorded in {}",
                        self.games.take_over.record.display()
                    );
                    self.notice(v1::NoticeLevel::Info, TOOK_OVER);
                    // The real entries are ossteam's now: refresh them.
                    if let Err(e) = self.make_discovery() {
                        tracing::warn!("refreshing the discovery entries failed: {e}");
                    }
                }
                Err(TakeOverError::Stopped(why)) => {
                    tracing::debug!("no take-over of ~/.steam yet: {why}");
                }
                Err(TakeOverError::Io(e)) => tracing::warn!("taking over ~/.steam failed: {e}"),
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
impl Core {
    pub(super) fn take_over_at_start(&mut self) {}
    pub(super) fn take_over_setting_changed(&mut self, _before: bool) {}
    pub(super) fn hand_back(&mut self, _why: &str) {}
    pub(super) fn sync_take_over(&mut self) {}
}
