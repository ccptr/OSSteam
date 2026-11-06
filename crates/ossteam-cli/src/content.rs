// SPDX-License-Identifier: GPL-2.0-or-later
//! `install`, `update`, `verify`, `uninstall`, `jobs` and `folders`
//! (SPEC §15.2; `docs/spec/ipc.md` §5.5, §5.6).
//!
//! The job commands queue a job and print it. With `--wait` they follow
//! the job until it ends, drawing its progress on stderr when stderr is a
//! terminal (CLI-3). Stopping the wait leaves the job running.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::{IsTerminal, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use ossteam_ipc::Client;
use ossteam_ipc::v1::{self, JobKind, JobPhase, JobState, Topic, event};
use serde_json::{Value, json};

use crate::events::error_json;
use crate::library::{resolve, size};
use crate::{Cli, Failure, connect, exit, exit_code};

/// How often the progress line is redrawn at most.
const REDRAW: Duration = Duration::from_millis(200);

pub fn install(
    cli: &Cli,
    app: &str,
    folder: Option<&Path>,
    branch: Option<&str>,
    wait: bool,
) -> Result<(), Failure> {
    let client = connect(cli, true)?;
    let id = resolve(&client, app)?;
    let folder = match folder {
        Some(f) => std::path::absolute(f)
            .map_err(|e| Failure::new(exit::OTHER, format!("{}: {e}", f.display())))?
            .to_string_lossy()
            .into_owned(),
        None => String::new(),
    };
    if wait {
        subscribe(&client)?;
    }
    let job = client.install(id, &folder, branch.unwrap_or_default())?;
    finish(cli, &client, vec![job], wait, false)
}

pub fn update(cli: &Cli, apps: &[String], all: bool, wait: bool) -> Result<(), Failure> {
    let client = connect(cli, true)?;
    let ids = apps
        .iter()
        .map(|a| resolve(&client, a))
        .collect::<Result<Vec<_>, _>>()?;
    if wait {
        subscribe(&client)?;
    }
    let jobs = client.update(&ids, all)?;
    if !cli.json && !cli.quiet {
        // An app that already has an unfinished job gets no update job
        // (`ipc.md` §5.6); name that job rather than saying nothing.
        let skipped: Vec<u32> = ids
            .iter()
            .copied()
            .filter(|id| !jobs.iter().any(|j| j.app_id == *id))
            .collect();
        if !skipped.is_empty() {
            for j in client.list_jobs()? {
                if skipped.contains(&j.app_id) && unfinished(j.state()) {
                    println!("{}", held_by(&j));
                }
            }
        }
        if jobs.is_empty() {
            if skipped.is_empty() || ids.is_empty() {
                println!("no updates to install");
            }
            return Ok(());
        }
    }
    finish(cli, &client, jobs, wait, true)
}

pub fn verify(cli: &Cli, app: &str, wait: bool) -> Result<(), Failure> {
    let client = connect(cli, true)?;
    let id = resolve(&client, app)?;
    if wait {
        subscribe(&client)?;
    }
    let job = client.verify(id)?;
    finish(cli, &client, vec![job], wait, false)
}

pub fn uninstall(
    cli: &Cli,
    app: &str,
    yes: bool,
    remove_compat_data: bool,
    wait: bool,
) -> Result<(), Failure> {
    let client = connect(cli, true)?;
    let id = resolve(&client, app)?;
    if !yes {
        let r = client.get_app(id)?;
        let name = r
            .app
            .as_ref()
            .filter(|a| !a.name.is_empty())
            .map_or_else(|| format!("app {id}"), |a| format!("{} ({id})", a.name));
        let mut what = match &r.installed {
            Some(i) => format!("Uninstall {name} and delete {} ({})", i.path, size(i.size)),
            None => format!("Uninstall {name} and delete what its install has written"),
        };
        if remove_compat_data && r.compat.is_some() {
            what.push_str(", and its compatibility tool prefix with its saves and settings");
        }
        what.push('?');
        if !std::io::stdin().is_terminal() {
            return Err(Failure::new(
                exit::OTHER,
                "standard input is not a terminal; pass --yes to uninstall without asking",
            ));
        }
        if !confirm(&what)? {
            return Err(Failure::new(exit::OTHER, "not uninstalled"));
        }
    }
    if wait {
        subscribe(&client)?;
    }
    let job = client.uninstall(id, remove_compat_data)?;
    finish(cli, &client, vec![job], wait, false)
}

fn confirm(question: &str) -> Result<bool, Failure> {
    eprint!("{question} [y/N] ");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .map_err(|e| Failure::new(exit::OTHER, format!("reading standard input: {e}")))?;
    Ok(matches!(line.trim(), "y" | "Y" | "yes" | "Yes" | "YES"))
}

/// Subscribed before the request, so no change of the new job is missed.
/// DAEMON tells a daemon that stops from a lost connection.
fn subscribe(client: &Client) -> Result<(), Failure> {
    client.subscribe(&[Topic::Jobs, Topic::Daemon])?;
    Ok(())
}

/// Prints the jobs, after following them to their end when `wait`. `many`
/// prints a list even for one job, as `update` does.
fn finish(
    cli: &Cli,
    client: &Client,
    jobs: Vec<v1::Job>,
    wait: bool,
    many: bool,
) -> Result<(), Failure> {
    let jobs = if wait {
        follow(cli, client, jobs)?
    } else {
        jobs
    };
    if cli.json {
        let doc = if many {
            json!({ "jobs": jobs.iter().map(job_json).collect::<Vec<_>>() })
        } else {
            json!({ "job": jobs.first().map(job_json) })
        };
        println!("{doc}");
    } else if !cli.quiet {
        for j in &jobs {
            println!("{}", outcome(j));
        }
    }
    // The first failure decides the exit code. With --json the document
    // above already carries the error.
    let failed = jobs.iter().find_map(|j| match j.state() {
        JobState::Failed => Some((
            j.error
                .as_ref()
                .map_or(exit::OTHER, |e| exit_code(e.code())),
            format!(
                "job {} failed: {}",
                j.job_id,
                j.error
                    .as_ref()
                    .map_or("no reason given", |e| e.message.as_str())
            ),
        )),
        JobState::Cancelled if wait => {
            Some((exit::OTHER, format!("job {} was cancelled", j.job_id)))
        }
        _ => None,
    });
    match failed {
        Some((code, message)) => Err(Failure {
            code,
            message: (!cli.json).then_some(message),
        }),
        None => Ok(()),
    }
}

/// One line for a job as a command leaves it.
fn outcome(j: &v1::Job) -> String {
    let what = format!("{} of app {}", kind_name(j.kind()), j.app_id);
    match j.state() {
        JobState::Queued => format!("queued the {what} (job {})", j.job_id),
        JobState::Running => format!("started the {what} (job {})", j.job_id),
        JobState::Paused => format!("the {what} is paused (job {})", j.job_id),
        JobState::Done => format!("finished the {what} (job {})", j.job_id),
        JobState::Failed => format!("the {what} failed (job {})", j.job_id),
        JobState::Cancelled => format!("the {what} was cancelled (job {})", j.job_id),
        JobState::Unspecified => format!("the {what} is in an unknown state (job {})", j.job_id),
    }
}

fn finished(s: JobState) -> bool {
    matches!(s, JobState::Done | JobState::Failed | JobState::Cancelled)
}

/// Follows `jobs` until each has ended, and returns them as they ended.
fn follow(cli: &Cli, client: &Client, jobs: Vec<v1::Job>) -> Result<Vec<v1::Job>, Failure> {
    let mut jobs: BTreeMap<u64, v1::Job> = jobs.into_iter().map(|j| (j.job_id, j)).collect();
    let draw = !cli.quiet && !cli.json && std::io::stderr().is_terminal();
    let mut bar = Bar::default();
    if draw && jobs.values().any(|j| !finished(j.state())) {
        eprintln!("(Ctrl-C stops waiting; the job keeps running)");
    }
    while jobs.values().any(|j| !finished(j.state())) {
        let Some(ev) = client.next_event() else {
            bar.clear(draw);
            return Err(Failure::new(
                exit::DAEMON_UNAVAILABLE,
                "the connection to the daemon was lost",
            ));
        };
        match ev.kind {
            Some(event::Kind::JobStateChanged(v1::JobStateChanged { job: Some(j) })) => {
                if let Some(old) = jobs.get_mut(&j.job_id) {
                    if draw && j.state() != old.state() && !finished(j.state()) {
                        bar.clear(true);
                        eprintln!("job {}: {}", j.job_id, state_name(j.state()));
                    }
                    *old = j;
                }
            }
            Some(event::Kind::JobProgress(v1::JobProgress {
                job_id,
                progress: Some(p),
            })) => {
                if let Some(j) = jobs.get_mut(&job_id) {
                    j.progress = Some(p);
                }
            }
            Some(event::Kind::DaemonStopping(_)) => {
                bar.clear(draw);
                return Err(Failure::new(
                    exit::DAEMON_UNAVAILABLE,
                    "the daemon stopped; the job continues when it starts again",
                ));
            }
            _ => continue,
        }
        if draw {
            bar.draw(&running(&jobs));
        }
    }
    bar.clear(draw);
    Ok(jobs.into_values().collect())
}

fn running(jobs: &BTreeMap<u64, v1::Job>) -> Vec<&v1::Job> {
    jobs.values()
        .filter(|j| j.state() == JobState::Running)
        .collect()
}

/// The progress line on stderr.
#[derive(Default)]
struct Bar {
    drawn: Option<Instant>,
    /// Downloaded bytes and when, for the rate.
    last: Option<(u64, Instant)>,
    rate: f64,
    shown: bool,
}

impl Bar {
    fn draw(&mut self, running: &[&v1::Job]) {
        let now = Instant::now();
        if self.drawn.is_some_and(|t| now - t < REDRAW) {
            return;
        }
        self.drawn = Some(now);
        let downloaded: u64 = running
            .iter()
            .filter_map(|j| j.progress.as_ref())
            .map(|p| p.downloaded)
            .sum();
        if let Some((before, at)) = self.last {
            let secs = (now - at).as_secs_f64();
            if secs > 0.0 {
                let r = downloaded.saturating_sub(before) as f64 / secs;
                self.rate = if self.rate == 0.0 {
                    r
                } else {
                    0.7 * self.rate + 0.3 * r
                };
            }
        }
        self.last = Some((downloaded, now));
        let mut line = String::new();
        for j in running {
            if !line.is_empty() {
                line.push_str("  ");
            }
            let _ = write!(
                line,
                "{} {}",
                j.app_id,
                j.progress.as_ref().map(progress_text).unwrap_or_default()
            );
        }
        if self.rate > 0.0
            && running.iter().any(|j| {
                j.progress
                    .as_ref()
                    .is_some_and(|p| p.phase() == JobPhase::Downloading)
            })
        {
            let _ = write!(line, "  {}/s", size(self.rate as u64));
        }
        eprint!("\r{line}\x1b[K");
        let _ = std::io::stderr().flush();
        self.shown = true;
    }

    fn clear(&mut self, draw: bool) {
        if draw && self.shown {
            eprint!("\r\x1b[K");
            let _ = std::io::stderr().flush();
            self.shown = false;
        }
    }
}

fn percent(done: u64, total: u64) -> String {
    if total == 0 {
        return String::new();
    }
    format!(" {:.0}%", done.min(total) as f64 * 100.0 / total as f64)
}

/// A job's progress in a few words.
pub fn progress_text(p: &v1::JobProgressInfo) -> String {
    let phase = phase_name(p.phase());
    match p.phase() {
        JobPhase::Downloading => format!(
            "{phase}{} ({} of {})",
            percent(p.downloaded, p.download_total),
            size(p.downloaded),
            size(p.download_total)
        ),
        JobPhase::Checking | JobPhase::Copying => {
            format!("{phase}{}", percent(p.bytes_done, p.bytes_total))
        }
        _ => phase.to_owned(),
    }
}

fn kind_name(k: JobKind) -> &'static str {
    match k {
        JobKind::Install => "install",
        JobKind::Update => "update",
        JobKind::Verify => "verify",
        JobKind::Uninstall => "uninstall",
        JobKind::Unspecified => "unknown",
    }
}

fn state_name(s: JobState) -> &'static str {
    match s {
        JobState::Queued => "queued",
        JobState::Running => "running",
        JobState::Paused => "paused",
        JobState::Done => "done",
        JobState::Failed => "failed",
        JobState::Cancelled => "cancelled",
        JobState::Unspecified => "unknown",
    }
}

/// The daemon's unfinished jobs, failed ones included (`ipc.md` §5.6).
fn unfinished(s: JobState) -> bool {
    matches!(
        s,
        JobState::Queued | JobState::Running | JobState::Paused | JobState::Failed
    )
}

/// Why an update of `j`'s app was not queued: `j` is unfinished.
fn held_by(j: &v1::Job) -> String {
    let what = format!(
        "app {} has a {} {} (job {})",
        j.app_id,
        state_name(j.state()),
        kind_name(j.kind()),
        j.job_id
    );
    match (j.state(), &j.error) {
        (JobState::Failed, Some(e)) => format!(
            "{what}: {}; `ossteam jobs resume {}` tries it again",
            e.message, j.job_id
        ),
        (JobState::Failed | JobState::Paused, _) => {
            format!("{what}; `ossteam jobs resume {}` goes on with it", j.job_id)
        }
        _ => what,
    }
}

fn phase_name(p: JobPhase) -> &'static str {
    match p {
        JobPhase::Preparing => "preparing",
        JobPhase::Checking => "checking",
        JobPhase::Copying => "copying",
        JobPhase::Downloading => "downloading",
        JobPhase::Finishing => "finishing",
        JobPhase::Unspecified => "starting",
    }
}

pub fn progress_json(p: &v1::JobProgressInfo) -> Value {
    json!({
        "phase": phase_name(p.phase()),
        "bytes_total": p.bytes_total,
        "bytes_done": p.bytes_done,
        "download_total": p.download_total,
        "downloaded": p.downloaded,
        "files_total": p.files_total,
        "files_done": p.files_done,
    })
}

pub fn job_json(j: &v1::Job) -> Value {
    json!({
        "job_id": j.job_id,
        "app_id": j.app_id,
        "kind": kind_name(j.kind()),
        "state": state_name(j.state()),
        "priority": j.priority,
        "folder": j.folder,
        "branch": j.branch,
        "created": j.created,
        "progress": j.progress.as_ref().map(progress_json),
        "error": j.error.as_ref().map(error_json),
    })
}

/// A job on one line, for `events`.
pub fn job_line(j: &v1::Job) -> String {
    let mut line = format!(
        "{} {} of app {} {}",
        j.job_id,
        kind_name(j.kind()),
        j.app_id,
        state_name(j.state())
    );
    if let Some(e) = &j.error {
        let _ = write!(line, ": {}", e.message);
    }
    line
}

// --- jobs ---------------------------------------------------------------

pub fn list_jobs(cli: &Cli) -> Result<(), Failure> {
    let client = connect(cli, true)?;
    let jobs = client.list_jobs()?;
    if cli.json {
        println!(
            "{}",
            json!({ "jobs": jobs.iter().map(job_json).collect::<Vec<_>>() })
        );
    } else if !cli.quiet {
        print!("{}", jobs_text(&jobs));
    }
    Ok(())
}

fn jobs_text(jobs: &[v1::Job]) -> String {
    let mut out = String::new();
    if jobs.is_empty() {
        return out;
    }
    let _ = writeln!(
        out,
        "{:<6}  {:<10}  {:<9}  {:<9}  {:>4}  PROGRESS",
        "JOB", "APP", "KIND", "STATE", "PRI"
    );
    for j in jobs {
        let detail = match (&j.error, j.state()) {
            (Some(e), JobState::Failed) => e.message.clone(),
            (_, JobState::Running) => j.progress.as_ref().map(progress_text).unwrap_or_default(),
            // How far a job that stopped part way got.
            (_, JobState::Paused | JobState::Queued) => j
                .progress
                .as_ref()
                .filter(|p| p.download_total > 0 || p.bytes_total > 0)
                .map(progress_text)
                .unwrap_or_default(),
            _ => String::new(),
        };
        let line = format!(
            "{:<6}  {:<10}  {:<9}  {:<9}  {:>4}  {detail}",
            j.job_id,
            j.app_id,
            kind_name(j.kind()),
            state_name(j.state()),
            j.priority
        );
        let _ = writeln!(out, "{}", line.trim_end());
    }
    out
}

/// What `jobs pause|resume|cancel|priority` do to a job.
#[derive(Clone, Copy)]
pub enum Control {
    Pause,
    Resume,
    Cancel,
    Priority(i32),
}

pub fn control(cli: &Cli, job_id: u64, c: Control) -> Result<(), Failure> {
    let client = connect(cli, true)?;
    let job = match c {
        Control::Pause => client.pause_job(job_id)?,
        Control::Resume => client.resume_job(job_id)?,
        Control::Cancel => client.cancel_job(job_id)?,
        Control::Priority(p) => client.set_job_priority(job_id, p)?,
    };
    if cli.json {
        println!("{}", json!({ "job": job_json(&job) }));
    } else if !cli.quiet {
        println!("{}", job_line(&job));
    }
    Ok(())
}

// --- folders ------------------------------------------------------------

fn folder_json(f: &v1::LibraryFolder) -> Value {
    json!({
        "path": f.path,
        "default": f.is_default,
        "available": f.available,
        "free_bytes": f.available.then_some(f.free_bytes),
        "app_ids": f.app_ids,
    })
}

pub fn list_folders(cli: &Cli) -> Result<(), Failure> {
    let client = connect(cli, true)?;
    let folders = client.list_library_folders()?;
    if cli.json {
        println!(
            "{}",
            json!({ "folders": folders.iter().map(folder_json).collect::<Vec<_>>() })
        );
    } else if !cli.quiet {
        print!("{}", folders_text(&folders));
    }
    Ok(())
}

fn folders_text(folders: &[v1::LibraryFolder]) -> String {
    let mut out = String::new();
    for f in folders {
        let mut line = f.path.clone();
        if f.is_default {
            line.push_str(" (default)");
        }
        let n = f.app_ids.len();
        let apps = format!("{n} app{}", if n == 1 { "" } else { "s" });
        if f.available {
            let _ = write!(line, "  {} free, {apps}", size(f.free_bytes));
        } else {
            let _ = write!(line, "  not available, {apps}");
        }
        let _ = writeln!(out, "{line}");
    }
    out
}

pub fn add_folder(cli: &Cli, path: &Path) -> Result<(), Failure> {
    let path = std::path::absolute(path)
        .map_err(|e| Failure::new(exit::OTHER, format!("{}: {e}", path.display())))?;
    let client = connect(cli, true)?;
    let f = client.add_library_folder(&path.to_string_lossy())?;
    if cli.json {
        println!("{}", json!({ "folder": folder_json(&f) }));
    } else if !cli.quiet {
        println!("added {}", f.path);
    }
    Ok(())
}

pub fn remove_folder(cli: &Cli, path: &Path) -> Result<(), Failure> {
    let path = std::path::absolute(path)
        .map_err(|e| Failure::new(exit::OTHER, format!("{}: {e}", path.display())))?;
    let client = connect(cli, true)?;
    let path = path.to_string_lossy();
    client.remove_library_folder(&path)?;
    if cli.json {
        println!("{}", json!({ "removed": path }));
    } else if !cli.quiet {
        println!("removed {path} from the list; its files are left in place");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(id: u64, kind: JobKind, state: JobState) -> v1::Job {
        v1::Job {
            job_id: id,
            app_id: 480,
            kind: kind as i32,
            state: state as i32,
            ..Default::default()
        }
    }

    #[test]
    fn progress_reads_by_phase() {
        let p = v1::JobProgressInfo {
            phase: JobPhase::Downloading as i32,
            download_total: 4 << 20,
            downloaded: 1 << 20,
            ..Default::default()
        };
        assert_eq!(progress_text(&p), "downloading 25% (1.0 MiB of 4.0 MiB)");
        let p = v1::JobProgressInfo {
            phase: JobPhase::Checking as i32,
            bytes_total: 10,
            bytes_done: 5,
            ..Default::default()
        };
        assert_eq!(progress_text(&p), "checking 50%");
        assert_eq!(
            progress_text(&v1::JobProgressInfo {
                phase: JobPhase::Finishing as i32,
                ..Default::default()
            }),
            "finishing"
        );
    }

    #[test]
    fn update_held_by_a_job() {
        let mut failed = job(17, JobKind::Update, JobState::Failed);
        failed.error = Some(v1::Error {
            message: "HTTP 503".into(),
            ..Default::default()
        });
        assert_eq!(
            held_by(&failed),
            "app 480 has a failed update (job 17): HTTP 503; \
             `ossteam jobs resume 17` tries it again"
        );
        assert_eq!(
            held_by(&job(4, JobKind::Verify, JobState::Running)),
            "app 480 has a running verify (job 4)"
        );
        assert!(unfinished(JobState::Failed));
        assert!(!unfinished(JobState::Done));
    }

    #[test]
    fn job_listing() {
        let mut failed = job(2, JobKind::Update, JobState::Failed);
        failed.error = Some(v1::Error {
            message: "no content server answered".into(),
            ..Default::default()
        });
        let mut paused = job(3, JobKind::Install, JobState::Paused);
        paused.progress = Some(v1::JobProgressInfo {
            phase: JobPhase::Downloading.into(),
            download_total: 4 << 20,
            downloaded: 1 << 20,
            ..Default::default()
        });
        let text = jobs_text(&[job(1, JobKind::Install, JobState::Queued), failed, paused]);
        assert_eq!(
            text,
            "JOB     APP         KIND       STATE       PRI  PROGRESS\n\
             1       480         install    queued        0\n\
             2       480         update     failed        0  no content server answered\n\
             3       480         install    paused        0  downloading 25% (1.0 MiB of 4.0 MiB)\n"
        );
        assert!(jobs_text(&[]).is_empty());
    }

    #[test]
    fn outcomes_and_folders() {
        assert_eq!(
            outcome(&job(3, JobKind::Verify, JobState::Done)),
            "finished the verify of app 480 (job 3)"
        );
        let folders = [
            v1::LibraryFolder {
                path: "/lib".into(),
                is_default: true,
                available: true,
                free_bytes: 1 << 30,
                app_ids: vec![480],
            },
            v1::LibraryFolder {
                path: "/mnt/usb".into(),
                ..Default::default()
            },
        ];
        assert_eq!(
            folders_text(&folders),
            "/lib (default)  1.0 GiB free, 1 app\n/mnt/usb  not available, 0 apps\n"
        );
    }
}
