// SPDX-License-Identifier: GPL-2.0-or-later
//! `cargo xtask shaders [--check]`: compiles `crates/ossteam-ui/shaders/*.slang`
//! to SPIR-V with `slangc` (ADR 0004). Each file holds a pipeline's
//! `vertexMain` and `fragmentMain`. The SPIR-V is committed, so building
//! ossteam needs no shader compiler; only changing a shader does.
//!
//! The output depends on the compiler's version, which is kept in
//! `slangc-version.txt`. `--check` fails when the SPIR-V does not match
//! its source, and refuses to judge with a different `slangc`.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, bail};

/// Vulkan 1.2, ossteam-ui's baseline, takes SPIR-V 1.5.
const PROFILE: &str = "spirv_1_5";
const VERSION_FILE: &str = "slangc-version.txt";

pub fn run(args: Vec<String>) -> anyhow::Result<()> {
    let check = match args.as_slice() {
        [] => false,
        [flag] if flag == "--check" => true,
        _ => bail!("usage: cargo xtask shaders [--check]"),
    };
    let dir = crate::workspace_root().join("crates/ossteam-ui/shaders");
    let version = slangc_version()?;
    let pinned = std::fs::read_to_string(dir.join(VERSION_FILE)).unwrap_or_default();
    if check && pinned.trim() != version {
        bail!(
            "the SPIR-V was made with slangc {}, but slangc here is {version}; \
             compare with that version, or run `cargo xtask shaders` to move to this one",
            pinned.trim()
        );
    }
    let mut sources: Vec<PathBuf> = std::fs::read_dir(&dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "slang"))
        .collect();
    sources.sort();
    let scratch = std::env::temp_dir().join(format!("ossteam-shaders-{}", std::process::id()));
    std::fs::create_dir_all(&scratch)?;
    let mut stale = Vec::new();
    for source in &sources {
        let out = source.with_extension("spv");
        let target = if check {
            scratch.join(out.file_name().expect("a file name"))
        } else {
            out.clone()
        };
        compile(source, &target)?;
        if check {
            if std::fs::read(&out).ok() != Some(std::fs::read(&target)?) {
                stale.push(out.display().to_string());
            }
        } else {
            println!("{}", out.display());
        }
    }
    let _ = std::fs::remove_dir_all(&scratch);
    if !check {
        std::fs::write(dir.join(VERSION_FILE), format!("{version}\n"))?;
    }
    if !stale.is_empty() {
        bail!(
            "SPIR-V does not match its source; run `cargo xtask shaders`: {}",
            stale.join(", ")
        );
    }
    Ok(())
}

fn slangc_version() -> anyhow::Result<String> {
    let output = Command::new("slangc")
        .arg("-v")
        .output()
        .context("running slangc (the Slang shader compiler, needed only to change shaders)")?;
    // slangc prints its version on stderr.
    let text = String::from_utf8_lossy(if output.stdout.is_empty() {
        &output.stderr
    } else {
        &output.stdout
    })
    .trim()
    .to_owned();
    if !output.status.success() || text.is_empty() {
        bail!("slangc -v failed: {text}");
    }
    Ok(text)
}

fn compile(source: &Path, out: &Path) -> anyhow::Result<()> {
    let status = Command::new("slangc")
        .arg(source)
        .args(["-target", "spirv", "-profile", PROFILE])
        // Keep `vertexMain` and `fragmentMain` apart in one module.
        .arg("-fvk-use-entrypoint-name")
        .arg("-o")
        .arg(out)
        .status()
        .context("running slangc")?;
    if !status.success() {
        bail!("slangc failed on {}", source.display());
    }
    Ok(())
}
