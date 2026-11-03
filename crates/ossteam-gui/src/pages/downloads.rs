// SPDX-License-Identifier: GPL-2.0-or-later
//! The Downloads page (`gui.md` §4.4): the queue, its order, and history.

use std::time::Instant;

use ossteam_ipc::v1::{self, JobState};
use ossteam_ui::element::{FocusHandle, ScrollHandle, panel, text};
use ossteam_ui::style::{Align, Styled};
use ossteam_ui::window::ViewCx;

use crate::format;
use crate::gui::Gui;
use crate::model::Model;
use crate::queue::{self, Move};
use crate::rate;
use crate::theme::Palette;
use crate::widgets::{
    self, El, SMALL, button, error_text, group, muted, progress_bar, scrolling_page, section, title,
};

#[derive(Default)]
pub struct DownloadsPage {
    scroll: ScrollHandle,
    update_all_error: Option<String>,
}

pub fn page_focus() -> FocusHandle {
    FocusHandle::named("downloads-page")
}

fn update_all(g: &mut Gui) {
    g.downloads.update_all_error = None;
    g.call(
        |c| c.update(&[], true),
        |g, r, _| match r {
            Ok(jobs) => {
                if let Some(m) = g.model_mut() {
                    for j in jobs {
                        m.store.add_job(j);
                    }
                }
            }
            Err(e) => g.downloads.update_all_error = Some(e.message),
        },
    );
}

/// Reorders the queue: the priority calls run in order on one thread, and
/// the daemon's `JobStateChanged` events redraw the queue.
fn reorder(g: &mut Gui, index: usize, mv: Move) {
    let Some(m) = g.model_mut() else {
        return;
    };
    let q = queue::queue(m.store.jobs.values());
    let Some(job_id) = q.get(index).map(|j| j.job_id) else {
        return;
    };
    let Some(wanted) = queue::moved(&q, index, mv) else {
        return;
    };
    let calls = queue::priorities(&q, &wanted);
    m.job_call(job_id, move |c| {
        for (id, priority) in calls {
            c.set_job_priority(id, priority)?;
        }
        Ok(())
    });
}

fn job_call(
    g: &mut Gui,
    id: u64,
    f: impl FnOnce(&ossteam_ipc::Client) -> Result<(), ossteam_ipc::ClientError> + Send + 'static,
) {
    if let Some(m) = g.model_mut() {
        m.job_call(id, f);
    }
}

/// A job's line: its app, kind and phase, progress, rate and time left,
/// its error, and Pause, Resume and Cancel. `position` is its index and
/// the queue's length, for the move buttons of the Downloads page.
pub fn job_row(m: &Model, job: &v1::Job, position: Option<(usize, usize)>, p: Palette) -> El {
    let id = job.job_id;
    let state = job.state();
    let name = m
        .store
        .app_name(job.app_id)
        .map(str::to_owned)
        .unwrap_or_else(|| format!("App {}", job.app_id));
    let progress = job.progress.unwrap_or_default();
    // A job that stopped part way, paused or failed or queued again,
    // shows how far it got.
    let started = queue::is_unfinished(job) && format::fraction(&progress).is_some();
    let detail = match state {
        JobState::Running => format::progress(&progress),
        s if started => format!("{}, {}", format::state(s), format::progress(&progress)),
        s => format::state(s).to_owned(),
    };
    let mut speed = String::new();
    if state == JobState::Running
        && let Some(r) = m.rates.get(id)
    {
        if let Some(bps) = r.per_second() {
            speed = format!("{}/s", format::size(bps as u64));
        }
        if let Some(left) = r.time_left(Instant::now(), rate::remaining(&progress)) {
            speed = format!("{speed}, {} left", format::duration(left));
        }
    }
    let error = job
        .error
        .as_ref()
        .filter(|_| state == JobState::Failed)
        .map(|e| e.message.clone());
    let call_error = m.job_errors.get(&id).cloned();

    // Pause and Resume share an id, and so a focus handle: the one that
    // replaces the other keeps the focus.
    let mut buttons = panel().row().gap(4.0);
    if matches!(state, JobState::Queued | JobState::Running) {
        buttons = buttons.child(
            button(p, widgets::id("pause-resume", id), "Pause")
                .secondary()
                .on_click(move |g, _| job_call(g, id, move |c| c.pause_job(id).map(drop))),
        );
    }
    if matches!(state, JobState::Paused | JobState::Failed) {
        buttons = buttons.child(
            button(p, widgets::id("pause-resume", id), "Resume")
                .secondary()
                .on_click(move |g, _| job_call(g, id, move |c| c.resume_job(id).map(drop))),
        );
    }
    if queue::is_unfinished(job) {
        buttons = buttons.child(
            button(p, widgets::id("cancel", id), "Cancel")
                .danger()
                .on_click(move |g, _| job_call(g, id, move |c| c.cancel_job(id).map(drop))),
        );
    }
    if let Some((index, len)) = position {
        for (label, key, mv, enabled) in [
            ("Move up", "move-up", Move::Up, index > 0),
            ("Move down", "move-down", Move::Down, index + 1 < len),
            ("Move to top", "move-top", Move::ToTop, index > 0),
        ] {
            buttons = buttons.child(
                button(p, widgets::id(key, id), label)
                    .ghost()
                    .disabled(!enabled)
                    .on_click(move |g, _| reorder(g, index, mv)),
            );
        }
    }

    let mut words = panel().row().gap(12.0).child(muted(p, detail));
    if !speed.is_empty() {
        words = words.child(muted(p, speed));
    }
    panel()
        .gap(4.0)
        .pad_xy(0.0, 8.0)
        .child(
            panel()
                .row()
                .align(Align::Center)
                .gap(8.0)
                .child(text(name).font_size(widgets::TEXT))
                .child(
                    text(format::kind(job.kind()))
                        .font_size(SMALL)
                        .text_color(p.muted),
                )
                .child(panel().grow())
                .child(buttons),
        )
        .when(state == JobState::Running || started, |d| {
            d.child(progress_bar(p, format::fraction(&progress)))
        })
        .child(words)
        .when_some(error, |d, e| d.child(error_text(p, e)))
        .when_some(call_error, |d, e| d.child(error_text(p, e)))
}

pub fn view(g: &Gui, m: &Model, p: Palette, _cx: &ViewCx) -> El {
    let logged_in = m.store.account().is_some();
    let queue = queue::queue(m.store.jobs.values());
    let history = queue::history(m.store.jobs.values());
    let len = queue.len();

    let queue_list: El = if queue.is_empty() {
        muted(p, "Nothing is queued.")
    } else {
        panel().children(
            queue
                .iter()
                .enumerate()
                .map(|(i, j)| job_row(m, j, Some((i, len)), p)),
        )
    };
    let history_list: El = if history.is_empty() {
        muted(p, "No finished jobs since ossteamd started.")
    } else {
        panel().children(history.iter().map(|j| job_row(m, j, None, p)))
    };

    let body = panel()
        .gap(16.0)
        .pad(24.0)
        .child(
            panel()
                .row()
                .align(Align::Center)
                .gap(12.0)
                .child(title("Downloads"))
                .child(panel().grow())
                .child(
                    button(p, "update-all", "Update all")
                        .secondary()
                        .disabled(!logged_in)
                        .on_click(|g, _| update_all(g)),
                ),
        )
        .when_some(g.downloads.update_all_error.clone(), |d, e| {
            d.child(error_text(p, e))
        })
        .when(!logged_in, |d| {
            d.child(muted(p, "Jobs belong to an account; sign in to see them."))
        })
        .child(section(p, "Queue", group(p).child(queue_list)))
        .child(section(p, "History", group(p).child(history_list)));
    scrolling_page(&g.downloads.scroll)
        .id("downloads-page")
        .focus(page_focus())
        .fill()
        .child(body)
}
