// SPDX-License-Identifier: GPL-2.0-or-later
//! Install scripts in the daemon (`install-scripts.md`; SPEC CONT-8):
//! what is recorded with an install, and the steps a start, verify and
//! uninstall take. Parsing is `ossteam_content::script`'s. The steps
//! themselves reach Windows, or a Proton prefix (`compat-tools.md` §7),
//! through [`System`].

pub mod prefix;
#[cfg(windows)]
pub mod windows;

use std::fmt;
use std::path::{Path, PathBuf};

use ossteam_content::Manifest;
use ossteam_content::manifest::flags;
use ossteam_content::script::{self as parsed, Hive, Process, RegKey, Script};
use ossteam_types::{CancelToken, DepotId};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

/// A script file of an installed app (§1, §5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptFile {
    pub depot: DepotId,
    /// The name as the manifest gives it, `\`-separated.
    pub path: String,
    pub size: u64,
    pub sha1: Vec<u8>,
}

/// The scripts among a manifest's files.
pub fn files(m: &Manifest) -> impl Iterator<Item = ScriptFile> + '_ {
    m.files
        .iter()
        .filter(|f| f.flags & flags::INSTALL_SCRIPT != 0 && !f.is_dir() && !f.is_symlink())
        .map(|f| ScriptFile {
            depot: m.depot,
            path: f.name.clone(),
            size: f.size,
            sha1: f.sha.clone(),
        })
}

/// A registry key, as steps and the state database carry it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Key {
    /// `HKEY_LOCAL_MACHINE`; otherwise `HKEY_CURRENT_USER`.
    pub machine: bool,
    pub path: String,
}

impl From<&RegKey> for Key {
    fn from(k: &RegKey) -> Self {
        Self {
            machine: k.hive == Hive::LocalMachine,
            path: k.path.clone(),
        }
    }
}

/// A registry value: its key and its name, `None` for the key's default
/// value. The values a script wrote are recorded so (§5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValueRef {
    pub key: Key,
    pub name: Option<String>,
}

impl fmt::Display for ValueRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let hive = if self.key.machine {
            "HKEY_LOCAL_MACHINE"
        } else {
            "HKEY_CURRENT_USER"
        };
        let name = self.name.as_deref().unwrap_or("(Default)");
        write!(f, "{hive}\\{}\\{name}", self.key.path)
    }
}

/// A registry value's type and data, as far as scripts write them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Data {
    /// `REG_SZ`.
    String(String),
    /// `REG_DWORD`.
    Dword(u32),
    /// Another type, which no script writes.
    Other,
}

impl From<&parsed::Data> for Data {
    fn from(d: &parsed::Data) -> Self {
        match d {
            parsed::Data::String(s) => Self::String(s.clone()),
            parsed::Data::Dword(n) => Self::Dword(*n),
        }
    }
}

/// One thing done to the system. The elevated helper reads these, so
/// their form is shared with it, and with it only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Step {
    /// Writes a value (§2.1), creating its key.
    Set { value: ValueRef, data: Data },
    /// Deletes a value; one that is not there is fine.
    Delete(ValueRef),
    /// Adds an inbound firewall rule for a program (§2.3).
    Firewall { name: String, program: String },
    /// Runs a program and waits for it. When it exits with 0, `has_run`
    /// is set to 1 (§2.2).
    Run {
        program: String,
        arguments: String,
        has_run: Option<ValueRef>,
    },
}

impl fmt::Display for Step {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Set { value, data } => write!(f, "set {value} to {data:?}"),
            Self::Delete(v) => write!(f, "delete {v}"),
            Self::Firewall { name, program } => write!(f, "firewall rule {name:?} for {program}"),
            Self::Run {
                program, arguments, ..
            } => write!(f, "run {program} {arguments}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Outcome {
    Done,
    NotRun,
    Failed(String),
}

/// What the daemon gives the elevated helper (§4): the steps, and where
/// to write their outcomes. It travels on the helper's command line,
/// which nothing can change once the helper starts, unlike a file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelperRequest {
    pub steps: Vec<Step>,
    pub result: PathBuf,
}

impl HelperRequest {
    /// JSON in hex, so the command line needs no quoting.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn encode(&self) -> String {
        let json = serde_json::to_vec(self).expect("steps serialize");
        json.iter().map(|b| format!("{b:02x}")).collect()
    }

    pub fn decode(s: &str) -> anyhow::Result<Self> {
        anyhow::ensure!(
            s.len().is_multiple_of(2) && s.is_ascii(),
            "the request is not hex"
        );
        let bytes = (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16))
            .collect::<Result<Vec<u8>, _>>()?;
        Ok(serde_json::from_slice(&bytes)?)
    }
}

/// `ossteamd --script-helper`: takes the steps it is given, elevated, and
/// writes their outcomes (§4). It does nothing else.
pub fn helper_main(request: &str) -> anyhow::Result<()> {
    let request = HelperRequest::decode(request)?;
    #[cfg(windows)]
    {
        let cancel = CancelToken::new();
        let out: Vec<Outcome> = request
            .steps
            .iter()
            .map(|s| windows::Windows.apply(s, &cancel))
            .collect();
        std::fs::write(&request.result, serde_json::to_vec(&out)?)?;
        Ok(())
    }
    #[cfg(not(windows))]
    anyhow::bail!(
        "install scripts run on Windows only; {} steps not taken",
        request.steps.len()
    )
}

/// A step and whether it needs elevation (§4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned {
    pub step: Step,
    pub elevated: bool,
}

/// The job was stopped while steps ran.
#[derive(Debug, PartialEq, Eq)]
pub struct Stopped;

/// What steps act on: Windows, a Proton prefix, or a fake in tests
/// (FAITH-4).
pub trait System: Send + Sync {
    /// A value's type and data; `None` when it is missing.
    fn value(&self, v: &ValueRef) -> Option<Data>;
    /// A `REG_DWORD` value; `None` when it is missing or of another type.
    fn dword(&self, v: &ValueRef) -> Option<u32> {
        match self.value(v) {
            Some(Data::Dword(n)) => Some(n),
            _ => None,
        }
    }
    fn exists(&self, v: &ValueRef) -> bool {
        self.value(v).is_some()
    }
    /// Whether a firewall rule `name` for `program` is there; `None`
    /// where there is no firewall (§2.3; a prefix, `compat-tools.md` §7).
    fn firewall_rule(&self, name: &str, program: &str) -> Option<bool>;
    /// Whether steps can be elevated: on Windows, not in a prefix, where
    /// none needs it (§4).
    fn elevates(&self) -> bool {
        true
    }
    /// A host directory as the scripts' programs name it, for
    /// `%INSTALLDIR%` (§2.5).
    fn path(&self, dir: &Path) -> String {
        dir.to_string_lossy().into_owned()
    }
    /// Takes a step in the daemon's own user context. Returns `NotRun`
    /// when `cancel` stops it before it starts.
    fn apply(&self, step: &Step, cancel: &CancelToken) -> Outcome;
    /// Takes steps in the daemon's own user context, in order. A system
    /// that does several registry steps at once, as a prefix does, takes
    /// them together.
    fn apply_all(&self, steps: &[Step], cancel: &CancelToken) -> Vec<Outcome> {
        steps.iter().map(|s| self.apply(s, cancel)).collect()
    }
    /// Takes the steps in one elevated helper, behind one UAC prompt.
    /// `Err` when the helper did not run: the user declined, or it could
    /// not start.
    fn apply_elevated(&self, steps: &[Step], cancel: &CancelToken) -> Result<Vec<Outcome>, String>;
    fn is_64bit(&self) -> bool;
    /// The Windows version as `Requirement_OS` names it (§2.2).
    fn os_type(&self) -> String;
    /// A variable other than `INSTALLDIR` and `ROOTDRIVE` (§2.5).
    fn var(&self, name: &str) -> Option<String>;
}

/// A Windows version as `Requirement_OS` names it (§2.2), from its
/// major and minor version and build. *Unverified:* the names beyond
/// `WinXP` and `Win7`.
pub fn os_type(major: u32, minor: u32, build: u32) -> &'static str {
    match (major, minor) {
        (5, _) => "WinXP",
        (6, 0) => "WinVista",
        (6, 1) => "Win7",
        (6, 2) => "Win8",
        (6, 3) => "Win81",
        (10, _) if build >= 22_000 => "Win11",
        _ => "Win10",
    }
}

/// The system on this platform: Windows only (§3). A prefix's is made
/// for each launch ([`prefix::Prefix`]).
pub fn system() -> Option<std::sync::Arc<dyn System>> {
    #[cfg(windows)]
    return Some(std::sync::Arc::new(windows::Windows));
    #[cfg(not(windows))]
    None
}

/// An app's scripts, read.
#[derive(Debug, Default)]
pub struct Loaded {
    pub scripts: Vec<Script>,
    /// Scripts that are missing or do not match the installed manifest:
    /// the app needs a verify (§6).
    pub unusable: Vec<ScriptFile>,
}

/// Why a script was left out.
enum Unread {
    /// Missing, or not as the manifest says.
    Unusable(String),
    /// It does not parse.
    Broken(String),
}

/// Reads an app's scripts, in order of depot, then path (§3). `dir_of`
/// gives the directory a depot is installed in. A script that cannot be
/// read, does not match its manifest or does not parse is left out and
/// logged (§6). Variables are replaced (§2.5).
pub fn load(
    files: &[ScriptFile],
    dir_of: &dyn Fn(DepotId) -> Option<PathBuf>,
    language: &str,
    sys: &dyn System,
) -> Loaded {
    let mut files = files.to_vec();
    files.sort_by(|a, b| (a.depot, &a.path).cmp(&(b.depot, &b.path)));
    let mut out = Loaded::default();
    for f in files {
        let read = dir_of(f.depot)
            .ok_or_else(|| Unread::Unusable("the depot is not installed".to_owned()))
            .and_then(|dir| read(&dir, &f, language, sys));
        match read {
            Ok(script) => out.scripts.push(script),
            Err(Unread::Unusable(why)) => {
                tracing::warn!(depot = %f.depot, path = %f.path, "install script not used: {why}");
                out.unusable.push(f);
            }
            Err(Unread::Broken(why)) => {
                tracing::warn!(depot = %f.depot, path = %f.path, "install script not used: {why}")
            }
        }
    }
    out
}

fn read(dir: &Path, f: &ScriptFile, language: &str, sys: &dyn System) -> Result<Script, Unread> {
    let unusable = |e: &dyn fmt::Display| Unread::Unusable(e.to_string());
    let components =
        ossteam_content::path::components(&f.path, cfg!(windows)).map_err(|e| unusable(&e))?;
    let path = components.iter().fold(dir.to_owned(), |p, c| p.join(c));
    let bytes = std::fs::read(&path).map_err(|e| unusable(&e))?;
    if bytes.len() as u64 != f.size || Sha1::digest(&bytes).as_slice() != f.sha1 {
        return Err(unusable(
            &"the file does not match the installed manifest; the app needs a verify",
        ));
    }
    let script =
        Script::parse(&decode(&bytes), language).map_err(|e| Unread::Broken(e.to_string()))?;
    for p in &script.problems {
        tracing::info!(depot = %f.depot, path = %f.path, "install script: {p}");
    }
    let install_dir = sys.path(dir).trim_end_matches('\\').to_owned();
    let root_drive = install_dir
        .get(..2)
        .filter(|d| d.ends_with(':'))
        .map(str::to_owned);
    let vars = |name: &str| match name {
        "INSTALLDIR" => Some(install_dir.clone()),
        "ROOTDRIVE" => root_drive.clone(),
        other => sys.var(other),
    };
    let (script, unknown) = script.expand(&vars);
    if !unknown.is_empty() {
        tracing::info!(depot = %f.depot, path = %f.path, ?unknown, "install script: unknown variables left as they are");
    }
    Ok(script)
}

/// Script text: UTF-8, with or without a byte order mark, or UTF-16LE
/// with one (*unverified* which encodings ship).
fn decode(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xff, 0xfe]) {
        let units: Vec<u16> = rest
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&c| u16::from_le_bytes(c))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    String::from_utf8_lossy(bytes).into_owned()
}

fn has_run(p: &Process) -> Option<ValueRef> {
    p.has_run.as_ref().map(|k| ValueRef {
        key: k.into(),
        name: Some(p.name.clone()),
    })
}

/// A Run Process entry as a step, if its conditions hold and it is due
/// (§2.2).
fn run_step(p: &Process, sys: &dyn System) -> Option<Planned> {
    if !p.requirement.holds(sys.is_64bit(), &sys.os_type()) {
        return None;
    }
    let has_run = has_run(p);
    if !p.due(has_run.as_ref().and_then(|v| sys.dword(v))) {
        return None;
    }
    Some(Planned {
        step: Step::Run {
            program: p.program.clone(),
            arguments: p.arguments.clone(),
            has_run,
        },
        elevated: !p.as_current_user && sys.elevates(),
    })
}

/// What a start takes (§3.1): the registry values not already there
/// (`HKEY_LOCAL_MACHINE` only from signed scripts, §2.6), the firewall
/// rules not there yet, then the Run Process entries that are due.
pub fn start(loaded: &[Script], sys: &dyn System) -> Vec<Planned> {
    let mut plan = Vec::new();
    for s in loaded {
        for r in &s.registry {
            let value = ValueRef {
                key: (&r.key).into(),
                name: r.name.clone(),
            };
            if value.key.machine && !s.signed {
                tracing::info!("install script: not writing {value}: the script is not signed");
                continue;
            }
            let data = Data::from(&r.data);
            if sys.value(&value).as_ref() == Some(&data) {
                continue;
            }
            plan.push(Planned {
                elevated: value.key.machine && sys.elevates(),
                step: Step::Set { value, data },
            });
        }
    }
    for s in loaded {
        for (name, program) in &s.firewall {
            match sys.firewall_rule(name, program) {
                Some(true) => {}
                Some(false) => plan.push(Planned {
                    step: Step::Firewall {
                        name: name.clone(),
                        program: program.clone(),
                    },
                    elevated: sys.elevates(),
                }),
                None => tracing::info!(
                    "install script: no firewall here; the rule {name:?} for {program} is not added"
                ),
            }
        }
    }
    plan.extend(
        loaded
            .iter()
            .flat_map(|s| &s.run)
            .filter_map(|p| run_step(p, sys)),
    );
    plan
}

/// The values a start wrote: its `Set` steps that were done (§5).
pub fn written(plan: &[Planned], out: &[Outcome]) -> Vec<ValueRef> {
    plan.iter()
        .zip(out)
        .filter_map(|(p, o)| match (&p.step, o) {
            (Step::Set { value, .. }, Outcome::Done) => Some(value.clone()),
            _ => None,
        })
        .collect()
}

/// `written` added to `recorded`, each value once.
pub fn record(recorded: &mut Vec<ValueRef>, written: Vec<ValueRef>) {
    for v in written {
        if !recorded.contains(&v) {
            recorded.push(v);
        }
    }
}

/// What verify takes away so the scripts apply again at the next start
/// (§3): the values the scripts wrote (`written`), and the `HasRunKey`
/// values of Run Process entries without `NoCleanUp`. Values that are not
/// there are left out, so nothing asks for elevation in vain.
pub fn removal(loaded: &[Script], written: &[ValueRef], sys: &dyn System) -> Vec<Planned> {
    let mut values: Vec<ValueRef> = Vec::new();
    let records = loaded
        .iter()
        .flat_map(|s| &s.run)
        .filter(|p| !p.no_cleanup)
        .filter_map(has_run);
    for v in written.iter().cloned().chain(records) {
        if !values.contains(&v) && sys.exists(&v) {
            values.push(v);
        }
    }
    values
        .into_iter()
        .map(|v| Planned {
            elevated: v.key.machine && sys.elevates(),
            step: Step::Delete(v),
        })
        .collect()
}

/// The Run Process On Uninstall entries whose conditions hold and that
/// are due (§2.4). In a prefix they are all an uninstall takes, as the
/// prefix goes after them (`compat-tools.md` §7).
pub fn uninstall_entries(loaded: &[Script], sys: &dyn System) -> Vec<Planned> {
    loaded
        .iter()
        .flat_map(|s| &s.run_on_uninstall)
        .filter_map(|p| run_step(p, sys))
        .collect()
}

/// What uninstall does on Windows before the files go (§3): the Run
/// Process On Uninstall entries, then the removal.
pub fn uninstall(loaded: &[Script], written: &[ValueRef], sys: &dyn System) -> Vec<Planned> {
    let mut plan = uninstall_entries(loaded, sys);
    plan.extend(removal(loaded, written, sys));
    plan
}

/// What a group of steps does, in words, for `LaunchStep`
/// (`launch.md` §8).
pub fn describe(steps: &[Step], elevated: bool) -> String {
    let n = steps.len();
    let values = |verb: &str| match n {
        1 => format!("{verb} a registry value"),
        n => format!("{verb} {n} registry values"),
    };
    if elevated {
        return match n {
            1 => "Asking for administrator approval for one step".to_owned(),
            n => format!("Asking for administrator approval for {n} steps"),
        };
    }
    match steps {
        [Step::Run { program, .. }] => {
            let file = program.rsplit(['\\', '/']).next().unwrap_or(program);
            format!("Running {file}")
        }
        [Step::Firewall { name, .. }] => format!("Adding a firewall rule for {name}"),
        s if s.iter().all(|s| matches!(s, Step::Set { .. })) => values("Writing"),
        s if s.iter().all(|s| matches!(s, Step::Delete(_))) => values("Removing"),
        _ => values("Changing"),
    }
}

/// Whether a step changes the registry only, so that it can be taken
/// with its neighbours.
fn registry_only(s: &Step) -> bool {
    matches!(s, Step::Set { .. } | Step::Delete(_))
}

/// Takes the steps in order, except that every elevated step runs in one
/// helper at the place of the first (§4). Neighbouring registry steps
/// run together (`System::apply_all`). Before each group, `report` gets
/// what it does. A step that fails does not stop the others (§6); each
/// is logged.
pub fn execute(
    sys: &dyn System,
    plan: &[Planned],
    cancel: &CancelToken,
    report: &mut dyn FnMut(String),
) -> Result<Vec<Outcome>, Stopped> {
    let mut out = vec![Outcome::NotRun; plan.len()];
    let elevated: Vec<usize> = (0..plan.len()).filter(|&i| plan[i].elevated).collect();
    let mut i = 0;
    while i < plan.len() {
        if cancel.is_cancelled() {
            return Err(Stopped);
        }
        if !plan[i].elevated {
            let mut end = i + 1;
            if registry_only(&plan[i].step) {
                while end < plan.len() && !plan[end].elevated && registry_only(&plan[end].step) {
                    end += 1;
                }
            }
            let steps: Vec<Step> = plan[i..end].iter().map(|p| p.step.clone()).collect();
            report(describe(&steps, false));
            let results = sys.apply_all(&steps, cancel);
            for (j, r) in (i..end).zip(results) {
                out[j] = r;
            }
            i = end;
            continue;
        }
        if elevated.first() == Some(&i) {
            let steps: Vec<Step> = elevated.iter().map(|&j| plan[j].step.clone()).collect();
            report(describe(&steps, true));
            match sys.apply_elevated(&steps, cancel) {
                Ok(results) if results.len() == steps.len() => {
                    for (&j, r) in elevated.iter().zip(results) {
                        out[j] = r;
                    }
                }
                Ok(results) => {
                    let why = format!(
                        "the helper answered {} results for {} steps",
                        results.len(),
                        steps.len()
                    );
                    for &j in &elevated {
                        out[j] = Outcome::Failed(why.clone());
                    }
                }
                Err(why) => {
                    tracing::warn!("the elevated steps did not run: {why}");
                }
            }
        }
        i += 1;
    }
    if cancel.is_cancelled() {
        return Err(Stopped);
    }
    for (p, o) in plan.iter().zip(&out) {
        match o {
            Outcome::Done => tracing::info!(elevated = p.elevated, "install script: {}", p.step),
            o => tracing::warn!(elevated = p.elevated, outcome = ?o, "install script: {}", p.step),
        }
    }
    Ok(out)
}

/// The written values still recorded after a removal: those whose
/// deletion was planned and did not happen. One that was not planned was
/// not there any more.
pub fn remaining(written: &[ValueRef], plan: &[Planned], out: &[Outcome]) -> Vec<ValueRef> {
    written
        .iter()
        .filter(|w| {
            plan.iter()
                .zip(out)
                .any(|(p, o)| p.step == Step::Delete((*w).clone()) && *o != Outcome::Done)
        })
        .cloned()
        .collect()
}

#[cfg(test)]
pub mod fake {
    //! A registry, firewall and process table in memory (FAITH-4).

    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    pub struct State {
        pub values: HashMap<String, Data>,
        /// Firewall rules: name and program.
        pub rules: Vec<(String, String)>,
        /// Programs that ran, with their arguments and whether elevated.
        pub ran: Vec<(String, String, bool)>,
        /// Exit codes by program; 0 when not given.
        pub exit: HashMap<String, u32>,
        pub prompts: usize,
        pub decline: bool,
        /// Each call of `apply_all`, by the number of steps it got.
        pub batches: Vec<usize>,
    }

    pub struct FakeSystem {
        pub state: Mutex<State>,
        /// Whether steps are elevated: Windows, not a prefix.
        pub elevates: bool,
        /// Whether there is a firewall.
        pub firewall: bool,
    }

    impl Default for FakeSystem {
        fn default() -> Self {
            Self {
                state: Mutex::default(),
                elevates: true,
                firewall: true,
            }
        }
    }

    impl FakeSystem {
        /// Like a prefix: no elevation, no firewall.
        pub fn prefix() -> Self {
            Self {
                elevates: false,
                firewall: false,
                ..Self::default()
            }
        }

        pub fn set(&self, v: &ValueRef, n: u32) {
            self.set_data(v, Data::Dword(n));
        }

        pub fn set_data(&self, v: &ValueRef, d: Data) {
            self.state.lock().unwrap().values.insert(v.to_string(), d);
        }

        pub fn has(&self, v: &ValueRef) -> bool {
            self.state
                .lock()
                .unwrap()
                .values
                .contains_key(&v.to_string())
        }

        fn take(&self, step: &Step, elevated: bool) -> Outcome {
            let mut s = self.state.lock().unwrap();
            let denied = |machine: bool| machine && self.elevates && !elevated;
            match step {
                Step::Set { value, data } => {
                    if denied(value.key.machine) {
                        return Outcome::Failed("access denied".into());
                    }
                    s.values.insert(value.to_string(), data.clone());
                    Outcome::Done
                }
                Step::Delete(v) => {
                    if denied(v.key.machine) {
                        return Outcome::Failed("access denied".into());
                    }
                    s.values.remove(&v.to_string());
                    Outcome::Done
                }
                Step::Firewall { name, program } => {
                    if !elevated {
                        return Outcome::Failed("access denied".into());
                    }
                    s.rules.push((name.clone(), program.clone()));
                    Outcome::Done
                }
                Step::Run {
                    program,
                    arguments,
                    has_run,
                } => {
                    s.ran.push((program.clone(), arguments.clone(), elevated));
                    match s.exit.get(program).copied().unwrap_or(0) {
                        0 => {
                            if let Some(v) = has_run {
                                s.values.insert(v.to_string(), Data::Dword(1));
                            }
                            Outcome::Done
                        }
                        n => Outcome::Failed(format!("exit code {n}")),
                    }
                }
            }
        }
    }

    impl System for FakeSystem {
        fn value(&self, v: &ValueRef) -> Option<Data> {
            self.state
                .lock()
                .unwrap()
                .values
                .get(&v.to_string())
                .cloned()
        }

        fn firewall_rule(&self, name: &str, program: &str) -> Option<bool> {
            self.firewall.then(|| {
                self.state
                    .lock()
                    .unwrap()
                    .rules
                    .iter()
                    .any(|(n, p)| n == name && p == program)
            })
        }

        fn elevates(&self) -> bool {
            self.elevates
        }

        fn apply(&self, step: &Step, _: &CancelToken) -> Outcome {
            self.take(step, false)
        }

        fn apply_all(&self, steps: &[Step], _: &CancelToken) -> Vec<Outcome> {
            self.state.lock().unwrap().batches.push(steps.len());
            steps.iter().map(|s| self.take(s, false)).collect()
        }

        fn apply_elevated(&self, steps: &[Step], _: &CancelToken) -> Result<Vec<Outcome>, String> {
            {
                let mut s = self.state.lock().unwrap();
                s.prompts += 1;
                if s.decline {
                    return Err("the user declined".into());
                }
            }
            Ok(steps.iter().map(|st| self.take(st, true)).collect())
        }

        fn is_64bit(&self) -> bool {
            true
        }

        fn os_type(&self) -> String {
            "Win10".into()
        }

        fn var(&self, name: &str) -> Option<String> {
            (name == "WINDIR").then(|| "C:\\Windows".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::FakeSystem;
    use super::*;

    const SCRIPT: &str = r#"
"InstallScript"
{
    "Run Process"
    {
        "DirectX"
        {
            "HasRunKey" "HKEY_LOCAL_MACHINE\\Software\\Valve\\Steam\\Apps\\CommonRedist\\DirectX\\Jun2010"
            "process 1" "%INSTALLDIR%\\DXSETUP.exe"
            "command 1" "/silent"
            "NoCleanUp" "1"
        }
        "Prereq"
        {
            "HasRunKey" "HKEY_CURRENT_USER\\Software\\Example"
            "process 1" "%INSTALLDIR%\\prereq.exe"
            "AsCurrentUser" "1"
        }
    }
    "Run Process On Uninstall"
    {
        "Cleanup" { "process 1" "%INSTALLDIR%\\cleanup.exe" "command 1" "%WinDir%" "AsCurrentUser" "1" }
        "Unregister" { "process 1" "%INSTALLDIR%\\unreg.exe" }
        "Old" { "process 1" "%INSTALLDIR%\\old.exe" "Requirement_OS" { "Is64BitWindows" "0" } }
    }
}
"#;

    fn value(machine: bool, path: &str, name: &str) -> ValueRef {
        ValueRef {
            key: Key {
                machine,
                path: path.into(),
            },
            name: Some(name.into()),
        }
    }

    fn dx() -> ValueRef {
        value(
            true,
            "Software\\Valve\\Steam\\Apps\\CommonRedist\\DirectX\\Jun2010",
            "DirectX",
        )
    }

    fn prereq() -> ValueRef {
        value(false, "Software\\Example", "Prereq")
    }

    struct Dir(PathBuf);

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// The script installed as `scripts\install.vdf` of depot 5.
    fn installed(text: &[u8]) -> (Dir, ScriptFile) {
        let dir = std::env::temp_dir().join(format!(
            "ossteamd-script-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(dir.join("scripts")).unwrap();
        std::fs::write(dir.join("scripts").join("install.vdf"), text).unwrap();
        let file = ScriptFile {
            depot: DepotId(5),
            path: "scripts\\install.vdf".into(),
            size: text.len() as u64,
            sha1: Sha1::digest(text).to_vec(),
        };
        (Dir(dir), file)
    }

    fn load_one(dir: &Dir, file: &ScriptFile, sys: &FakeSystem) -> Vec<Script> {
        let d = dir.0.clone();
        load(
            std::slice::from_ref(file),
            &move |depot| (depot == DepotId(5)).then(|| d.clone()),
            "english",
            sys,
        )
        .scripts
    }

    #[test]
    fn scripts_are_checked_against_the_manifest() {
        let sys = FakeSystem::default();
        let (dir, file) = installed(SCRIPT.as_bytes());
        let loaded = load_one(&dir, &file, &sys);
        assert_eq!(loaded.len(), 1);
        let program = &loaded[0].run[0].program;
        assert_eq!(
            program,
            &format!("{}\\DXSETUP.exe", dir.0.to_string_lossy())
        );
        let cleanup = &loaded[0].run_on_uninstall[0];
        assert_eq!(cleanup.arguments, "C:\\Windows");

        let mut changed = file.clone();
        changed.sha1[0] ^= 1;
        assert!(load_one(&dir, &changed, &sys).is_empty());
        let mut other = file.clone();
        other.depot = DepotId(6);
        let d = dir.0.clone();
        let none = load(
            &[other.clone()],
            &move |depot| (depot == DepotId(5)).then(|| d.clone()),
            "english",
            &sys,
        );
        assert!(none.scripts.is_empty());
        assert_eq!(none.unusable, [other]);
        let d = dir.0.clone();
        let changed = load(
            &[changed],
            &move |depot| (depot == DepotId(5)).then(|| d.clone()),
            "english",
            &sys,
        );
        assert_eq!(changed.unusable.len(), 1);
        // One that does not parse is left out, but is no reason to verify.
        let (dir, broken) = installed(b"\"InstallScript\" {");
        let d = dir.0.clone();
        let broken = load(
            &[broken],
            &move |depot| (depot == DepotId(5)).then(|| d.clone()),
            "english",
            &sys,
        );
        assert!(broken.scripts.is_empty() && broken.unusable.is_empty());
    }

    #[test]
    fn byte_order_marks() {
        assert_eq!(decode(b"\xef\xbb\xbf\"a\" {}"), "\"a\" {}");
        let utf16: Vec<u8> = [0xfeff_u16]
            .into_iter()
            .chain("\"a\"".encode_utf16())
            .flat_map(u16::to_le_bytes)
            .collect();
        assert_eq!(decode(&utf16), "\"a\"");
    }

    #[test]
    fn verify_removes_written_values_and_run_records() {
        let sys = FakeSystem::default();
        let (dir, file) = installed(SCRIPT.as_bytes());
        let loaded = load_one(&dir, &file, &sys);
        let written = value(false, "Software\\Example\\Game", "InstallPath");
        let gone = value(true, "Software\\Example\\Game", "Language");
        sys.set(&written, 0);
        sys.set(&dx(), 1);
        sys.set(&prereq(), 1);

        let plan = removal(&loaded, &[written.clone(), gone.clone()], &sys);
        // DirectX has NoCleanUp; the value no longer there is left out.
        assert_eq!(
            plan,
            [
                Planned {
                    step: Step::Delete(written.clone()),
                    elevated: false
                },
                Planned {
                    step: Step::Delete(prereq()),
                    elevated: false
                },
            ]
        );
        let out = execute(&sys, &plan, &CancelToken::new(), &mut |_| {}).unwrap();
        assert_eq!(out, [Outcome::Done, Outcome::Done]);
        assert!(!sys.has(&written) && !sys.has(&prereq()) && sys.has(&dx()));
        assert_eq!(sys.state.lock().unwrap().prompts, 0);
        assert!(remaining(&[written, gone], &plan, &out).is_empty());
    }

    #[test]
    fn uninstall_runs_entries_then_removes() {
        let sys = FakeSystem::default();
        let (dir, file) = installed(SCRIPT.as_bytes());
        let loaded = load_one(&dir, &file, &sys);
        let machine = value(true, "Software\\Example\\Game", "Installed");
        sys.set(&machine, 1);

        let plan = uninstall(&loaded, std::slice::from_ref(&machine), &sys);
        // "Old" is for 32-bit Windows only.
        let elevated: Vec<bool> = plan.iter().map(|p| p.elevated).collect();
        assert_eq!(elevated, [false, true, true]);
        let out = execute(&sys, &plan, &CancelToken::new(), &mut |_| {}).unwrap();
        assert_eq!(out, [Outcome::Done, Outcome::Done, Outcome::Done]);
        let s = sys.state.lock().unwrap();
        assert_eq!(s.prompts, 1, "one prompt for every elevated step");
        let ran: Vec<(&str, bool)> = s
            .ran
            .iter()
            .map(|(p, _, e)| (p.rsplit('\\').next().unwrap(), *e))
            .collect();
        assert_eq!(ran, [("cleanup.exe", false), ("unreg.exe", true)]);
        assert!(!s.values.contains_key(&machine.to_string()));
    }

    #[test]
    fn declined_elevation_leaves_the_values_recorded() {
        let sys = FakeSystem::default();
        sys.state.lock().unwrap().decline = true;
        let machine = value(true, "Software\\Example\\Game", "Installed");
        let user = value(false, "Software\\Example\\Game", "Path");
        sys.set(&machine, 1);
        sys.set(&user, 1);
        let written = [machine.clone(), user.clone()];
        let plan = removal(&[], &written, &sys);
        let out = execute(&sys, &plan, &CancelToken::new(), &mut |_| {}).unwrap();
        assert_eq!(out, [Outcome::NotRun, Outcome::Done]);
        assert_eq!(remaining(&written, &plan, &out), [machine]);
    }

    #[test]
    fn failed_runs_and_due_entries() {
        let sys = FakeSystem::default();
        let (dir, file) = installed(SCRIPT.as_bytes());
        let loaded = load_one(&dir, &file, &sys);
        let unreg = loaded[0].run_on_uninstall[1].program.clone();
        sys.state.lock().unwrap().exit.insert(unreg, 3);
        let plan = uninstall(&loaded, &[], &sys);
        let out = execute(&sys, &plan, &CancelToken::new(), &mut |_| {}).unwrap();
        assert_eq!(out, [Outcome::Done, Outcome::Failed("exit code 3".into())]);

        // A Run Process entry already recorded as run is not due.
        sys.set(&dx(), 1);
        assert!(run_step(&loaded[0].run[0], &sys).is_none());
        assert!(run_step(&loaded[0].run[1], &sys).is_some());
    }

    #[test]
    fn helper_requests_round_trip() {
        let r = HelperRequest {
            steps: vec![
                Step::Delete(dx()),
                Step::Run {
                    program: "C:\\Games\\A b\\x.exe".into(),
                    arguments: "/q \"a\"".into(),
                    has_run: Some(prereq()),
                },
            ],
            result: PathBuf::from("C:\\Temp\\out.json"),
        };
        let s = r.encode();
        assert!(s.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(HelperRequest::decode(&s).unwrap(), r);
        assert!(HelperRequest::decode("abc").is_err());
        assert!(HelperRequest::decode("zz").is_err());
    }

    /// Spacewar's script (depot 481), with a firewall entry and an entry
    /// that runs as the user added, and an unsigned copy.
    const START: &str = r#"
"InstallScript"
{
    "Registry"
    {
        "HKEY_LOCAL_MACHINE\\SOFTWARE\\Valve\\Spacewar"
        {
            "string" { "Exe_Path" "%INSTALLDIR%\\SteamworksExample.exe" }
            "dword" { "PatchVersion" "42" }
        }
        "HKEY_CURRENT_USER\\Software\\Valve\\Spacewar"
        {
            "dword" { "Volume" "7" }
        }
    }
    "Firewall" { "Spacewar" "%INSTALLDIR%\\SteamworksExample.exe" }
    "Run Process"
    {
        "Prereq"
        {
            "HasRunKey" "HKEY_CURRENT_USER\\Software\\Example"
            "process 1" "%INSTALLDIR%\\prereq.exe"
            "AsCurrentUser" "1"
        }
    }
}
"#;

    fn spacewar(signed: bool) -> String {
        let sig = if signed {
            "\"kvsignatures\" { \"InstallScript\" \"73e0\" }"
        } else {
            ""
        };
        format!("{START}{sig}")
    }

    fn exe_path() -> ValueRef {
        value(true, "SOFTWARE\\Valve\\Spacewar", "Exe_Path")
    }

    fn patch_version() -> ValueRef {
        value(true, "SOFTWARE\\Valve\\Spacewar", "PatchVersion")
    }

    fn volume() -> ValueRef {
        value(false, "Software\\Valve\\Spacewar", "Volume")
    }

    #[test]
    fn a_start_writes_values_adds_rules_and_runs_entries() {
        let sys = FakeSystem::default();
        let (dir, file) = installed(spacewar(true).as_bytes());
        let loaded = load_one(&dir, &file, &sys);
        let exe = format!("{}\\SteamworksExample.exe", dir.0.to_string_lossy());
        // Already as the script has it: not planned.
        sys.set(&patch_version(), 42);

        let plan = start(&loaded, &sys);
        let steps: Vec<(&Step, bool)> = plan.iter().map(|p| (&p.step, p.elevated)).collect();
        let prereq_step = Step::Run {
            program: format!("{}\\prereq.exe", dir.0.to_string_lossy()),
            arguments: String::new(),
            has_run: Some(prereq()),
        };
        assert_eq!(
            steps,
            [
                (
                    &Step::Set {
                        value: exe_path(),
                        data: Data::String(exe.clone())
                    },
                    true
                ),
                (
                    &Step::Set {
                        value: volume(),
                        data: Data::Dword(7)
                    },
                    false
                ),
                (
                    &Step::Firewall {
                        name: "Spacewar".into(),
                        program: exe.clone()
                    },
                    true
                ),
                (&prereq_step, false),
            ]
        );
        let mut reported = Vec::new();
        let out = execute(&sys, &plan, &CancelToken::new(), &mut |t| reported.push(t)).unwrap();
        assert!(out.iter().all(|o| *o == Outcome::Done), "{out:?}");
        assert_eq!(
            reported,
            [
                "Asking for administrator approval for 2 steps",
                "Writing a registry value",
                "Running prereq.exe",
            ]
        );
        assert_eq!(written(&plan, &out), [exe_path(), volume()]);
        let mut recorded = vec![volume()];
        record(&mut recorded, written(&plan, &out));
        assert_eq!(recorded, [volume(), exe_path()]);
        assert_eq!(sys.state.lock().unwrap().prompts, 1);

        // Everything is in place now: the next start plans nothing.
        assert!(start(&loaded, &sys).is_empty());
    }

    #[test]
    fn unsigned_scripts_write_no_local_machine_values() {
        let sys = FakeSystem::default();
        let (dir, file) = installed(spacewar(false).as_bytes());
        let loaded = load_one(&dir, &file, &sys);
        let plan = start(&loaded, &sys);
        let sets: Vec<&ValueRef> = plan
            .iter()
            .filter_map(|p| match &p.step {
                Step::Set { value, .. } => Some(value),
                _ => None,
            })
            .collect();
        assert_eq!(sets, [&volume()]);
    }

    #[test]
    fn a_prefix_takes_registry_steps_together() {
        let sys = FakeSystem::prefix();
        let (dir, file) = installed(spacewar(true).as_bytes());
        let loaded = load_one(&dir, &file, &sys);
        let plan = start(&loaded, &sys);
        // No firewall, and nothing elevated.
        assert_eq!(plan.len(), 4);
        assert!(plan.iter().all(|p| !p.elevated));
        let mut reported = Vec::new();
        let out = execute(&sys, &plan, &CancelToken::new(), &mut |t| reported.push(t)).unwrap();
        assert!(out.iter().all(|o| *o == Outcome::Done), "{out:?}");
        assert_eq!(
            reported,
            ["Writing 3 registry values", "Running prereq.exe"]
        );
        let s = sys.state.lock().unwrap();
        assert_eq!((s.prompts, s.batches.as_slice()), (0, [3, 1].as_slice()));
    }

    #[test]
    fn a_prefix_uninstall_runs_entries_only() {
        let sys = FakeSystem::prefix();
        let (dir, file) = installed(SCRIPT.as_bytes());
        let loaded = load_one(&dir, &file, &sys);
        sys.set(&prereq(), 1);
        let plan = uninstall_entries(&loaded, &sys);
        // "Old" is for 32-bit Windows only; nothing is deleted.
        let programs: Vec<&str> = plan
            .iter()
            .map(|p| match &p.step {
                Step::Run { program, .. } => program.rsplit('\\').next().unwrap(),
                other => panic!("{other}"),
            })
            .collect();
        assert_eq!(programs, ["cleanup.exe", "unreg.exe"]);
        assert!(plan.iter().all(|p| !p.elevated));
    }

    #[test]
    fn a_stop_ends_the_steps() {
        let sys = FakeSystem::default();
        let cancel = CancelToken::new();
        cancel.cancel();
        let plan = removal(&[], &[], &sys);
        assert_eq!(execute(&sys, &plan, &cancel, &mut |_| {}), Err(Stopped));
    }
}
