// SPDX-License-Identifier: GPL-2.0-or-later
//! Which depots an install takes, and which manifest of each (`content.md`
//! §6, CONT-2).
//!
//! This reads the app info's `depots` object directly; the typed views in
//! `ossteam-pics` sit above this crate. Entitlement (rule 8) is decided by
//! the depot key requests, after this.

use ossteam_types::{AppId, DepotId, ManifestId};
use ossteam_vdf::{Object, Value};

/// The host the install is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Host {
    /// `windows`, `macos` or `linux`, as app info writes them.
    pub os: &'static str,
    /// `32` or `64`.
    pub arch: &'static str,
}

impl Host {
    /// The machine this build runs on.
    pub fn this() -> Self {
        let os = if cfg!(windows) {
            "windows"
        } else if cfg!(target_os = "macos") {
            "macos"
        } else {
            "linux"
        };
        let arch = if cfg!(target_pointer_width = "64") {
            "64"
        } else {
            "32"
        };
        Self { os, arch }
    }
}

/// What the user's install asks for.
pub struct Wanted<'a> {
    pub branch: &'a str,
    /// The `content.language` setting.
    pub language: &'a str,
    /// The license granting the app carries the low-violence flag.
    pub low_violence: bool,
    pub host: Host,
    /// Whether the library holds an app, for DLC depots.
    pub owns: &'a dyn Fn(AppId) -> bool,
    /// Whether the user turned an optional DLC on for this app
    /// (`content.optional_dlc.<app id>`).
    pub optional_on: &'a dyn Fn(AppId) -> bool,
}

/// One depot the install takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chosen {
    pub depot: DepotId,
    pub manifest: ManifestId,
    /// For a depot shared from another app (`depotfromapp`): that app,
    /// whose info holds the manifest.
    pub from_app: Option<AppId>,
    /// Set with `sharedinstall`: the files go to `from_app`'s install
    /// directory.
    pub shared_install: bool,
    pub dlc: Option<AppId>,
    /// The optional DLC (`config/optionaldlc`) that this depot belongs to,
    /// which the user turned on.
    pub optional: Option<AppId>,
    /// `maxsize`, for the estimate before manifests are fetched.
    pub max_size: Option<u64>,
}

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum ChooseError {
    #[error("the app has no branch {0:?}")]
    NoBranch(String),
    #[error("branch {0:?} is password-protected, which is not supported yet")]
    PasswordBranch(String),
    #[error("depot {0} has only an encrypted manifest on this branch, which is not supported yet")]
    EncryptedManifest(DepotId),
    #[error("depot {depot} names manifest {value:?}, which is not a manifest id")]
    BadManifest { depot: DepotId, value: String },
    #[error("depot {depot} is shared from app {app}, whose info is not available")]
    MissingApp { depot: DepotId, app: AppId },
    #[error("depot {0} is shared from an app that shares it from another in turn")]
    SharedTwice(DepotId),
}

fn number<T: TryFrom<i64>>(v: &Value) -> Option<T> {
    v.as_i64().and_then(|n| T::try_from(n).ok())
}

fn str_at<'a>(o: &'a Object, path: &[&str]) -> &'a str {
    o.get_path(path).and_then(Value::as_str).unwrap_or_default()
}

fn flag(o: &Object, path: &[&str]) -> bool {
    o.get_path(path).and_then(Value::as_bool) == Some(true)
}

/// Whether a comma-separated list is empty or names `want`.
fn list_has(list: &str, want: &str) -> bool {
    let mut items = list
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .peekable();
    items.peek().is_none() || items.any(|s| s.eq_ignore_ascii_case(want))
}

/// The optional DLC a depot entry belongs to: its `config/optionaldlc`
/// (rule 5), `None` for a depot that is not optional.
pub fn optional_dlc(entry: &Object) -> Option<AppId> {
    entry
        .get_path(&["config", "optionaldlc"])
        .and_then(number)
        .filter(|&n: &u32| n != 0)
        .map(AppId)
}

/// Checks the branch (`content.md` §6, last paragraph). `depots` is the
/// app info's `depots` object.
pub fn check_branch(depots: &Object, branch: &str) -> Result<(), ChooseError> {
    if branch == "public" {
        return Ok(());
    }
    let b = depots
        .get_path(&["branches", branch])
        .and_then(Value::as_object)
        .ok_or_else(|| ChooseError::NoBranch(branch.to_owned()))?;
    if flag(b, &["pwdrequired"]) {
        return Err(ChooseError::PasswordBranch(branch.to_owned()));
    }
    Ok(())
}

/// The manifest a depot entry names for `branch`: `Ok(None)` when it has
/// none (rule 6).
fn manifest_of(
    depot: DepotId,
    entry: &Object,
    branch: &str,
) -> Result<Option<ManifestId>, ChooseError> {
    let Some(v) = entry.get_path(&["manifests", branch]) else {
        if entry.get_path(&["encryptedmanifests", branch]).is_some() {
            return Err(ChooseError::EncryptedManifest(depot));
        }
        return Ok(None);
    };
    let gid = match v {
        Value::Object(o) => o.get("gid"),
        other => Some(other),
    };
    let bad = || ChooseError::BadManifest {
        depot,
        value: gid
            .and_then(Value::to_text_scalar)
            .unwrap_or_else(|| "(an object without gid)".into()),
    };
    let id = gid.and_then(Value::as_u64).ok_or_else(bad)?;
    if id == 0 {
        return Err(bad());
    }
    Ok(Some(ManifestId(id)))
}

/// The depots an install of the app takes, in the order its info lists
/// them (`content.md` §6, rules 1 to 7). `app` is the app's info (the
/// contents of `appinfo`); `other` looks up the info of apps that depots
/// are shared from.
pub fn choose<'o>(
    app: &Object,
    other: &dyn Fn(AppId) -> Option<&'o Object>,
    want: &Wanted<'_>,
) -> Result<Vec<Chosen>, ChooseError> {
    let Some(depots) = app.get_object("depots") else {
        return Ok(Vec::new());
    };
    check_branch(depots, want.branch)?;
    let mut out = Vec::new();
    for (key, value) in depots.iter() {
        let (Ok(id), Some(entry)) = (key.parse::<u32>(), value.as_object()) else {
            continue;
        };
        let depot = DepotId(id);
        // Rules 1 to 3.
        let oslist = str_at(entry, &["config", "oslist"]);
        if !list_has(oslist, want.host.os) {
            continue;
        }
        let osarch = str_at(entry, &["config", "osarch"]).trim();
        if !osarch.is_empty() && osarch != want.host.arch {
            continue;
        }
        let language = str_at(entry, &["config", "language"]).trim();
        if !language.is_empty() && !language.eq_ignore_ascii_case(want.language) {
            continue;
        }
        // Rule 4.
        if flag(entry, &["config", "lowviolence"]) && !want.low_violence {
            continue;
        }
        // Rule 5.
        let dlc = entry
            .get("dlcappid")
            .and_then(number)
            .filter(|&n: &u32| n != 0)
            .map(AppId);
        if let Some(d) = dlc
            && !(want.owns)(d)
        {
            continue;
        }
        let optional = optional_dlc(entry);
        if let Some(d) = optional
            && !(want.optional_on)(d)
        {
            continue;
        }
        // Rules 6 and 7.
        let from_app = entry
            .get("depotfromapp")
            .and_then(number)
            .filter(|&n: &u32| n != 0)
            .map(AppId);
        let manifest = match (entry.get("manifests"), from_app) {
            (None, Some(from)) if entry.get("encryptedmanifests").is_none() => {
                let source = other(from)
                    .and_then(|o| o.get_path(&["depots", key]))
                    .and_then(Value::as_object)
                    .ok_or(ChooseError::MissingApp { depot, app: from })?;
                if source.get("manifests").is_none() && source.get("depotfromapp").is_some() {
                    return Err(ChooseError::SharedTwice(depot));
                }
                manifest_of(depot, source, want.branch)?
            }
            _ => manifest_of(depot, entry, want.branch)?,
        };
        let Some(manifest) = manifest else {
            continue;
        };
        out.push(Chosen {
            depot,
            manifest,
            from_app,
            shared_install: from_app.is_some() && flag(entry, &["sharedinstall"]),
            dlc,
            optional,
            max_size: entry.get("maxsize").and_then(Value::as_u64),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Object {
        ossteam_vdf::text::parse(s).unwrap()
    }

    const APP: &str = r#"
"depots"
{
    "481"  { "config" { "oslist" "windows,linux" } "manifests" { "public" "111" "beta" { "gid" "112" "size" "5" } } "maxsize" "1000" }
    "482"  { "config" { "oslist" "windows" } "manifests" { "public" "121" } }
    "483"  { "config" { "language" "german" } "manifests" { "public" "131" } }
    "484"  { "config" { "osarch" "32" } "manifests" { "public" "141" } }
    "485"  { "config" { "lowviolence" "1" } "manifests" { "public" "151" } }
    "486"  { "dlcappid" "990" "manifests" { "public" "161" } }
    "487"  { "dlcappid" "991" "manifests" { "public" "171" } }
    "488"  { "manifests" { "beta" "181" } }
    "489"  { "dlcappid" "990" "config" { "optionaldlc" "990" } "manifests" { "public" "191" } }
    "228990" { "depotfromapp" "228980" "sharedinstall" "1" }
    "baselanguages" "english"
    "branches"
    {
        "public" { "buildid" "1" }
        "beta" { "buildid" "2" }
        "locked" { "buildid" "3" "pwdrequired" "1" }
    }
}
"#;

    const REDIST: &str = r#"
"depots"
{
    "228990" { "manifests" { "public" "9999" } }
}
"#;

    fn chosen(branch: &str, os: &'static str, low_violence: bool) -> Result<Vec<u32>, ChooseError> {
        let app = parse(APP);
        let redist = parse(REDIST);
        let other = |a: AppId| (a == AppId(228980)).then_some(&redist);
        let owns = |a: AppId| a == AppId(990);
        let want = Wanted {
            branch,
            language: "english",
            low_violence,
            host: Host { os, arch: "64" },
            owns: &owns,
            optional_on: &|_| false,
        };
        choose(&app, &other, &want).map(|v| v.into_iter().map(|c| c.depot.0).collect())
    }

    #[test]
    fn applies_platform_language_arch_violence_and_dlc() {
        assert_eq!(
            chosen("public", "linux", false).unwrap(),
            [481, 486, 228990]
        );
        assert_eq!(
            chosen("public", "windows", false).unwrap(),
            [481, 482, 486, 228990]
        );
        assert_eq!(
            chosen("public", "linux", true).unwrap(),
            [481, 485, 486, 228990]
        );
    }

    #[test]
    fn branches_pick_their_manifests() {
        let app = parse(APP);
        let other = |_| None;
        let owns = |_| false;
        let want = Wanted {
            branch: "beta",
            language: "english",
            low_violence: false,
            host: Host {
                os: "linux",
                arch: "64",
            },
            owns: &owns,
            optional_on: &|_| false,
        };
        let got = choose(&app, &other, &want);
        // 228990 has no manifests of its own and its source app is unknown.
        assert_eq!(
            got,
            Err(ChooseError::MissingApp {
                depot: DepotId(228990),
                app: AppId(228980)
            })
        );
        assert_eq!(chosen("beta", "linux", false).unwrap(), [481, 488]);
        assert_eq!(
            chosen("nope", "linux", false),
            Err(ChooseError::NoBranch("nope".into()))
        );
        assert_eq!(
            chosen("locked", "linux", false),
            Err(ChooseError::PasswordBranch("locked".into()))
        );
    }

    #[test]
    fn shared_depots_take_the_source_apps_manifest() {
        let app = parse(APP);
        let redist = parse(REDIST);
        let other = |a: AppId| (a == AppId(228980)).then_some(&redist);
        let owns = |_| false;
        let want = Wanted {
            branch: "public",
            language: "english",
            low_violence: false,
            host: Host {
                os: "linux",
                arch: "64",
            },
            owns: &owns,
            optional_on: &|_| false,
        };
        let got = choose(&app, &other, &want).unwrap();
        let shared = got.iter().find(|c| c.depot == DepotId(228990)).unwrap();
        assert_eq!(shared.manifest, ManifestId(9999));
        assert_eq!(shared.from_app, Some(AppId(228980)));
        assert!(shared.shared_install);
        let first = &got[0];
        assert_eq!(
            (first.manifest, first.max_size),
            (ManifestId(111), Some(1000))
        );
    }

    #[test]
    fn encrypted_and_malformed_manifests_fail() {
        let owns = |_| false;
        let other = |_| None;
        let want = Wanted {
            branch: "public",
            language: "english",
            low_violence: false,
            host: Host {
                os: "linux",
                arch: "64",
            },
            owns: &owns,
            optional_on: &|_| false,
        };
        let enc = parse(
            r#""depots" { "5" { "encryptedmanifests" { "public" { "encrypted_gid_2" "AB" } } } }"#,
        );
        assert_eq!(
            choose(&enc, &other, &want),
            Err(ChooseError::EncryptedManifest(DepotId(5)))
        );
        let bad = parse(r#""depots" { "5" { "manifests" { "public" "x" } } }"#);
        assert!(matches!(
            choose(&bad, &other, &want),
            Err(ChooseError::BadManifest { .. })
        ));
        let twice_src = parse(r#""depots" { "5" { "depotfromapp" "3" } }"#);
        let twice = parse(r#""depots" { "5" { "depotfromapp" "2" } }"#);
        let other2 = |a: AppId| (a == AppId(2)).then_some(&twice_src);
        assert_eq!(
            choose(&twice, &other2, &want),
            Err(ChooseError::SharedTwice(DepotId(5)))
        );
    }

    #[test]
    fn optional_dlc_only_when_turned_on() {
        let app = parse(APP);
        let redist = parse(REDIST);
        let other = |a: AppId| (a == AppId(228980)).then_some(&redist);
        let owns = |a: AppId| a == AppId(990);
        let on = |a: AppId| a == AppId(990);
        let want = Wanted {
            branch: "public",
            language: "english",
            low_violence: false,
            host: Host {
                os: "linux",
                arch: "64",
            },
            owns: &owns,
            optional_on: &on,
        };
        let got = choose(&app, &other, &want).unwrap();
        let ids: Vec<u32> = got.iter().map(|c| c.depot.0).collect();
        assert_eq!(ids, [481, 486, 489, 228990]);
        assert_eq!(got[2].optional, Some(AppId(990)));
        assert_eq!(got[1].optional, None);
        // Turned on but not owned: rule 5's ownership still applies.
        let want = Wanted {
            owns: &|_| false,
            ..want
        };
        let ids: Vec<u32> = choose(&app, &other, &want)
            .unwrap()
            .iter()
            .map(|c| c.depot.0)
            .collect();
        assert_eq!(ids, [481, 228990]);
    }

    #[test]
    fn os_lists() {
        assert!(list_has("", "linux"));
        assert!(list_has(" , ", "linux"));
        assert!(list_has("windows, Linux", "linux"));
        assert!(!list_has("windows,macos", "linux"));
    }
}
