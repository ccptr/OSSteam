// SPDX-License-Identifier: GPL-2.0-or-later
//! The library: the apps an account's licenses grant (`pics.md` §7,
//! PICS-3). It is derived from licenses and cached info, never stored.

use std::collections::{BTreeMap, BTreeSet};

use ossteam_types::{AppId, PackageId};

use crate::info::{AppInfo, PackageInfo};
use crate::license::License;

/// The packages that the licenses put in this account's library, in id
/// order and without repeats (`pics.md` §1).
pub fn granted_packages(licenses: &[License], own_account_id: u32) -> Vec<PackageId> {
    licenses
        .iter()
        .filter(|l| l.grants(own_account_id))
        .map(|l| l.package_id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// One app in the library.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LibraryApp {
    pub id: AppId,
    /// Empty when the app's info is not available.
    pub name: String,
    /// The type as Steam writes it (`Game`, `DLC`, ...); empty when the
    /// info is not available.
    pub app_type: String,
    /// For DLC: the app it belongs to.
    pub parent: Option<AppId>,
    /// Whether the app's info is available. Apps without it are still
    /// listed, by id.
    pub has_info: bool,
    /// The granted DLC listed under this app.
    pub dlc: Vec<LibraryApp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Library {
    /// Top-level apps: everything but DLC whose parent is also granted,
    /// sorted by name and then id. Apps without a name sort last.
    pub apps: Vec<LibraryApp>,
    /// Granted packages whose info is not available, so the apps they grant
    /// are not known and cannot be listed.
    pub packages_without_info: Vec<PackageId>,
}

impl Library {
    /// Builds the library from the granted packages and whatever info is
    /// available for them and their apps.
    pub fn build<'a>(
        packages: &[PackageId],
        package_info: impl Fn(PackageId) -> Option<PackageInfo<'a>>,
        app_info: impl Fn(AppId) -> Option<AppInfo<'a>>,
    ) -> Self {
        let mut apps = BTreeSet::new();
        let mut packages_without_info = Vec::new();
        for &p in packages {
            match package_info(p) {
                Some(info) => apps.extend(info.app_ids()),
                None => packages_without_info.push(p),
            }
        }

        let mut entries: BTreeMap<AppId, LibraryApp> = apps
            .iter()
            .map(|&id| {
                let entry = match app_info(id) {
                    Some(a) => LibraryApp {
                        id,
                        name: a.name().to_owned(),
                        app_type: a.app_type().to_owned(),
                        parent: a.is_dlc().then(|| a.parent()).flatten(),
                        has_info: true,
                        dlc: Vec::new(),
                    },
                    None => LibraryApp {
                        id,
                        ..Default::default()
                    },
                };
                (id, entry)
            })
            .collect();

        // DLC moves under its parent when the parent is granted too. Only
        // apps that are not DLC themselves take DLC, so nesting is one level
        // deep even if some info names a DLC as a parent.
        let nested: Vec<(AppId, AppId)> = entries
            .values()
            .filter_map(|a| {
                let p = a.parent?;
                let parent = entries.get(&p)?;
                (p != a.id && parent.parent.is_none()).then_some((a.id, p))
            })
            .collect();
        for (id, parent) in nested {
            let dlc = entries.remove(&id).expect("listed above");
            entries
                .get_mut(&parent)
                .expect("parents are never nested")
                .dlc
                .push(dlc);
        }

        let mut top: Vec<LibraryApp> = entries.into_values().collect();
        for a in &mut top {
            sort(&mut a.dlc);
        }
        sort(&mut top);
        Self {
            apps: top,
            packages_without_info,
        }
    }

    /// Every app in the library, DLC included, in listing order.
    pub fn iter(&self) -> impl Iterator<Item = &LibraryApp> {
        self.apps.iter().flat_map(|a| std::iter::once(a).chain(&a.dlc))
    }

    pub fn find(&self, id: AppId) -> Option<&LibraryApp> {
        self.iter().find(|a| a.id == id)
    }

    pub fn len(&self) -> usize {
        self.iter().count()
    }

    pub fn is_empty(&self) -> bool {
        self.apps.is_empty()
    }
}

fn sort(apps: &mut [LibraryApp]) {
    apps.sort_by(|a, b| {
        (a.name.is_empty(), a.name.to_lowercase(), a.id).cmp(&(
            b.name.is_empty(),
            b.name.to_lowercase(),
            b.id,
        ))
    });
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use ossteam_vdf::{Object, Value};

    use super::*;
    use crate::license::flags;

    fn license(package: u32, flags: u32, owner: u32) -> License {
        License {
            package_id: PackageId(package),
            flags,
            license_type: 0,
            owner_id: owner,
            time_created: 0,
            access_token: 0,
        }
    }

    fn package(apps: &[u32]) -> Object {
        let mut ids = Object::new();
        for (i, a) in apps.iter().enumerate() {
            ids.push(i.to_string(), Value::Int32(i32::try_from(*a).unwrap()));
        }
        Object::from_iter([("appids", ids)])
    }

    fn app(name: &str, kind: &str, parent: Option<u32>) -> Object {
        let mut common = Object::from_iter([("name", name), ("type", kind)]);
        if let Some(p) = parent {
            common.push("parent", p.to_string());
        }
        Object::from_iter([("common", common)])
    }

    #[test]
    fn only_licenses_in_force_grant_packages() {
        let l = [
            license(30, 0, 5),
            license(10, flags::RENEW, 0),
            license(10, 0, 5),
            license(20, flags::PENDING, 5),
            license(40, 0, 6),
        ];
        assert_eq!(granted_packages(&l, 5), [PackageId(10), PackageId(30)]);
    }

    #[test]
    fn dlc_is_grouped_under_a_granted_parent() {
        let packages: HashMap<PackageId, Object> = [
            (PackageId(1), package(&[480, 481, 482])),
            (PackageId(2), package(&[500, 481])),
        ]
        .into();
        let apps: HashMap<AppId, Object> = [
            (AppId(480), app("Spacewar", "Game", None)),
            (AppId(481), app("Spacewar Soundtrack", "DLC", Some(480))),
            (AppId(482), app("Another Pack", "dlc", Some(480))),
            (AppId(500), app("Orphan DLC", "DLC", Some(999))),
        ]
        .into();
        let lib = Library::build(
            &[PackageId(1), PackageId(2), PackageId(3)],
            |p| packages.get(&p).map(PackageInfo),
            |a| apps.get(&a).map(AppInfo),
        );

        assert_eq!(lib.packages_without_info, [PackageId(3)]);
        let top: Vec<u32> = lib.apps.iter().map(|a| a.id.0).collect();
        assert_eq!(top, [500, 480]);
        assert_eq!(lib.apps[0].parent, Some(AppId(999)));
        let dlc: Vec<u32> = lib.apps[1].dlc.iter().map(|a| a.id.0).collect();
        assert_eq!(dlc, [482, 481]);
        assert_eq!(lib.len(), 4);
        assert_eq!(lib.find(AppId(481)).unwrap().name, "Spacewar Soundtrack");
    }

    #[test]
    fn apps_without_info_are_listed_by_id() {
        let packages: HashMap<PackageId, Object> = [(PackageId(1), package(&[7, 480]))].into();
        let named = app("Spacewar", "Game", None);
        let lib = Library::build(
            &[PackageId(1)],
            |p| packages.get(&p).map(PackageInfo),
            |a| (a == AppId(480)).then_some(AppInfo(&named)),
        );
        assert_eq!(lib.apps.len(), 2);
        assert_eq!(lib.apps[0].name, "Spacewar");
        assert_eq!(lib.apps[1].id, AppId(7));
        assert!(!lib.apps[1].has_info);
        assert!(lib.apps[1].name.is_empty());
    }

    #[test]
    fn a_non_dlc_parent_field_does_not_nest() {
        let packages: HashMap<PackageId, Object> = [(PackageId(1), package(&[10, 11]))].into();
        let apps: HashMap<AppId, Object> = [
            (AppId(10), app("Game", "Game", None)),
            (AppId(11), app("Demo", "Demo", Some(10))),
        ]
        .into();
        let lib = Library::build(
            &[PackageId(1)],
            |p| packages.get(&p).map(PackageInfo),
            |a| apps.get(&a).map(AppInfo),
        );
        assert_eq!(lib.apps.len(), 2);
        assert!(lib.apps.iter().all(|a| a.dlc.is_empty()));
    }

    #[test]
    fn dlc_of_dlc_stays_on_top() {
        let packages: HashMap<PackageId, Object> = [(PackageId(1), package(&[10, 11, 12]))].into();
        let apps: HashMap<AppId, Object> = [
            (AppId(10), app("Game", "Game", None)),
            (AppId(11), app("Pack", "DLC", Some(10))),
            (AppId(12), app("Pack Extra", "DLC", Some(11))),
        ]
        .into();
        let lib = Library::build(
            &[PackageId(1)],
            |p| packages.get(&p).map(PackageInfo),
            |a| apps.get(&a).map(AppInfo),
        );
        let top: Vec<u32> = lib.apps.iter().map(|a| a.id.0).collect();
        assert_eq!(top, [10, 12]);
        assert_eq!(lib.apps[0].dlc[0].id, AppId(11));
    }
}
