// SPDX-License-Identifier: GPL-2.0-or-later
//! Install scripts in a Proton prefix (`compat-tools.md` §7): its
//! registry read from Wine's files, changes imported with Wine's
//! `regedit`, and Run Process entries, each as a setup command through
//! the launch's chain.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ossteam_launch::command;
use ossteam_launch::compat::registry::{self, Change, Registry, Value};
use ossteam_launch::isolation;
use ossteam_launch::process::{self, Exit, Game};
use ossteam_types::{AppId, CancelToken};

use super::{Data, Outcome, Step, System, ValueRef};

/// The file a start's registry changes are imported from, in the app's
/// compatibility data directory.
pub const IMPORT_FILE: &str = "ossteam-script.reg";

/// How an app's setup commands run (`compat-tools.md` §7).
#[derive(Clone)]
pub struct Setup {
    pub app: AppId,
    /// The chain's command line up to the command, with `%verb%` `run`.
    pub chain: String,
    /// The `STEAM_COMPAT_*` variables, without the game's own
    /// (`compat-tools.md` §5.2).
    pub env: Vec<(String, String)>,
    /// The generated client-install directory.
    pub working_dir: PathBuf,
    /// The game's log, which they add to (`launch.md` §3).
    pub log: PathBuf,
    pub reaper: PathBuf,
    /// The launch's private `~/.steam`.
    pub isolation: Option<Arc<isolation::Plan>>,
}

impl Setup {
    /// Runs `command` after the chain, and waits for every process it
    /// starts. Its exit code.
    pub fn run(&self, command: &str) -> Result<i64, String> {
        let spec = process::Spec {
            app: self.app,
            executable: PathBuf::from("/bin/sh"),
            command_line: format!("{} {command}", self.chain),
            working_dir: self.working_dir.clone(),
            env: self.env.clone(),
            log: self.log.clone(),
            append_log: true,
            reaper: self.reaper.clone(),
            interrupt_first: false,
            isolation: self.isolation.clone(),
        };
        tracing::info!(app = %self.app, command = %spec.command_line, "setup command");
        let game = Game::spawn(&spec).map_err(|e| format!("starting it: {e}"))?;
        match game.wait().map_err(|e| format!("waiting for it: {e}"))? {
            Exit::Code(c) => Ok(c),
            Exit::Signal(s) => Err(format!("signal {s} ended it")),
        }
    }
}

/// `regedit /S` of a file, by its path in the prefix.
fn regedit(path: &str) -> String {
    format!("regedit /S {}", command::shell_quote(path))
}

/// A prefix as install scripts see it.
pub struct Prefix {
    compat_data: PathBuf,
    registry: Registry,
    /// The user directory under `C:\users`.
    user: String,
    setup: Setup,
}

impl Prefix {
    /// Opens the prefix in `compat_data`, making it first with an empty
    /// import when it has no registry yet (`compat-tools.md` §7).
    pub fn open(
        compat_data: &Path,
        setup: Setup,
        report: &mut dyn FnMut(String),
    ) -> Result<Self, String> {
        let pfx = registry::prefix_dir(compat_data);
        if !pfx.join("system.reg").exists() {
            report("Making the Proton prefix".into());
            let code = import(compat_data, &setup, &[])?;
            if code != 0 {
                tracing::warn!(app = %setup.app, "making the prefix: regedit exited with {code}");
            }
        }
        let registry =
            Registry::read(&pfx).map_err(|e| format!("reading {}: {e}", pfx.display()))?;
        if registry::wine_path(&pfx, compat_data).is_none() {
            return Err(format!(
                "no drive of {} holds {}",
                pfx.display(),
                compat_data.display()
            ));
        }
        let user = registry::user(&pfx).unwrap_or_else(|| "steamuser".into());
        Ok(Self {
            compat_data: compat_data.to_owned(),
            registry,
            user,
            setup,
        })
    }

    fn change(&self, step: &Step) -> Option<Change> {
        let (value, data) = match step {
            Step::Set { value, data } => (value, Some(data)),
            Step::Delete(value) => (value, None),
            _ => return None,
        };
        Some(Change {
            machine: value.key.machine,
            path: registry::view(
                value.key.machine,
                &value.key.path,
                self.registry.machine.win64,
            ),
            name: value.name.clone(),
            value: data.map(|d| match d {
                Data::String(s) => Value::String(s.clone()),
                Data::Dword(n) => Value::Dword(*n),
                Data::Other => Value::Other,
            }),
        })
    }

    /// Imports `changes`; `Done` for each when `regedit` exits with 0.
    fn import(&self, changes: &[Change]) -> Outcome {
        match import(&self.compat_data, &self.setup, changes) {
            Ok(0) => Outcome::Done,
            Ok(code) => Outcome::Failed(format!("regedit exited with {code}")),
            Err(e) => Outcome::Failed(format!("regedit: {e}")),
        }
    }

    fn run(&self, program: &str, arguments: &str, has_run: Option<&ValueRef>) -> Outcome {
        // A Windows command line, which no shell may split (`compat-tools.md`
        // §7): its words, each quoted.
        let words = command::windows_split(arguments);
        let line = command::shell_command_line(Path::new(program), "", &words);
        match self.setup.run(&line) {
            Ok(0) => {
                if let Some(v) = has_run {
                    let set = Step::Set {
                        value: v.clone(),
                        data: Data::Dword(1),
                    };
                    let change = self.change(&set).expect("a registry step");
                    if let Outcome::Failed(e) = self.import(&[change]) {
                        // It ran, so it counts; it runs again at the next start.
                        tracing::warn!("recording that {program} ran in {v}: {e}");
                    }
                }
                Outcome::Done
            }
            Ok(code) => Outcome::Failed(format!("exit code {code}")),
            Err(e) => Outcome::Failed(e),
        }
    }

    fn users(&self, rest: &str) -> String {
        format!("C:\\users\\{}\\{rest}", self.user)
    }
}

/// A host path as programs in the prefix name it (`compat-tools.md` §7).
/// Before the prefix is made it has no drives yet; Proton then links Z:
/// to `/`.
fn wine_path(compat_data: &Path, host: &Path) -> String {
    registry::wine_path(&registry::prefix_dir(compat_data), host)
        .unwrap_or_else(|| registry::dos_path("Z:", host))
}

/// Writes `changes` to the import file and runs `regedit` on it.
fn import(compat_data: &Path, setup: &Setup, changes: &[Change]) -> Result<i64, String> {
    let file = compat_data.join(IMPORT_FILE);
    std::fs::create_dir_all(compat_data)
        .and_then(|()| std::fs::write(&file, registry::import_file(changes)))
        .map_err(|e| format!("writing {}: {e}", file.display()))?;
    let code = setup.run(&regedit(&wine_path(compat_data, &file)));
    let _ = std::fs::remove_file(&file);
    code
}

impl System for Prefix {
    fn value(&self, v: &ValueRef) -> Option<Data> {
        Some(
            match self
                .registry
                .get(v.key.machine, &v.key.path, v.name.as_deref())?
            {
                Value::String(s) => Data::String(s.clone()),
                Value::Dword(n) => Data::Dword(*n),
                Value::Other => Data::Other,
            },
        )
    }

    fn firewall_rule(&self, _: &str, _: &str) -> Option<bool> {
        None
    }

    fn elevates(&self) -> bool {
        false
    }

    fn path(&self, dir: &Path) -> String {
        wine_path(&self.compat_data, dir)
    }

    fn apply(&self, step: &Step, cancel: &CancelToken) -> Outcome {
        self.apply_all(std::slice::from_ref(step), cancel)
            .pop()
            .unwrap_or(Outcome::NotRun)
    }

    /// Registry steps next to each other go in one import.
    fn apply_all(&self, steps: &[Step], cancel: &CancelToken) -> Vec<Outcome> {
        let mut out = Vec::with_capacity(steps.len());
        let mut i = 0;
        while i < steps.len() {
            if cancel.is_cancelled() {
                out.resize(steps.len(), Outcome::NotRun);
                break;
            }
            let changes: Vec<Change> = steps[i..].iter().map_while(|s| self.change(s)).collect();
            if !changes.is_empty() {
                let o = self.import(&changes);
                out.extend(std::iter::repeat_n(o, changes.len()));
                i += changes.len();
                continue;
            }
            out.push(match &steps[i] {
                Step::Run {
                    program,
                    arguments,
                    has_run,
                } => self.run(program, arguments, has_run.as_ref()),
                Step::Firewall { .. } => Outcome::Failed("Wine has no firewall".into()),
                Step::Set { .. } | Step::Delete(_) => unreachable!("taken above"),
            });
            i += 1;
        }
        out
    }

    fn apply_elevated(&self, _: &[Step], _: &CancelToken) -> Result<Vec<Outcome>, String> {
        Err("nothing is elevated in a prefix".into())
    }

    fn is_64bit(&self) -> bool {
        self.registry.machine.win64
    }

    /// The version Wine reports, from the prefix's registry.
    fn os_type(&self) -> String {
        let key = "Software\\Microsoft\\Windows NT\\CurrentVersion";
        let get = |name| self.registry.get(true, key, Some(name));
        let number = |v: Option<&Value>| match v {
            Some(Value::Dword(n)) => Some(*n),
            Some(Value::String(s)) => s.trim().parse().ok(),
            _ => None,
        };
        let build = number(get("CurrentBuild")).unwrap_or(0);
        let (major, minor) = match number(get("CurrentMajorVersionNumber")) {
            Some(major) => (major, number(get("CurrentMinorVersionNumber")).unwrap_or(0)),
            None => match get("CurrentVersion") {
                Some(Value::String(v)) => {
                    let (a, b) = v.split_once('.').unwrap_or((v, "0"));
                    (a.parse().unwrap_or(10), b.parse().unwrap_or(0))
                }
                _ => (10, 0),
            },
        };
        super::os_type(major, minor, build).to_owned()
    }

    fn var(&self, name: &str) -> Option<String> {
        match name {
            "WINDIR" => Some("C:\\windows".into()),
            "APPDATA" => Some(self.users("AppData\\Roaming")),
            "LOCALAPPDATA" => Some(self.users("AppData\\Local")),
            "USER_MYDOCS" => Some(self.users("Documents")),
            "COMMON_MYDOCS" => Some("C:\\users\\Public\\Documents".into()),
            "STEAMPATH" => {
                match self
                    .registry
                    .get(true, "Software\\Valve\\Steam", Some("InstallPath"))
                {
                    Some(Value::String(s)) => Some(s.clone()),
                    _ => None,
                }
            }
            _ => None,
        }
    }
}
