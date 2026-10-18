// SPDX-License-Identifier: GPL-2.0-or-later
//! The download queue's order and reordering (`gui.md` §4.4).

use ossteam_ipc::v1::{self, JobState};

pub fn is_unfinished(j: &v1::Job) -> bool {
    matches!(
        j.state(),
        JobState::Queued | JobState::Running | JobState::Paused | JobState::Failed
    )
}

/// Unfinished jobs in the order the daemon runs them: priority, highest
/// first, then creation time (ipc.md §5.6).
pub fn queue<'a>(jobs: impl IntoIterator<Item = &'a v1::Job>) -> Vec<&'a v1::Job> {
    let mut q: Vec<_> = jobs.into_iter().filter(|j| is_unfinished(j)).collect();
    q.sort_by_key(|j| sort_key(j));
    q
}

/// Finished jobs, newest first.
pub fn history<'a>(jobs: impl IntoIterator<Item = &'a v1::Job>) -> Vec<&'a v1::Job> {
    let mut h: Vec<_> = jobs.into_iter().filter(|j| !is_unfinished(j)).collect();
    h.sort_by_key(|j| std::cmp::Reverse((j.created, j.job_id)));
    h
}

fn sort_key(j: &v1::Job) -> (std::cmp::Reverse<i32>, u64, u64) {
    (std::cmp::Reverse(j.priority), j.created, j.job_id)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Move {
    Up,
    Down,
    ToTop,
}

/// The queue's job ids after moving the job at `index`; `None` when it
/// cannot move that way.
pub fn moved(queue: &[&v1::Job], index: usize, m: Move) -> Option<Vec<u64>> {
    let mut ids: Vec<u64> = queue.iter().map(|j| j.job_id).collect();
    match m {
        Move::Up | Move::ToTop if index == 0 || index >= ids.len() => return None,
        Move::Down if index + 1 >= ids.len() => return None,
        Move::Up => ids.swap(index, index - 1),
        Move::Down => ids.swap(index, index + 1),
        Move::ToTop => {
            let id = ids.remove(index);
            ids.insert(0, id);
        }
    }
    Some(ids)
}

/// The `SetJobPriority` calls that make the daemon run `queue` in the
/// order `wanted`, with as few changes as possible. From the bottom up,
/// each job keeps its priority if it already sorts before the job after
/// it, and is otherwise raised just enough to (§4.4). Jobs only rise, so
/// what is below stays in order.
pub fn priorities(queue: &[&v1::Job], wanted: &[u64]) -> Vec<(u64, i32)> {
    let mut calls = Vec::new();
    // The job below, as (priority after the calls, created, id).
    let mut below: Option<(i32, u64, u64)> = None;
    for id in wanted.iter().rev() {
        let Some(j) = queue.iter().find(|j| j.job_id == *id) else {
            continue;
        };
        let mut p = j.priority;
        if let Some((bp, bcreated, bid)) = below {
            let before = p > bp || (p == bp && (j.created, j.job_id) < (bcreated, bid));
            if !before {
                p = if (j.created, j.job_id) < (bcreated, bid) {
                    bp
                } else {
                    bp.saturating_add(1)
                };
                calls.push((j.job_id, p));
            }
        }
        below = Some((p, j.created, j.job_id));
    }
    calls.reverse();
    calls
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(id: u64, priority: i32, created: u64, state: JobState) -> v1::Job {
        v1::Job {
            job_id: id,
            priority,
            created,
            state: state as i32,
            ..Default::default()
        }
    }

    fn ids(q: &[&v1::Job]) -> Vec<u64> {
        q.iter().map(|j| j.job_id).collect()
    }

    /// Applies `calls` and returns the resulting queue order.
    fn after(jobs: &[v1::Job], calls: &[(u64, i32)]) -> Vec<u64> {
        let mut jobs = jobs.to_vec();
        for (id, p) in calls {
            jobs.iter_mut().find(|j| j.job_id == *id).unwrap().priority = *p;
        }
        ids(&queue(&jobs))
    }

    #[test]
    fn the_queue_is_in_daemon_order_and_history_newest_first() {
        let jobs = [
            job(1, 0, 10, JobState::Running),
            job(2, 5, 30, JobState::Queued),
            job(3, 0, 20, JobState::Failed),
            job(4, 0, 5, JobState::Done),
            job(5, 0, 50, JobState::Cancelled),
            job(6, 5, 25, JobState::Paused),
        ];
        assert_eq!(ids(&queue(&jobs)), [6, 2, 1, 3]);
        assert_eq!(ids(&history(&jobs)), [5, 4]);
    }

    #[test]
    fn moves_need_the_fewest_priority_changes() {
        let jobs = [
            job(1, 0, 10, JobState::Running),
            job(2, 0, 20, JobState::Queued),
            job(3, 0, 30, JobState::Queued),
            job(4, 0, 40, JobState::Queued),
        ];
        let q = queue(&jobs);

        let wanted = moved(&q, 3, Move::ToTop).unwrap();
        assert_eq!(wanted, [4, 1, 2, 3]);
        let calls = priorities(&q, &wanted);
        assert_eq!(calls, [(4, 1)]);
        assert_eq!(after(&jobs, &calls), wanted);

        let wanted = moved(&q, 0, Move::Down).unwrap();
        let calls = priorities(&q, &wanted);
        assert_eq!(calls, [(2, 1)]);
        assert_eq!(after(&jobs, &calls), wanted);

        let wanted = moved(&q, 2, Move::Up).unwrap();
        assert_eq!(after(&jobs, &priorities(&q, &wanted)), wanted);

        assert_eq!(moved(&q, 0, Move::Up), None);
        assert_eq!(moved(&q, 0, Move::ToTop), None);
        assert_eq!(moved(&q, 3, Move::Down), None);
    }

    #[test]
    fn raised_jobs_stay_above_higher_priorities_below() {
        let jobs = [
            job(1, 7, 10, JobState::Queued),
            job(2, 3, 5, JobState::Queued),
            job(3, -2, 1, JobState::Queued),
        ];
        let q = queue(&jobs);
        assert_eq!(ids(&q), [1, 2, 3]);
        for (index, m) in [
            (2, Move::ToTop),
            (2, Move::Up),
            (0, Move::Down),
            (1, Move::ToTop),
        ] {
            let wanted = moved(&q, index, m).unwrap();
            assert_eq!(
                after(&jobs, &priorities(&q, &wanted)),
                wanted,
                "{index} {m:?}"
            );
        }
        // Job 3 was created first, so it needs only to equal job 1.
        assert_eq!(priorities(&q, &[3, 1, 2]), [(3, 7)]);
    }
}
