// SPDX-License-Identifier: GPL-2.0-or-later
//! `toolmanifest.vdf` in a tool's install directory (`compat-tools.md`
//! §3).

use std::path::Path;

use ossteam_types::AppId;
use ossteam_vdf::{Object, Value};

/// The file's name.
pub const FILE: &str = "toolmanifest.vdf";

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ToolManifest {
    /// `1` unless the file says otherwise.
    pub version: u32,
    pub commandline: String,
    pub requires: Option<AppId>,
    pub use_subprocess_reaper: bool,
    pub layer_name: String,
    pub filter_exclusive_priority: Option<u32>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("cannot read {path}: {source}")]
    Read {
        path: String,
        source: std::io::Error,
    },
    #[error("{path} is not KeyValues: {source}")]
    Parse {
        path: String,
        source: ossteam_vdf::text::Error,
    },
    #[error("{path} has no manifest")]
    Empty { path: String },
}

impl ToolManifest {
    /// The manifest in a tool's install directory.
    pub fn read(tool_dir: &Path) -> Result<Self, Error> {
        let path = tool_dir.join(FILE);
        let shown = path.display().to_string();
        let text = std::fs::read(&path).map_err(|source| Error::Read {
            path: shown.clone(),
            source,
        })?;
        Self::parse(&String::from_utf8_lossy(&text), &shown)
    }

    /// The manifest in `text`; `path` names it in errors.
    pub fn parse(text: &str, path: &str) -> Result<Self, Error> {
        let root = ossteam_vdf::text::parse(text).map_err(|source| Error::Parse {
            path: path.to_owned(),
            source,
        })?;
        let m = root.get_object("manifest").ok_or_else(|| Error::Empty {
            path: path.to_owned(),
        })?;
        Ok(Self {
            version: number(m, "version").unwrap_or(1),
            commandline: m
                .get("commandline")
                .and_then(Value::to_text_scalar)
                .unwrap_or_default(),
            requires: number(m, "require_tool_appid")
                .filter(|&a| a != 0)
                .map(AppId),
            use_subprocess_reaper: number(m, "use_tool_subprocess_reaper") == Some(1),
            layer_name: m
                .get("compatmanager_layer_name")
                .and_then(Value::to_text_scalar)
                .unwrap_or_default(),
            filter_exclusive_priority: number(m, "filter_exclusive_priority"),
        })
    }
}

fn number(o: &Object, key: &str) -> Option<u32> {
    o.get(key)
        .and_then(|v| v.as_u64().or_else(|| v.as_str()?.trim().parse().ok()))
        .and_then(|n| u32::try_from(n).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniper() {
        let m = ToolManifest::parse(
            r#"// Generated file, do not edit
"manifest"
{
	"commandline" "/_v2-entry-point --verb=%verb% --"
	"filter_exclusive_priority" "3"
	"version" "2"
	"use_tool_subprocess_reaper" "1"
	"compatmanager_layer_name" "container-runtime"
}
"#,
            "t",
        )
        .unwrap();
        assert_eq!(
            m,
            ToolManifest {
                version: 2,
                commandline: "/_v2-entry-point --verb=%verb% --".into(),
                requires: None,
                use_subprocess_reaper: true,
                layer_name: "container-runtime".into(),
                filter_exclusive_priority: Some(3),
            }
        );
    }

    #[test]
    fn scout_requires_soldier() {
        let m = ToolManifest::parse(
            r#""manifest"
{
  "version" "2"
  "commandline" "/scout-on-soldier-entry-point-v2 --"
  "compatmanager_layer_name" "scout-in-container"
  "filter_exclusive_priority" "10"
  "require_tool_appid" "1391110"
}"#,
            "t",
        )
        .unwrap();
        assert_eq!(m.requires, Some(AppId(1391110)));
        assert!(!m.use_subprocess_reaper);
    }

    #[test]
    fn version_defaults_to_one_and_errors_name_the_file() {
        let m = ToolManifest::parse(r#""manifest" { "commandline" "/run" }"#, "t").unwrap();
        assert_eq!(m.version, 1);
        let e = ToolManifest::parse(r#""other" { }"#, "/x/toolmanifest.vdf").unwrap_err();
        assert_eq!(e.to_string(), "/x/toolmanifest.vdf has no manifest");
    }
}
