// SPDX-License-Identifier: GPL-2.0-or-later
//! Update holds (`docs/spec/steamworks/isteamappdisableupdate.md` §3): a
//! game asks that its app's automatic updates wait.

use std::time::{Duration, Instant};

use ossteam_gameipc::{Done, ResponseBody, UpdateHoldRequest};
use ossteam_types::AppId;

use super::shim::error;
use super::{Command, Core};
use crate::gameipc::ConnId;

impl Core {
    /// `UpdateHold` (§2).
    pub(super) fn update_hold(&mut self, conn: ConnId, r: UpdateHoldRequest) -> ResponseBody {
        let Some(app) = self.shim_app_of(conn) else {
            return error("the connection is closing");
        };
        if let Err(e) = self.shim_owner(app) {
            return e;
        }
        match u64::try_from(r.seconds).ok().filter(|&s| s > 0) {
            Some(seconds) => {
                let until = Instant::now() + Duration::from_secs(seconds);
                self.update_holds.insert(app, until);
                tracing::info!(app = %app, seconds, "a game holds the app's automatic updates");
                let commands = self.games.commands.clone();
                let spawned = std::thread::Builder::new()
                    .name("update-hold".into())
                    .spawn(move || {
                        std::thread::sleep(until.saturating_duration_since(Instant::now()));
                        let _ = commands.send(Command::UpdateHoldOver(app, until));
                    });
                if let Err(e) = spawned {
                    tracing::error!("cannot start an update hold's timer: {e}");
                }
            }
            None => self.end_update_hold(app, None),
        }
        ResponseBody::Done(Done {})
    }

    /// Whether automatic updates of `app` wait (§3).
    pub(super) fn update_held(&self, app: AppId) -> bool {
        self.update_holds
            .get(&app)
            .is_some_and(|&until| until > Instant::now())
    }

    /// Ends `app`'s hold: the one that ran until `until`, or any. Queues
    /// the update the hold kept back.
    pub(super) fn end_update_hold(&mut self, app: AppId, until: Option<Instant>) {
        let Some(&held) = self.update_holds.get(&app) else {
            return;
        };
        if until.is_some_and(|u| u != held) {
            // A later request replaced it.
            return;
        }
        self.update_holds.remove(&app);
        tracing::info!(app = %app, "the app's automatic updates no longer wait");
        self.queue_held_update(app);
    }
}
