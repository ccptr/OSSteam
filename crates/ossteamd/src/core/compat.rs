// SPDX-License-Identifier: GPL-2.0-or-later
//! The core's side of compatibility tools (`compat-tools.md`): which tool
//! an app gets, which platform its depots are for, and the checks of the
//! `compat.*` settings.

use std::path::PathBuf;

use ossteam_content::depots::Host;
use ossteam_ipc::RemoteError;
use ossteam_ipc::v1::ErrorCode;
use ossteam_launch::compat::{
    ChainTool, CompatEnv, MANIFESTS_APP, Mapping, MappingInput, ToolList, ToolManifest, chain,
};
use ossteam_types::{AppId, SteamId};
use ossteam_vdf::Object;

use super::{Core, invalid};
use crate::config;
use crate::content::Kind;
use crate::content::folders::Layout;
use crate::content::job;

fn unsupported(m: impl Into<String>) -> RemoteError {
    RemoteError::new(ErrorCode::Unsupported, m)
}

fn not_found(m: impl Into<String>) -> RemoteError {
    RemoteError::new(ErrorCode::NotFound, m)
}

/// Whether this build uses compatibility tools (PLAT-2).
pub const ENABLED: bool = cfg!(target_os = "linux");

impl Core {
    /// Valve's tool list, from app 891390's cached info (§1). `None` off
    /// Linux and while the info is not available.
    pub(super) fn tool_list(&self) -> Option<ToolList> {
        if !ENABLED {
            return None;
        }
        self.app_info(MANIFESTS_APP).map(|i| ToolList::parse(&i))
    }

    /// The mapping that gives `app` its tool (§2), or `None` when it runs
    /// natively.
    pub(super) fn compat_mapping<'l>(
        &self,
        list: &'l ToolList,
        app: AppId,
        info: Option<&Object>,
    ) -> Option<Mapping<'l>> {
        list.resolve(MappingInput {
            app,
            appinfo: info,
            user: self.config.app_tool(app),
            default_tool: self.config.default_tool(),
        })
    }

    /// The platform whose depots an install of `app` takes (§4): the one
    /// its tool runs programs of, or the host's.
    pub(super) fn install_host(&self, app: AppId, info: &Object) -> Host {
        let host = Host::this();
        let Some(list) = self.tool_list() else {
            return host;
        };
        match self
            .compat_mapping(&list, app, Some(info))
            .and_then(|m| m.tool.platform())
        {
            Some(os) => Host {
                os: platform_name(os).unwrap_or(host.os),
                ..host
            },
            None => host,
        }
    }

    /// `SetSetting` on `compat.default_tool` or `compat.tool.<app>`: a
    /// non-empty value must name a tool (§8).
    pub(super) fn check_tool_setting(&self, key: &str, name: &str) -> Result<(), RemoteError> {
        if name.is_empty() || (key != config::COMPAT_DEFAULT_TOOL && key != config::COMPAT_TOOL) {
            return Ok(());
        }
        if !ENABLED {
            return Err(invalid("compatibility tools exist only on Linux"));
        }
        let list = self.tool_list().ok_or_else(|| {
            invalid(format!(
                "the compatibility tool list (app {MANIFESTS_APP}) is not available yet"
            ))
        })?;
        if list.find(name).is_none() {
            let mut known: Vec<&str> = list
                .tools
                .iter()
                .filter(|t| !t.unlisted)
                .map(|t| t.name.as_str())
                .chain(list.selectable.iter().map(|s| s.alias.as_str()))
                .collect();
            known.sort_unstable();
            return Err(invalid(format!(
                "there is no compatibility tool {name:?}; listed tools: {}",
                known.join(", ")
            )));
        }
        Ok(())
    }
}

/// `os` as the depot rules write it.
fn platform_name(os: &str) -> Option<&'static str> {
    ["windows", "macos", "linux"]
        .into_iter()
        .find(|p| p.eq_ignore_ascii_case(os))
}

/// A launch's chain once every tool is installed (§4).
pub(super) struct Ready {
    /// Outermost first.
    pub tools: Vec<ChainTool>,
    /// The library folders that hold the tools.
    pub tool_folders: Vec<PathBuf>,
}

pub(super) enum Prepared {
    Ready(Ready),
    /// The launch waits for these jobs, which install or update tools.
    Waiting(Vec<u64>),
}

impl Core {
    /// The chain that starts at `first`, or the job that must run before
    /// it is complete (§4). A missing tool is installed into
    /// `folder_id`, the game's folder.
    pub(super) fn prepare_chain(
        &mut self,
        steam_id: SteamId,
        list: &ToolList,
        first: AppId,
        folder_id: &str,
    ) -> Result<Prepared, RemoteError> {
        let mut found: Vec<(ChainTool, PathBuf)> = Vec::new();
        let mut broken = None;
        let walk = chain::walk(first, |a| {
            let inst = self.content.installed.get(&a)?;
            if inst.update_available || self.unfinished_job(a).is_some() {
                return None;
            }
            let folder = self
                .content
                .folders
                .iter()
                .find(|f| f.id == inst.folder_id)?;
            let dir = Layout::new(&folder.path).app_dir(&inst.install_dir);
            match ToolManifest::read(&dir) {
                Ok(manifest) => {
                    let requires = manifest.requires;
                    found.push((
                        ChainTool {
                            app: a,
                            dir,
                            manifest,
                        },
                        folder.path.clone(),
                    ));
                    Some(requires)
                }
                Err(e) => {
                    broken = Some((a, e));
                    None
                }
            }
        })
        .map_err(|e| unsupported(e.to_string()))?;
        if let Some((a, e)) = broken {
            return Err(not_found(format!(
                "{e}; verify app {a}, the compatibility tool {}",
                tool_name(list, a)
            )));
        }
        let Some(missing) = walk.missing else {
            let (mut tools, tool_folders): (Vec<_>, Vec<_>) = found.into_iter().unzip();
            tools.reverse();
            return Ok(Prepared::Ready(Ready {
                tools,
                tool_folders,
            }));
        };
        if let Some(j) = self.unfinished_job(missing) {
            return Ok(Prepared::Waiting(vec![j]));
        }
        let name = tool_name(list, missing);
        let job = if let Some(inst) = self.content.installed.get(&missing).cloned() {
            tracing::info!(tool = %name, "updating a compatibility tool for a launch");
            self.create_job(steam_id, missing, Kind::Update, inst.folder_id, inst.branch)?
        } else {
            let owned = self.snapshot()?.library.find(missing).is_some();
            if !owned {
                return Err(RemoteError::new(
                    ErrorCode::NotOwned,
                    format!(
                        "the compatibility tool {name} (app {missing}) is not in the account's library"
                    ),
                ));
            }
            let info = self.app_info(missing).ok_or_else(|| {
                not_found(format!(
                    "the info of the compatibility tool {name} (app {missing}) is not available"
                ))
            })?;
            job::install_dir(&info).map_err(unsupported)?;
            tracing::info!(tool = %name, "installing a compatibility tool for a launch");
            self.create_job(
                steam_id,
                missing,
                Kind::Install,
                folder_id.to_owned(),
                "public".into(),
            )?
        };
        Ok(Prepared::Waiting(vec![job.job_id]))
    }
}

/// A tool's display name, for messages.
pub(super) fn tool_name(list: &ToolList, app: AppId) -> String {
    list.by_app(app)
        .map_or_else(|| format!("app {app}"), |t| t.display_name.clone())
}

/// The `STEAM_COMPAT_*` and per-app variables of a launch through `ready`
/// (§5.2, §5.3), on top of `base`, the game's own.
pub(super) fn chain_env(
    base: Vec<(String, String)>,
    compat: &CompatEnv<'_>,
    extra: Option<&str>,
) -> Vec<(String, String)> {
    let mut vars = base;
    vars.extend(compat.vars().into_iter().map(|(k, v)| (k.to_owned(), v)));
    if let Some(text) = extra.filter(|t| !t.is_empty()) {
        let lookup = |name: &str| {
            vars.iter()
                .rev()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone())
                .or_else(|| std::env::var(name).ok())
        };
        match chain::assignment(text, lookup) {
            Some(kv) => vars.push(kv),
            None => tracing::warn!("app environment {text:?} is not NAME=VALUE; ignored"),
        }
    }
    vars
}
