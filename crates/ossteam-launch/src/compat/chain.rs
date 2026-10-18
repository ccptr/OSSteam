// SPDX-License-Identifier: GPL-2.0-or-later
//! The chain of tools, its command line and its environment
//! (`compat-tools.md` §4 and §5).

use std::path::{Path, PathBuf};

use ossteam_types::AppId;

use super::ToolManifest;
use crate::command::shell_quote;

/// What `%verb%` becomes (§5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    /// The game.
    WaitForExitAndRun,
    /// A setup command (§7).
    Run,
}

impl Verb {
    pub fn as_str(self) -> &'static str {
        match self {
            Verb::WaitForExitAndRun => "waitforexitandrun",
            Verb::Run => "run",
        }
    }
}

/// An installed tool of a chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainTool {
    pub app: AppId,
    /// Its install directory.
    pub dir: PathBuf,
    pub manifest: ToolManifest,
}

/// How far [`walk`] got.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Walk {
    /// The tools found installed, innermost (the app's own tool) first.
    pub installed: Vec<AppId>,
    /// The first tool that is not installed, whose manifest, and so the
    /// rest of the chain, is not known yet (§4).
    pub missing: Option<AppId>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("the compatibility tool {0} requires itself, directly or through another tool")]
pub struct Cycle(pub AppId);

/// Follows `require_tool_appid` outwards from `first`. `requires` gives
/// an installed tool's requirement as its manifest states it, or `None`
/// when the tool is not installed.
pub fn walk(
    first: AppId,
    mut requires: impl FnMut(AppId) -> Option<Option<AppId>>,
) -> Result<Walk, Cycle> {
    let mut installed = Vec::new();
    let mut next = Some(first);
    while let Some(app) = next {
        if installed.contains(&app) {
            return Err(Cycle(app));
        }
        match requires(app) {
            None => {
                return Ok(Walk {
                    installed,
                    missing: Some(app),
                });
            }
            Some(r) => {
                installed.push(app);
                next = r;
            }
        }
    }
    Ok(Walk {
        installed,
        missing: None,
    })
}

/// The tools' part of the command line, outermost first (§5.1). Each
/// tool's `commandline` has its first word, when it starts with `/`,
/// made absolute under the quoted install directory, and `%verb%`
/// replaced in version 2 manifests. The game's command line follows,
/// after a space.
pub fn command_prefix(outermost_first: &[ChainTool], verb: Verb) -> String {
    let mut parts = Vec::with_capacity(outermost_first.len());
    for tool in outermost_first {
        let mut line = tool.manifest.commandline.trim().to_owned();
        if tool.manifest.version >= 2 {
            line = line.replace("%verb%", verb.as_str());
        }
        let (first, rest) = line.split_once([' ', '\t']).unwrap_or((&line, ""));
        let first = if first.starts_with('/') {
            format!("{}{first}", shell_quote(&tool.dir.to_string_lossy()))
        } else {
            first.to_owned()
        };
        let rest = rest.trim();
        parts.push(if rest.is_empty() {
            first
        } else {
            format!("{first} {rest}")
        });
    }
    parts.join(" ")
}

/// The `STEAM_COMPAT_*` variables of a launch through a chain (§5.2).
#[derive(Debug, Clone)]
pub struct CompatEnv<'a> {
    pub app: AppId,
    pub install_dir: &'a Path,
    /// The prefix directory (§6.1).
    pub data_dir: &'a Path,
    /// The generated client-install directory (§6.2).
    pub client_dir: &'a Path,
    /// The library folders that hold the app and the tools.
    pub library_paths: Vec<PathBuf>,
    /// The tools' install directories, innermost first.
    pub tool_paths: Vec<PathBuf>,
    /// More directories the container must share.
    pub mounts: Vec<PathBuf>,
    /// `app_compat_configs/<n>/config`, when the app has one.
    pub config: Option<&'a str>,
}

impl CompatEnv<'_> {
    /// The variables in a fixed order. Lists are `:`-separated, without
    /// duplicates; an empty list is not set.
    pub fn vars(&self) -> Vec<(&'static str, String)> {
        let path = |p: &Path| p.to_string_lossy().into_owned();
        let mut v = vec![
            ("STEAM_COMPAT_APP_ID", self.app.0.to_string()),
            ("STEAM_COMPAT_INSTALL_PATH", path(self.install_dir)),
            ("STEAM_COMPAT_DATA_PATH", path(self.data_dir)),
            ("STEAM_COMPAT_CLIENT_INSTALL_PATH", path(self.client_dir)),
        ];
        for (name, list) in [
            ("STEAM_COMPAT_LIBRARY_PATHS", &self.library_paths),
            ("STEAM_COMPAT_TOOL_PATHS", &self.tool_paths),
            ("STEAM_COMPAT_MOUNTS", &self.mounts),
        ] {
            if let Some(joined) = join(list) {
                v.push((name, joined));
            }
        }
        if let Some(c) = self.config.filter(|c| !c.trim().is_empty()) {
            v.push(("STEAM_COMPAT_CONFIG", c.trim().to_owned()));
        }
        v
    }
}

fn join(paths: &[PathBuf]) -> Option<String> {
    let mut seen: Vec<&Path> = Vec::new();
    for p in paths {
        if !seen.contains(&p.as_path()) {
            seen.push(p);
        }
    }
    (!seen.is_empty()).then(|| {
        seen.iter()
            .map(|p| p.to_string_lossy())
            .collect::<Vec<_>>()
            .join(":")
    })
}

/// `NAME=VALUE` from `app_compat_configs/<n>/environment` (§5.3), with
/// `$NAME` and `${NAME}` in the value replaced by `lookup`'s answer (empty
/// when unset). `None` when it is not an assignment.
pub fn assignment(text: &str, lookup: impl Fn(&str) -> Option<String>) -> Option<(String, String)> {
    let (name, value) = text.trim().split_once('=')?;
    let valid = |c: char| c.is_ascii_alphanumeric() || c == '_';
    if name.is_empty() || !name.chars().all(valid) || name.starts_with(|c: char| c.is_ascii_digit())
    {
        return None;
    }
    let mut out = String::new();
    let mut rest = value;
    while let Some(i) = rest.find('$') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let (var, tail) = if let Some(braced) = after.strip_prefix('{') {
            match braced.split_once('}') {
                Some((var, tail)) => (var, tail),
                None => ("", after),
            }
        } else {
            let end = after.find(|c: char| !valid(c)).unwrap_or(after.len());
            after.split_at(end)
        };
        if var.is_empty() {
            out.push('$');
        } else {
            out.push_str(&lookup(var).unwrap_or_default());
        }
        rest = tail;
    }
    out.push_str(rest);
    Some((name.to_owned(), out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(app: u32, dir: &str, manifest: &str) -> ChainTool {
        ChainTool {
            app: AppId(app),
            dir: dir.into(),
            manifest: ToolManifest::parse(manifest, "t").unwrap(),
        }
    }

    fn sniper() -> ChainTool {
        tool(
            1628350,
            "/lib/common/SteamLinuxRuntime_sniper",
            r#""manifest" { "version" "2" "commandline" "/_v2-entry-point --verb=%verb% --" }"#,
        )
    }

    #[test]
    fn walks_outwards_until_a_tool_is_missing() {
        let requires = |a: AppId| match a.0 {
            1493710 => Some(Some(AppId(4183110))),
            4183110 => Some(None),
            _ => None,
        };
        assert_eq!(
            walk(AppId(1493710), requires).unwrap(),
            Walk {
                installed: vec![AppId(1493710), AppId(4183110)],
                missing: None
            }
        );
        // Scout requires soldier, which is not installed.
        let requires = |a: AppId| (a.0 == 1070560).then_some(Some(AppId(1391110)));
        assert_eq!(
            walk(AppId(1070560), requires).unwrap(),
            Walk {
                installed: vec![AppId(1070560)],
                missing: Some(AppId(1391110))
            }
        );
        let cycle = |a: AppId| Some(Some(AppId(if a.0 == 1 { 2 } else { 1 })));
        assert_eq!(walk(AppId(1), cycle), Err(Cycle(AppId(1))));
    }

    #[test]
    fn prefix_matches_the_official_clients_log() {
        // gameprocess_log.txt for Counter-Strike 2.
        assert_eq!(
            command_prefix(&[sniper()], Verb::WaitForExitAndRun),
            "'/lib/common/SteamLinuxRuntime_sniper'/_v2-entry-point --verb=waitforexitandrun --"
        );
        assert_eq!(
            command_prefix(&[sniper()], Verb::Run),
            "'/lib/common/SteamLinuxRuntime_sniper'/_v2-entry-point --verb=run --"
        );
    }

    #[test]
    fn prefix_stacks_tools_outermost_first() {
        let proton = tool(
            1493710,
            "/lib/common/Proton - Experimental",
            r#""manifest" { "version" "2" "commandline" "/proton %verb%" }"#,
        );
        assert_eq!(
            command_prefix(&[sniper(), proton], Verb::WaitForExitAndRun),
            "'/lib/common/SteamLinuxRuntime_sniper'/_v2-entry-point --verb=waitforexitandrun -- \
             '/lib/common/Proton - Experimental'/proton waitforexitandrun"
        );
        // Version 1 keeps %verb% as written.
        let v1 = tool(1, "/t", r#""manifest" { "commandline" "/run %verb%" }"#);
        assert_eq!(command_prefix(&[v1], Verb::Run), "'/t'/run %verb%");
    }

    #[test]
    fn environment() {
        let env = CompatEnv {
            app: AppId(730),
            install_dir: Path::new("/lib/common/Game"),
            data_dir: Path::new("/lib/compatdata/730"),
            client_dir: Path::new("/data/compat-client"),
            library_paths: vec!["/lib".into(), "/other".into(), "/lib".into()],
            tool_paths: vec!["/lib/common/Proton".into(), "/other/common/SLR".into()],
            mounts: vec![],
            config: Some(" noesync,nofsync "),
        };
        assert_eq!(
            env.vars(),
            [
                ("STEAM_COMPAT_APP_ID", "730".to_owned()),
                ("STEAM_COMPAT_INSTALL_PATH", "/lib/common/Game".into()),
                ("STEAM_COMPAT_DATA_PATH", "/lib/compatdata/730".into()),
                (
                    "STEAM_COMPAT_CLIENT_INSTALL_PATH",
                    "/data/compat-client".into()
                ),
                ("STEAM_COMPAT_LIBRARY_PATHS", "/lib:/other".into()),
                (
                    "STEAM_COMPAT_TOOL_PATHS",
                    "/lib/common/Proton:/other/common/SLR".into()
                ),
                ("STEAM_COMPAT_CONFIG", "noesync,nofsync".into()),
            ]
        );
    }

    #[test]
    fn assignments_expand_variables() {
        let lookup = |n: &str| (n == "LD_LIBRARY_PATH").then(|| "/a:/b".to_owned());
        assert_eq!(
            assignment("DXVK_HDR=0", lookup),
            Some(("DXVK_HDR".into(), "0".into()))
        );
        assert_eq!(
            assignment(
                "LD_LIBRARY_PATH=/usr/lib/steamos/mesa/lib:$LD_LIBRARY_PATH",
                lookup
            ),
            Some((
                "LD_LIBRARY_PATH".into(),
                "/usr/lib/steamos/mesa/lib:/a:/b".into()
            ))
        );
        assert_eq!(
            assignment("X=${LD_LIBRARY_PATH}x$UNSET$", lookup),
            Some(("X".into(), "/a:/bx$".into()))
        );
        assert_eq!(assignment("not an assignment", lookup), None);
        assert_eq!(assignment("1X=y", lookup), None);
    }
}
