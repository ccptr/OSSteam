// SPDX-License-Identifier: GPL-2.0-or-later
//! The Downloads page (`gui.md` §4.4): the queue, its order, and history.

use std::time::Instant;

use gpui::{
    AnyElement, App, Context, Entity, FocusHandle, Focusable, ScrollHandle, SharedString,
    Subscription, Window, div, prelude::*,
};
use ossteam_ipc::v1::{self, JobState};

use crate::format;
use crate::model::Model;
use crate::pages::{error_text, muted, panel, section};
use crate::queue::{self, Move};
use crate::rate;
use crate::theme::palette;
use crate::widgets::{button, page_scroll, progress_bar, reveal};

pub struct DownloadsPage {
    model: Entity<Model>,
    focus: FocusHandle,
    scroll: ScrollHandle,
    update_all_error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl DownloadsPage {
    pub fn new(model: Entity<Model>, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![cx.observe(&model, |_, _, cx| cx.notify())];
        Self {
            model,
            focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            update_all_error: None,
            _subscriptions: subscriptions,
        }
    }

    fn update_all(&mut self, cx: &mut Context<Self>) {
        self.update_all_error = None;
        let call = self.model.read(cx).daemon.call(|c| c.update(&[], true));
        cx.spawn(async move |this, cx| {
            let r = call.await;
            let _ = this.update(cx, |page, cx| {
                match r {
                    Ok(jobs) => page.model.update(cx, |m, cx| {
                        for j in jobs {
                            m.store.add_job(j);
                        }
                        cx.notify();
                    }),
                    Err(e) => page.update_all_error = Some(e.message.into()),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

/// Reorders the queue: the priority calls run in order on one thread, and
/// the daemon's `JobStateChanged` events redraw the queue.
fn reorder(model: &Entity<Model>, index: usize, m: Move, cx: &mut App) {
    model.update(cx, |model, cx| {
        let q = queue::queue(model.store.jobs.values());
        let Some(job_id) = q.get(index).map(|j| j.job_id) else {
            return;
        };
        let Some(wanted) = queue::moved(&q, index, m) else {
            return;
        };
        let calls = queue::priorities(&q, &wanted);
        model.job_call(job_id, cx, move |c| {
            for (id, priority) in calls {
                c.set_job_priority(id, priority)?;
            }
            Ok(())
        });
    });
}

/// A job's line: its app, kind and phase, progress, rate and time left,
/// its error, and Pause, Resume and Cancel. `position` is its index and
/// the queue's length, for the move buttons of the Downloads page.
pub fn job_row(
    model: &Entity<Model>,
    job: &v1::Job,
    position: Option<(usize, usize)>,
    cx: &App,
) -> AnyElement {
    let p = palette(cx);
    let m = model.read(cx);
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
    let mut buttons = div().flex().gap_1();
    if matches!(state, JobState::Queued | JobState::Running) {
        let model = model.clone();
        buttons = buttons.child(
            button(("pause-resume", id as usize), "Pause")
                .secondary()
                .on_click(move |_, _, cx| {
                    model.update(cx, |m, cx| {
                        m.job_call(id, cx, move |c| c.pause_job(id).map(drop))
                    });
                }),
        );
    }
    if matches!(state, JobState::Paused | JobState::Failed) {
        let model = model.clone();
        buttons = buttons.child(
            button(("pause-resume", id as usize), "Resume")
                .secondary()
                .on_click(move |_, _, cx| {
                    model.update(cx, |m, cx| {
                        m.job_call(id, cx, move |c| c.resume_job(id).map(drop))
                    });
                }),
        );
    }
    if queue::is_unfinished(job) {
        let model = model.clone();
        buttons = buttons.child(button(("cancel", id as usize), "Cancel").danger().on_click(
            move |_, _, cx| {
                model.update(cx, |m, cx| {
                    m.job_call(id, cx, move |c| c.cancel_job(id).map(drop))
                });
            },
        ));
    }
    if let Some((index, len)) = position {
        for (label, mv, enabled) in [
            ("Move up", Move::Up, index > 0),
            ("Move down", Move::Down, index + 1 < len),
            ("Move to top", Move::ToTop, index > 0),
        ] {
            let model = model.clone();
            buttons = buttons.child(
                button(SharedString::from(format!("{label}-{id}")), label)
                    .ghost()
                    .disabled(!enabled)
                    .on_click(move |_, _, cx| reorder(&model, index, mv, cx)),
            );
        }
    }

    div()
        .flex()
        .flex_col()
        .gap_1()
        .py_2()
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(div().text_sm().child(name))
                .child(
                    div()
                        .text_xs()
                        .text_color(p.muted)
                        .child(format::kind(job.kind()).to_owned()),
                )
                .child(div().flex_1())
                .child(buttons),
        )
        .when(state == JobState::Running || started, |d| {
            d.child(progress_bar(format::fraction(&progress), cx))
        })
        .child(
            div()
                .flex()
                .gap_3()
                .child(muted(detail, cx))
                .when(!speed.is_empty(), |d| d.child(muted(speed, cx))),
        )
        .when_some(error, |d, e| d.child(error_text(e, cx)))
        .when_some(call_error, |d, e| d.child(error_text(e, cx)))
        .into_any_element()
}

impl Focusable for DownloadsPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for DownloadsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        reveal::scroll_to_focus(&self.focus, &self.scroll, window, cx);
        let m = self.model.read(cx);
        let logged_in = m.store.account().is_some();
        let jobs = m.store.jobs.clone();
        let queue = queue::queue(jobs.values());
        let history = queue::history(jobs.values());
        let len = queue.len();

        let mut queue_list = div().flex().flex_col();
        for (i, j) in queue.iter().enumerate() {
            queue_list = queue_list.child(job_row(&self.model, j, Some((i, len)), cx));
        }
        let mut history_list = div().flex().flex_col();
        for j in &history {
            history_list = history_list.child(job_row(&self.model, j, None, cx));
        }

        page_scroll::by_keyboard(div().id("downloads-page"), &self.scroll, None)
            .track_focus(&self.focus)
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .p_6()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(div().text_xl().child("Downloads"))
                    .child(div().flex_1())
                    .child(
                        button("update-all", "Update all")
                            .secondary()
                            .disabled(!logged_in)
                            .on_click(cx.listener(|page, _, _, cx| page.update_all(cx))),
                    ),
            )
            .when_some(self.update_all_error.clone(), |d, e| {
                d.child(error_text(e, cx))
            })
            .when(!logged_in, |d| {
                d.child(muted("Jobs belong to an account; sign in to see them.", cx))
            })
            .child(
                section("Queue", cx).child(panel(cx).child(if queue.is_empty() {
                    muted("Nothing is queued.", cx).into_any_element()
                } else {
                    queue_list.into_any_element()
                })),
            )
            .child(
                section("History", cx).child(panel(cx).child(if history.is_empty() {
                    muted("No finished jobs since ossteamd started.", cx).into_any_element()
                } else {
                    history_list.into_any_element()
                })),
            )
    }
}
