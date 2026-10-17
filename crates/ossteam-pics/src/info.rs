// SPDX-License-Identifier: GPL-2.0-or-later
//! App and package info: the buffers PICS sends and typed views of them
//! (`pics.md` §3).

use ossteam_types::{AppId, DepotId};
use ossteam_vdf::{Object, Value, binary, text};

/// One app's or package's product info, as fetched and as cached.
#[derive(Debug, Clone, PartialEq)]
pub struct Info {
    pub id: u32,
    pub change_number: u32,
    /// The hash the server gave for the buffer.
    pub sha: Vec<u8>,
    /// The info is limited because no access token was presented.
    pub missing_token: bool,
    /// For apps the contents of `appinfo`; for packages the contents of
    /// the root key.
    pub data: Object,
}

#[derive(Debug, thiserror::Error)]
pub enum InfoError {
    #[error("the buffer is empty")]
    Empty,
    #[error("the app info is not UTF-8")]
    NotUtf8,
    #[error("the app info does not parse: {0}")]
    Text(#[from] text::Error),
    #[error("the package info does not parse: {0}")]
    Binary(#[from] binary::Error),
    #[error("the info has no {0} object")]
    NoRoot(&'static str),
}

/// Reads an app buffer: text KeyValues ended by a NUL, with one root key
/// `appinfo`. Returns that key's object.
pub fn parse_app_buffer(buf: &[u8]) -> Result<Object, InfoError> {
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    if end == 0 {
        return Err(InfoError::Empty);
    }
    let text = std::str::from_utf8(&buf[..end]).map_err(|_| InfoError::NotUtf8)?;
    let mut root = text::parse(text)?;
    take_object(&mut root, "appinfo").ok_or(InfoError::NoRoot("appinfo"))
}

/// Reads a package buffer: a 4-byte field that is skipped, then binary
/// KeyValues whose one root key is the package id. Returns that key's
/// object.
pub fn parse_package_buffer(buf: &[u8]) -> Result<Object, InfoError> {
    let Some(kv) = buf.get(4..).filter(|b| !b.is_empty()) else {
        return Err(InfoError::Empty);
    };
    let (mut root, _) = binary::parse(kv, binary::Keys::Inline)?;
    let key = root
        .entries
        .first()
        .filter(|e| matches!(e.value, Value::Object(_)))
        .map(|e| e.key.clone())
        .ok_or(InfoError::NoRoot("package"))?;
    take_object(&mut root, &key).ok_or(InfoError::NoRoot("package"))
}

fn take_object(root: &mut Object, key: &str) -> Option<Object> {
    let i = root
        .entries
        .iter()
        .position(|e| e.key.eq_ignore_ascii_case(key))?;
    match root.entries.swap_remove(i).value {
        Value::Object(o) => Some(o),
        _ => None,
    }
}

/// Reads a number that may be stored as a string (text KeyValues) or as an
/// integer (binary KeyValues).
fn number<T: TryFrom<i64>>(v: &Value) -> Option<T> {
    v.as_i64().and_then(|n| T::try_from(n).ok())
}

fn str_at<'a>(o: &'a Object, path: &[&str]) -> &'a str {
    o.get_path(path).and_then(Value::as_str).unwrap_or_default()
}

fn split_list(s: &str) -> Vec<String> {
    s.split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The values of an index-keyed list (`0`, `1`, ...) as ids.
fn id_list(o: Option<&Object>) -> Vec<u32> {
    o.map(|o| o.iter().filter_map(|(_, v)| number(v)).collect())
        .unwrap_or_default()
}

/// A typed view of an app's info.
#[derive(Debug, Clone, Copy)]
pub struct AppInfo<'a>(pub &'a Object);

/// A typed view of a package's info.
#[derive(Debug, Clone, Copy)]
pub struct PackageInfo<'a>(pub &'a Object);

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LaunchOption {
    /// The key under `config/launch`.
    pub index: u32,
    pub executable: String,
    pub arguments: String,
    pub description: String,
    pub kind: String,
    pub oslist: Vec<String>,
    pub osarch: String,
    pub betakey: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Depot {
    pub id: DepotId,
    pub name: String,
    pub oslist: Vec<String>,
    pub osarch: String,
    pub language: String,
    /// The DLC this depot belongs to, if any.
    pub dlc_app_id: Option<AppId>,
    /// The app whose depot this is, for depots shared from another app.
    pub from_app: Option<AppId>,
    pub shared_install: bool,
    pub max_size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Branch {
    pub name: String,
    pub build_id: Option<u32>,
    pub description: String,
    pub password_required: bool,
    pub time_updated: Option<u64>,
}

impl AppInfo<'_> {
    pub fn name(&self) -> &str {
        str_at(self.0, &["common", "name"])
    }

    /// The type as Steam writes it (`Game`, `DLC`, ...).
    pub fn app_type(&self) -> &str {
        str_at(self.0, &["common", "type"])
    }

    pub fn is_dlc(&self) -> bool {
        self.app_type().eq_ignore_ascii_case("dlc")
    }

    pub fn parent(&self) -> Option<AppId> {
        self.0
            .get_path(&["common", "parent"])
            .and_then(number)
            .filter(|&n: &u32| n != 0)
            .map(AppId)
    }

    pub fn oslist(&self) -> Vec<String> {
        split_list(str_at(self.0, &["common", "oslist"]))
    }

    /// The DLC the app lists in `extended/listofdlc`.
    pub fn dlc(&self) -> Vec<AppId> {
        str_at(self.0, &["extended", "listofdlc"])
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .map(AppId)
            .collect()
    }

    pub fn install_dir(&self) -> &str {
        str_at(self.0, &["config", "installdir"])
    }

    pub fn launch_options(&self) -> Vec<LaunchOption> {
        let Some(launch) = self
            .0
            .get_path(&["config", "launch"])
            .and_then(Value::as_object)
        else {
            return Vec::new();
        };
        launch
            .iter()
            .filter_map(|(k, v)| Some((k.parse().ok()?, v.as_object()?)))
            .map(|(index, o)| LaunchOption {
                index,
                executable: str_at(o, &["executable"]).into(),
                arguments: str_at(o, &["arguments"]).into(),
                description: str_at(o, &["description"]).into(),
                kind: str_at(o, &["type"]).into(),
                oslist: split_list(str_at(o, &["config", "oslist"])),
                osarch: str_at(o, &["config", "osarch"]).into(),
                betakey: str_at(o, &["config", "betakey"]).into(),
            })
            .collect()
    }

    /// The depots, in the order the info lists them. Other keys under
    /// `depots`, such as `branches`, are not depots and are skipped.
    pub fn depots(&self) -> Vec<Depot> {
        let Some(depots) = self.0.get_object("depots") else {
            return Vec::new();
        };
        depots
            .iter()
            .filter_map(|(k, v)| Some((k.parse().ok()?, v.as_object()?)))
            .map(|(id, o)| Depot {
                id: DepotId(id),
                name: str_at(o, &["name"]).into(),
                oslist: split_list(str_at(o, &["config", "oslist"])),
                osarch: str_at(o, &["config", "osarch"]).into(),
                language: str_at(o, &["config", "language"]).into(),
                dlc_app_id: o.get("dlcappid").and_then(number).map(AppId),
                from_app: o.get("depotfromapp").and_then(number).map(AppId),
                shared_install: o.get("sharedinstall").and_then(Value::as_bool) == Some(true),
                max_size: o.get("maxsize").and_then(number),
            })
            .collect()
    }

    pub fn branches(&self) -> Vec<Branch> {
        let Some(branches) = self
            .0
            .get_path(&["depots", "branches"])
            .and_then(Value::as_object)
        else {
            return Vec::new();
        };
        branches
            .iter()
            .filter_map(|(k, v)| Some((k, v.as_object()?)))
            .map(|(name, o)| Branch {
                name: name.into(),
                build_id: o.get("buildid").and_then(number),
                description: str_at(o, &["description"]).into(),
                password_required: o.get("pwdrequired").and_then(Value::as_bool) == Some(true),
                time_updated: o.get("timeupdated").and_then(number),
            })
            .collect()
    }
}

impl PackageInfo<'_> {
    pub fn app_ids(&self) -> Vec<AppId> {
        id_list(self.0.get_object("appids"))
            .into_iter()
            .map(AppId)
            .collect()
    }

    pub fn depot_ids(&self) -> Vec<DepotId> {
        id_list(self.0.get_object("depotids"))
            .into_iter()
            .map(DepotId)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const APP: &str = r#""appinfo"
{
	"appid"		"480"
	"common"
	{
		"name"		"Spacewar"
		"type"		"Game"
		"oslist"		"windows,linux"
	}
	"extended"
	{
		"listofdlc"		"110902, 110903"
	}
	"config"
	{
		"installdir"		"Spacewar"
		"launch"
		{
			"0"
			{
				"executable"		"SteamworksExample.exe"
				"type"		"default"
				"config"
				{
					"oslist"		"windows"
				}
			}
		}
	}
	"depots"
	{
		"481"
		{
			"name"		"Spacewar Content"
			"maxsize"		"1048576"
			"config"
			{
				"oslist"		"windows"
			}
		}
		"228988"
		{
			"depotfromapp"		"228980"
			"sharedinstall"		"1"
		}
		"branches"
		{
			"public"
			{
				"buildid"		"1234"
				"timeupdated"		"1700000000"
			}
			"beta"
			{
				"buildid"		"1300"
				"pwdrequired"		"1"
				"description"		"testing"
			}
		}
	}
}
"#;

    fn app_buffer() -> Vec<u8> {
        let mut b = APP.as_bytes().to_vec();
        b.push(0);
        b
    }

    #[test]
    fn reads_app_info() {
        let o = parse_app_buffer(&app_buffer()).unwrap();
        let a = AppInfo(&o);
        assert_eq!(a.name(), "Spacewar");
        assert_eq!(a.app_type(), "Game");
        assert!(!a.is_dlc());
        assert_eq!(a.oslist(), ["windows", "linux"]);
        assert_eq!(a.dlc(), [AppId(110902), AppId(110903)]);
        assert_eq!(a.install_dir(), "Spacewar");
        let l = a.launch_options();
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].executable, "SteamworksExample.exe");
        assert_eq!(l[0].oslist, ["windows"]);
        let d = a.depots();
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].max_size, Some(1_048_576));
        assert_eq!(d[1].from_app, Some(AppId(228980)));
        assert!(d[1].shared_install);
        let b = a.branches();
        assert_eq!(b[0].name, "public");
        assert_eq!(b[0].build_id, Some(1234));
        assert!(b[1].password_required);
    }

    #[test]
    fn dlc_has_a_parent() {
        let buf = b"\"appinfo\" { \"common\" { \"type\" \"DLC\" \"parent\" \"480\" } }\0";
        let o = parse_app_buffer(buf).unwrap();
        assert!(AppInfo(&o).is_dlc());
        assert_eq!(AppInfo(&o).parent(), Some(AppId(480)));
    }

    #[test]
    fn rejects_bad_app_buffers() {
        assert!(matches!(parse_app_buffer(b"\0"), Err(InfoError::Empty)));
        assert!(matches!(
            parse_app_buffer(b"\"other\" { }\0"),
            Err(InfoError::NoRoot(_))
        ));
        assert!(parse_app_buffer(b"\"appinfo\" {\0").is_err());
    }

    #[test]
    fn reads_package_info() {
        let mut appids = Object::new();
        appids.push("0", Value::Int32(480));
        appids.push("1", Value::Int32(481));
        let mut depots = Object::new();
        depots.push("0", Value::Int32(481));
        let mut pkg = Object::new();
        pkg.push("packageid", Value::Int32(1000));
        pkg.push("appids", appids);
        pkg.push("depotids", depots);
        let mut root = Object::new();
        root.push("1000", pkg);
        let mut buf = 1u32.to_le_bytes().to_vec();
        buf.extend(binary::to_vec(&root));

        let o = parse_package_buffer(&buf).unwrap();
        let p = PackageInfo(&o);
        assert_eq!(p.app_ids(), [AppId(480), AppId(481)]);
        assert_eq!(p.depot_ids(), [DepotId(481)]);
        assert!(matches!(parse_package_buffer(&[1, 0, 0, 0]), Err(InfoError::Empty)));
    }
}
