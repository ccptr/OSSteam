// SPDX-License-Identifier: GPL-2.0-or-later
//! The core's side of compatibility tools (`compat-tools.md`): which tool
//! an app gets, which platform its depots are for, and the checks of the
//! `compat.*` settings.

use ossteam_content::depots::Host;
use ossteam_ipc::RemoteError;
use ossteam_launch::compat::{MANIFESTS_APP, Mapping, MappingInput, ToolList};
use ossteam_types::AppId;
use ossteam_vdf::Object;

use super::{Core, invalid};
use crate::config;

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
