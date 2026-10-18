// SPDX-License-Identifier: GPL-2.0-or-later
//! Launch options from `config/launch` and which of them qualify
//! (`launch.md` §1, LAUNCH-1).

use ossteam_types::AppId;
use ossteam_vdf::{Object, Value};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LaunchOption {
    /// The key under `config/launch`.
    pub index: u32,
    pub executable: String,
    pub arguments: String,
    pub working_dir: String,
    pub description: String,
    /// `description_loc`: language to text.
    pub description_loc: Vec<(String, String)>,
    /// `type`, as written.
    pub kind: String,
    pub oslist: Vec<String>,
    pub osarch: String,
    pub betakey: String,
    pub owns_dlc: Vec<AppId>,
    pub realm: String,
}

/// What a launch runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Platform {
    /// As `oslist` writes it: `windows`, `macos` or `linux`.
    pub os: &'static str,
    pub is_64bit: bool,
}

impl Platform {
    /// The platform of a native launch on this machine, or `None` on an
    /// OS that Steam has no launch options for.
    pub fn native() -> Option<Self> {
        let os = match std::env::consts::OS {
            "linux" => "linux",
            "windows" => "windows",
            "macos" => "macos",
            _ => return None,
        };
        Some(Self {
            os,
            is_64bit: os_is_64bit(),
        })
    }
}

/// Whether the OS is 64-bit, which a 32-bit build of the daemon may run
/// on as well.
fn os_is_64bit() -> bool {
    if cfg!(target_pointer_width = "64") {
        return true;
    }
    #[cfg(windows)]
    {
        // A 32-bit process on 64-bit Windows runs under WOW64, which sets
        // this for it.
        std::env::var_os("PROCESSOR_ARCHITEW6432").is_some()
    }
    #[cfg(unix)]
    {
        matches!(
            rustix::system::uname().machine().to_bytes(),
            b"x86_64" | b"aarch64" | b"arm64"
        )
    }
}

/// Every launch option in an app's info, in index order. Keys that are
/// not decimal indexes, and values that are not objects, are skipped.
pub fn parse(appinfo: &Object) -> Vec<LaunchOption> {
    let Some(launch) = appinfo
        .get_path(&["config", "launch"])
        .and_then(Value::as_object)
    else {
        return Vec::new();
    };
    let mut options: Vec<LaunchOption> = launch
        .iter()
        .filter_map(|(k, v)| Some((k.parse().ok()?, v.as_object()?)))
        .map(|(index, o)| LaunchOption {
            index,
            executable: text(o, &["executable"]),
            arguments: text(o, &["arguments"]),
            working_dir: text(o, &["workingdir"]),
            description: text(o, &["description"]),
            description_loc: o
                .get_object("description_loc")
                .map(|d| {
                    d.iter()
                        .filter_map(|(k, v)| Some((k.to_owned(), v.as_str()?.to_owned())))
                        .collect()
                })
                .unwrap_or_default(),
            kind: text(o, &["type"]),
            oslist: list(&text(o, &["config", "oslist"])),
            osarch: text(o, &["config", "osarch"]),
            betakey: text(o, &["config", "betakey"]),
            owns_dlc: list(&text(o, &["config", "ownsdlc"]))
                .iter()
                .filter_map(|s| s.parse().ok().map(AppId))
                .collect(),
            realm: text(o, &["config", "realm"]),
        })
        .collect();
    options.sort_by_key(|o| o.index);
    options
}

fn text(o: &Object, path: &[&str]) -> String {
    o.get_path(path)
        .and_then(Value::to_text_scalar)
        .map(|s| s.trim().to_owned())
        .unwrap_or_default()
}

fn list(s: &str) -> Vec<String> {
    s.split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_owned)
        .collect()
}

impl LaunchOption {
    /// Whether the option can be launched on `platform`, from `branch`,
    /// by an account that owns the DLC `owns` says it does (`launch.md`
    /// §1).
    pub fn qualifies(
        &self,
        platform: Platform,
        branch: &str,
        owns: &dyn Fn(AppId) -> bool,
    ) -> bool {
        let os = self.oslist.is_empty()
            || self
                .oslist
                .iter()
                .any(|o| o.eq_ignore_ascii_case(platform.os));
        let arch = match self.osarch.as_str() {
            "" | "32" => true,
            "64" => platform.is_64bit,
            _ => false,
        };
        let beta = self.betakey.is_empty()
            || self
                .betakey
                .split(',')
                .any(|b| b.trim().eq_ignore_ascii_case(branch));
        let realm = self.realm.is_empty() || self.realm.eq_ignore_ascii_case("steamglobal");
        os && arch && beta && realm && self.owns_dlc.iter().all(|&d| owns(d))
    }

    /// The description in `language`, falling back to `description`.
    pub fn description_in(&self, language: &str) -> &str {
        self.description_loc
            .iter()
            .find(|(l, _)| l.eq_ignore_ascii_case(language))
            .map_or(self.description.as_str(), |(_, t)| t.as_str())
    }
}

/// Why no option could be chosen.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ChoiceError {
    #[error("the app has no launch option for {os}; it has options for {}", others.join(", "))]
    NoneQualifies {
        os: &'static str,
        others: Vec<String>,
    },
    #[error("the app has several launch options; choose one of {}", list_indexes(.0))]
    Ambiguous(Vec<u32>),
    #[error("launch option {0} does not exist")]
    Unknown(u32),
    #[error("launch option {0} cannot be launched here")]
    DoesNotQualify(u32),
}

fn list_indexes(v: &[u32]) -> String {
    v.iter().map(u32::to_string).collect::<Vec<_>>().join(", ")
}

/// The option a launch uses: `index` when given and it qualifies, or the
/// only option that qualifies.
pub fn choose<'a>(
    options: &'a [LaunchOption],
    index: Option<u32>,
    platform: Platform,
    branch: &str,
    owns: &dyn Fn(AppId) -> bool,
) -> Result<&'a LaunchOption, ChoiceError> {
    let qualifying: Vec<&LaunchOption> = options
        .iter()
        .filter(|o| o.qualifies(platform, branch, owns))
        .collect();
    if let Some(i) = index {
        return match options.iter().find(|o| o.index == i) {
            None => Err(ChoiceError::Unknown(i)),
            Some(o) if qualifying.iter().any(|q| q.index == i) => Ok(o),
            Some(_) => Err(ChoiceError::DoesNotQualify(i)),
        };
    }
    match qualifying.as_slice() {
        [only] => Ok(only),
        [] => {
            let mut others: Vec<String> = options
                .iter()
                .flat_map(|o| o.oslist.iter().map(|s| s.to_ascii_lowercase()))
                .collect();
            others.sort();
            others.dedup();
            if others.is_empty() {
                others.push("no platform".into());
            }
            Err(ChoiceError::NoneQualifies {
                os: platform.os,
                others,
            })
        }
        many => Err(ChoiceError::Ambiguous(
            many.iter().map(|o| o.index).collect(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINUX64: Platform = Platform {
        os: "linux",
        is_64bit: true,
    };
    const WIN32: Platform = Platform {
        os: "windows",
        is_64bit: false,
    };

    fn info() -> Object {
        ossteam_vdf::text::parse(
            r#""appinfo" {
                "config" {
                    "launch" {
                        "1" { "executable" "game.sh" "arguments" "-steam" "type" "default"
                              "config" { "oslist" "linux" "osarch" "64" } }
                        "0" { "executable" "bin\\game.exe" "workingdir" "bin"
                              "description" "Play" "description_loc" { "french" "Jouer" }
                              "config" { "OSList" "windows" } }
                        "2" { "executable" "beta.sh" "config" { "oslist" "linux" "BetaKey" "beta" } }
                        "3" { "executable" "dlc.sh" "config" { "oslist" "linux" "ownsdlc" "10, 11" } }
                        "4" { "executable" "cn.sh" "config" { "oslist" "linux" "realm" "steamchina" } }
                        "x" { "executable" "skipped" }
                    }
                }
            }"#,
        )
        .unwrap()
        .get_object("appinfo")
        .unwrap()
        .clone()
    }

    #[test]
    fn parses_in_index_order() {
        let o = parse(&info());
        assert_eq!(
            o.iter().map(|o| o.index).collect::<Vec<_>>(),
            [0, 1, 2, 3, 4]
        );
        assert_eq!(o[0].executable, "bin\\game.exe");
        assert_eq!(o[0].working_dir, "bin");
        assert_eq!(o[0].oslist, ["windows"]);
        assert_eq!(o[0].description_in("French"), "Jouer");
        assert_eq!(o[0].description_in("german"), "Play");
        assert_eq!(o[1].kind, "default");
        assert_eq!(o[1].osarch, "64");
        assert_eq!(o[2].betakey, "beta");
        assert_eq!(o[3].owns_dlc, [AppId(10), AppId(11)]);
        assert_eq!(o[4].realm, "steamchina");
    }

    #[test]
    fn qualification() {
        let o = parse(&info());
        let none = |_| false;
        let all = |_| true;
        assert!(!o[0].qualifies(LINUX64, "public", &none));
        assert!(o[0].qualifies(WIN32, "public", &none));
        assert!(o[1].qualifies(LINUX64, "public", &none));
        assert!(!o[1].qualifies(
            Platform {
                os: "linux",
                is_64bit: false
            },
            "public",
            &none
        ));
        assert!(!o[2].qualifies(LINUX64, "public", &none));
        assert!(o[2].qualifies(LINUX64, "Beta", &none));
        assert!(!o[3].qualifies(LINUX64, "public", &|d| d == AppId(10)));
        assert!(o[3].qualifies(LINUX64, "public", &all));
        assert!(!o[4].qualifies(LINUX64, "public", &all));
    }

    #[test]
    fn choosing() {
        let o = parse(&info());
        let none = |_| false;
        assert_eq!(choose(&o, None, LINUX64, "public", &none).unwrap().index, 1);
        assert_eq!(
            choose(&o, None, LINUX64, "beta", &none),
            Err(ChoiceError::Ambiguous(vec![1, 2]))
        );
        assert_eq!(
            choose(&o, Some(2), LINUX64, "beta", &none).unwrap().index,
            2
        );
        assert_eq!(
            choose(&o, Some(0), LINUX64, "public", &none),
            Err(ChoiceError::DoesNotQualify(0))
        );
        assert_eq!(
            choose(&o, Some(9), LINUX64, "public", &none),
            Err(ChoiceError::Unknown(9))
        );
        let mac = Platform {
            os: "macos",
            is_64bit: true,
        };
        let e = choose(&o, None, mac, "public", &none).unwrap_err();
        assert_eq!(
            e.to_string(),
            "the app has no launch option for macos; it has options for linux, windows"
        );
    }
}
