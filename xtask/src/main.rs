// SPDX-License-Identifier: GPL-2.0-or-later
//! Repository tooling, run as `cargo xtask <command>`.
//!
//! - `check-layering`: enforces the crate dependency rules (SPEC WS-1 to
//!   WS-3, LEAF-2).
//! - `check-spdx`: enforces license headers and crate licenses (LIC-1).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, bail};
use cargo_metadata::{DependencyKind, MetadataCommand};

const SPDX_LINE: &str = "// SPDX-License-Identifier: GPL-2.0-or-later";
const LICENSE: &str = "GPL-2.0-or-later";

/// Which workspace crates each workspace crate may depend on directly
/// (normal and build dependencies). Every allowed edge points down a layer,
/// so checking direct edges also bounds the transitive closure. A crate
/// missing from this table fails the check, so new crates get placed
/// deliberately.
///
/// `None` means any workspace crate except the leaf clients and the shim.
const ALLOWED: &[(&str, Option<&[&str]>)] = &[
    ("ossteam-types", Some(&[])),
    ("ossteam-vdf", Some(&["ossteam-types"])),
    ("ossteam-proto", Some(&["ossteam-types"])),
    (
        "ossteam-net",
        Some(&["ossteam-types", "ossteam-vdf", "ossteam-proto"]),
    ),
    (
        "ossteam-webapi",
        Some(&["ossteam-types", "ossteam-vdf", "ossteam-proto"]),
    ),
    ("ossteam-auth", Some(STEAM_SERVICES)),
    ("ossteam-pics", Some(STEAM_SERVICES)),
    ("ossteam-content", Some(STEAM_SERVICES)),
    ("ossteam-stats", Some(STEAM_SERVICES)),
    ("ossteam-launch", Some(STEAM_SERVICES)),
    ("ossteam-gameipc", Some(&["ossteam-types"])),
    ("ossteam-ipc", Some(&["ossteam-types"])),
    // WS-2: the shim sees only the game IPC protocol and plain types.
    (
        "ossteam-steamclient",
        Some(&["ossteam-gameipc", "ossteam-types"]),
    ),
    ("ossteamd", None),
    // LEAF-2: leaf clients reach the daemon through ossteam-ipc only.
    ("ossteam-cli", Some(&["ossteam-ipc", "ossteam-types"])),
    ("ossteam-gui", Some(&["ossteam-ipc", "ossteam-types"])),
    ("xtask", Some(&[])),
];

const STEAM_SERVICES: &[&str] = &[
    "ossteam-types",
    "ossteam-vdf",
    "ossteam-proto",
    "ossteam-net",
    "ossteam-webapi",
];

const NEVER_DEPENDED_ON: &[&str] = &["ossteam-cli", "ossteam-gui", "ossteam-steamclient", "xtask"];

/// Directories whose files are vendored verbatim and keep their upstream
/// notices (LIC-6).
const VENDORED: &[&str] = &["crates/ossteam-proto/protos"];

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let result = match args.next().as_deref() {
        Some("check-layering") => check_layering(),
        Some("check-spdx") => check_spdx(),
        Some(other) => Err(anyhow::anyhow!("unknown command {other:?}")),
        None => Err(anyhow::anyhow!(
            "usage: cargo xtask <check-layering|check-spdx>"
        )),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives in the workspace")
        .to_owned()
}

fn check_layering() -> anyhow::Result<()> {
    let meta = MetadataCommand::new()
        .manifest_path(workspace_root().join("Cargo.toml"))
        .no_deps()
        .exec()
        .context("running cargo metadata")?;
    let members: BTreeMap<&str, _> = meta
        .workspace_packages()
        .into_iter()
        .map(|p| (p.name.as_str(), p))
        .collect();
    let allowed: BTreeMap<&str, Option<&[&str]>> = ALLOWED.iter().copied().collect();

    let mut problems = Vec::new();
    for (name, pkg) in &members {
        let Some(rule) = allowed.get(name) else {
            problems.push(format!(
                "{name} is not in xtask's layering table; add it to ALLOWED"
            ));
            continue;
        };
        for dep in &pkg.dependencies {
            if dep.kind == DependencyKind::Development || !members.contains_key(dep.name.as_str()) {
                continue;
            }
            let ok = match rule {
                Some(list) => list.contains(&dep.name.as_str()),
                None => !NEVER_DEPENDED_ON.contains(&dep.name.as_str()),
            };
            if !ok {
                problems.push(format!("{name} must not depend on {}", dep.name));
            }
        }
    }
    report("layering", problems)
}

fn check_spdx() -> anyhow::Result<()> {
    let root = workspace_root();
    let mut problems = Vec::new();

    let meta = MetadataCommand::new()
        .manifest_path(root.join("Cargo.toml"))
        .no_deps()
        .exec()
        .context("running cargo metadata")?;
    for pkg in meta.workspace_packages() {
        if pkg.license.as_deref() != Some(LICENSE) {
            problems.push(format!(
                "{}: license must be {LICENSE}, is {:?}",
                pkg.name, pkg.license
            ));
        }
    }

    if !root.join("COPYING").is_file() {
        problems.push("COPYING (the GPLv2 text) is missing".into());
    }

    let mut files = Vec::new();
    for dir in ["crates", "xtask", "fuzz"] {
        collect_sources(&root, &root.join(dir), &mut files)?;
    }
    for path in files {
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        if text.lines().next() != Some(SPDX_LINE) {
            let rel = path.strip_prefix(&root).unwrap_or(&path);
            problems.push(format!(
                "{}: first line must be `{SPDX_LINE}`",
                rel.display()
            ));
        }
    }
    report("SPDX", problems)
}

fn collect_sources(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("listing {}", dir.display()))? {
        let path = entry?.path();
        let rel = path.strip_prefix(root).unwrap_or(&path);
        if VENDORED.iter().any(|v| rel.starts_with(v)) || rel.ends_with("target") {
            continue;
        }
        if path.is_dir() {
            collect_sources(root, &path, out)?;
        } else if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("rs" | "proto")
        ) {
            out.push(path);
        }
    }
    Ok(())
}

fn report(what: &str, problems: Vec<String>) -> anyhow::Result<()> {
    if problems.is_empty() {
        println!("{what} check passed");
        return Ok(());
    }
    for p in &problems {
        eprintln!("  {p}");
    }
    bail!("{what} check found {} problem(s)", problems.len())
}
