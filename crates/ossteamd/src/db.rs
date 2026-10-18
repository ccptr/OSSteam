// SPDX-License-Identifier: GPL-2.0-or-later
//! The daemon state database (SPEC D-6). It holds no secrets: tokens live
//! in the keyring (`secrets`).

use std::path::{Path, PathBuf};

use anyhow::Context;
use ossteam_ipc::v1;
use ossteam_types::{AppId, DepotId, ManifestId, SteamId};
use rusqlite::{Connection, OptionalExtension, params};

use crate::content::script::{Key, ScriptFile, ValueRef};

pub const FILE_NAME: &str = "state.db";

/// The schema version this build writes, kept in `PRAGMA user_version`.
const SCHEMA_VERSION: i64 = 7;

const SCHEMA_V1: &str = "
    CREATE TABLE accounts (
        steam_id     INTEGER PRIMARY KEY,
        account_name TEXT NOT NULL UNIQUE,
        persona_name TEXT NOT NULL DEFAULT ''
    );
    CREATE TABLE meta (
        key   TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );
";

/// Per-account choices, kept whether or not the account is remembered.
const SCHEMA_V2: &str = "
    CREATE TABLE account_settings (
        steam_id      INTEGER PRIMARY KEY,
        persona_state INTEGER NOT NULL
    );
";

/// Setting overrides made over IPC (`config`), as `SetSetting` writes
/// their values.
const SCHEMA_V3: &str = "
    CREATE TABLE settings (
        key   TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );
";

/// Library folders, installed apps and content jobs (`content.md` §8, §9;
/// `ipc.md` §5.5, §5.6). Manifest ids are `u64`s stored bit for bit as
/// `INTEGER`. Job kinds and states are the IPC enum numbers, which never
/// change (`ipc.md` §9).
const SCHEMA_V4: &str = "
    CREATE TABLE library_folders (
        id         TEXT PRIMARY KEY,
        path       TEXT NOT NULL UNIQUE,
        is_default INTEGER NOT NULL DEFAULT 0
    );
    CREATE TABLE installed_apps (
        app_id           INTEGER PRIMARY KEY,
        folder_id        TEXT NOT NULL REFERENCES library_folders (id),
        install_dir      TEXT NOT NULL,
        branch           TEXT NOT NULL,
        build_id         INTEGER NOT NULL,
        size             INTEGER NOT NULL,
        update_available INTEGER NOT NULL DEFAULT 0
    );
    CREATE TABLE installed_depots (
        app_id      INTEGER NOT NULL REFERENCES installed_apps (app_id) ON DELETE CASCADE,
        depot_id    INTEGER NOT NULL,
        manifest_id INTEGER NOT NULL,
        shared_from INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY (app_id, depot_id)
    );
    CREATE TABLE jobs (
        job_id        INTEGER PRIMARY KEY,
        steam_id      INTEGER NOT NULL,
        app_id        INTEGER NOT NULL,
        kind          INTEGER NOT NULL,
        state         INTEGER NOT NULL,
        priority      INTEGER NOT NULL DEFAULT 0,
        folder_id     TEXT NOT NULL,
        branch        TEXT NOT NULL,
        created       INTEGER NOT NULL,
        error_code    INTEGER NOT NULL DEFAULT 0,
        error_message TEXT NOT NULL DEFAULT '',
        journal       BLOB
    );
";

/// Install scripts of installed apps, and the registry values they wrote
/// (`install-scripts.md` §5). A `NULL` name is the key's default value.
const SCHEMA_V5: &str = "
    CREATE TABLE install_scripts (
        app_id   INTEGER NOT NULL REFERENCES installed_apps (app_id) ON DELETE CASCADE,
        depot_id INTEGER NOT NULL,
        path     TEXT NOT NULL,
        size     INTEGER NOT NULL,
        sha1     BLOB NOT NULL,
        PRIMARY KEY (app_id, depot_id, path)
    );
    CREATE TABLE script_values (
        app_id  INTEGER NOT NULL REFERENCES installed_apps (app_id) ON DELETE CASCADE,
        machine INTEGER NOT NULL,
        key     TEXT NOT NULL,
        name    TEXT
    );
";

/// The platform an app's depots were chosen for (`compat-tools.md` §4):
/// `windows` for an app run by Proton on Linux. Empty is the host's, which
/// every install before this column had.
const SCHEMA_V6: &str = "
    ALTER TABLE installed_apps ADD COLUMN platform TEXT NOT NULL DEFAULT '';
";

const SCHEMA_V7: &str = "
    ALTER TABLE jobs ADD COLUMN remove_compat_data INTEGER NOT NULL DEFAULT 0;
";

/// The persona state a user chose (`pics.md` §8). The numbers are stored
/// and must not change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PersonaState {
    #[default]
    Online,
    Invisible,
    Offline,
}

impl PersonaState {
    fn to_db(self) -> i64 {
        match self {
            Self::Online => 1,
            Self::Invisible => 2,
            Self::Offline => 3,
        }
    }

    fn from_db(n: i64) -> Option<Self> {
        match n {
            1 => Some(Self::Online),
            2 => Some(Self::Invisible),
            3 => Some(Self::Offline),
            _ => None,
        }
    }
}

/// A remembered account: one whose refresh token is stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub steam_id: SteamId,
    pub account_name: String,
    pub persona_name: String,
}

pub struct Db {
    conn: Connection,
}

impl Db {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        Self::init(conn).with_context(|| format!("preparing {}", path.display()))
    }

    #[cfg(test)]
    pub fn in_memory() -> Self {
        Self::init(Connection::open_in_memory().unwrap()).unwrap()
    }

    fn init(conn: Connection) -> anyhow::Result<Self> {
        // An in-memory database answers "memory"; either answer is fine.
        conn.pragma_update_and_check(None, "journal_mode", "WAL", |_| Ok(()))?;
        conn.pragma_update(None, "foreign_keys", true)?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        match version {
            0..=5 => {
                let tx = conn.unchecked_transaction()?;
                for (from, schema) in [
                    (0, SCHEMA_V1),
                    (1, SCHEMA_V2),
                    (2, SCHEMA_V3),
                    (3, SCHEMA_V4),
                    (4, SCHEMA_V5),
                    (5, SCHEMA_V6),
                    (6, SCHEMA_V7),
                ] {
                    if version <= from {
                        tx.execute_batch(schema)?;
                    }
                }
                tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
                tx.commit()?;
            }
            SCHEMA_VERSION => {}
            v => anyhow::bail!(
                "the database has schema version {v}, newer than this ossteamd ({SCHEMA_VERSION})"
            ),
        }
        Ok(Self { conn })
    }

    pub fn accounts(&self) -> anyhow::Result<Vec<Account>> {
        let mut stmt = self.conn.prepare(
            "SELECT steam_id, account_name, persona_name FROM accounts ORDER BY account_name",
        )?;
        let rows = stmt.query_map([], row_to_account)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn account(&self, steam_id: SteamId) -> anyhow::Result<Option<Account>> {
        Ok(self
            .conn
            .query_row(
                "SELECT steam_id, account_name, persona_name FROM accounts WHERE steam_id = ?1",
                params![steam_id.0 as i64],
                row_to_account,
            )
            .optional()?)
    }

    /// Records an account as remembered. An account name belongs to one
    /// Steam id, so a row that held the same name under another id goes.
    pub fn remember(&mut self, steam_id: SteamId, account_name: &str) -> anyhow::Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM accounts WHERE account_name = ?1 AND steam_id != ?2",
            params![account_name, steam_id.0 as i64],
        )?;
        tx.execute(
            "INSERT INTO accounts (steam_id, account_name) VALUES (?1, ?2)
             ON CONFLICT (steam_id) DO UPDATE SET account_name = excluded.account_name",
            params![steam_id.0 as i64, account_name],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn set_persona_name(&self, steam_id: SteamId, name: &str) -> anyhow::Result<()> {
        self.conn.execute(
            "UPDATE accounts SET persona_name = ?2 WHERE steam_id = ?1",
            params![steam_id.0 as i64, name],
        )?;
        Ok(())
    }

    /// The persona state chosen for an account; online when none was.
    pub fn persona_state(&self, steam_id: SteamId) -> anyhow::Result<PersonaState> {
        let n: Option<i64> = self
            .conn
            .query_row(
                "SELECT persona_state FROM account_settings WHERE steam_id = ?1",
                params![steam_id.0 as i64],
                |r| r.get(0),
            )
            .optional()?;
        Ok(n.and_then(PersonaState::from_db).unwrap_or_default())
    }

    pub fn set_persona_state(&self, steam_id: SteamId, state: PersonaState) -> anyhow::Result<()> {
        self.conn.execute(
            "INSERT INTO account_settings (steam_id, persona_state) VALUES (?1, ?2)
             ON CONFLICT (steam_id) DO UPDATE SET persona_state = excluded.persona_state",
            params![steam_id.0 as i64, state.to_db()],
        )?;
        Ok(())
    }

    pub fn forget(&self, steam_id: SteamId) -> anyhow::Result<()> {
        self.conn.execute(
            "DELETE FROM accounts WHERE steam_id = ?1",
            params![steam_id.0 as i64],
        )?;
        Ok(())
    }

    /// The account to resume when the daemon starts.
    pub fn active_account(&self) -> anyhow::Result<Option<SteamId>> {
        Ok(self
            .meta("active_account")?
            .and_then(|v| v.parse().ok())
            .map(SteamId))
    }

    pub fn set_active_account(&self, steam_id: Option<SteamId>) -> anyhow::Result<()> {
        self.set_meta("active_account", steam_id.map(|s| s.0.to_string()))
    }

    /// The cell id the last logon reported (`cm.md` §1), or 0.
    pub fn cell_id(&self) -> anyhow::Result<u32> {
        Ok(self
            .meta("cell_id")?
            .and_then(|v| v.parse().ok())
            .unwrap_or(0))
    }

    pub fn set_cell_id(&self, cell_id: u32) -> anyhow::Result<()> {
        self.set_meta("cell_id", Some(cell_id.to_string()))
    }

    /// Every stored setting override, as (key, value) rows.
    pub fn setting_overrides(&self) -> anyhow::Result<Vec<(String, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT key, value FROM settings ORDER BY key")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Stores an override, or with `None` removes it.
    pub fn set_setting_override(&self, key: &str, value: Option<&str>) -> anyhow::Result<()> {
        match value {
            Some(v) => self.conn.execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                params![key, v],
            )?,
            None => self
                .conn
                .execute("DELETE FROM settings WHERE key = ?1", [key])?,
        };
        Ok(())
    }

    fn meta(&self, key: &str) -> anyhow::Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
            .optional()?)
    }

    fn set_meta(&self, key: &str, value: Option<String>) -> anyhow::Result<()> {
        match value {
            Some(v) => self.conn.execute(
                "INSERT INTO meta (key, value) VALUES (?1, ?2)
                 ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                params![key, v],
            )?,
            None => self
                .conn
                .execute("DELETE FROM meta WHERE key = ?1", [key])?,
        };
        Ok(())
    }
}

/// A library folder (`ipc.md` §5.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folder {
    /// The id in its `library.toml`.
    pub id: String,
    pub path: PathBuf,
    pub is_default: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledDepot {
    pub depot: DepotId,
    pub manifest: ManifestId,
    /// For a depot installed into another app's directory: that app.
    pub shared_from: Option<AppId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledApp {
    pub app_id: AppId,
    pub folder_id: String,
    pub install_dir: String,
    pub branch: String,
    pub build_id: u32,
    pub size: u64,
    pub update_available: bool,
    /// What the depots were chosen for: `windows`, `macos` or `linux`.
    pub platform: String,
    pub depots: Vec<InstalledDepot>,
    pub scripts: Vec<ScriptFile>,
    /// What the scripts wrote, for verify and uninstall to remove.
    pub script_values: Vec<ValueRef>,
}

/// A job as stored; its journal is read separately.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobRow {
    pub job_id: u64,
    pub steam_id: SteamId,
    pub app_id: AppId,
    pub kind: v1::JobKind,
    pub state: v1::JobState,
    pub priority: i32,
    pub folder_id: String,
    pub branch: String,
    /// Unix time.
    pub created: u64,
    /// The error a failed job failed with: its IPC code and message.
    pub error: Option<(v1::ErrorCode, String)>,
    /// An uninstall that deletes the app's prefix too.
    pub remove_compat_data: bool,
}

impl Db {
    pub fn folders(&self) -> anyhow::Result<Vec<Folder>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, path, is_default FROM library_folders ORDER BY is_default DESC, rowid",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Folder {
                id: r.get(0)?,
                path: PathBuf::from(r.get::<_, String>(1)?),
                is_default: r.get(2)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn add_folder(&self, f: &Folder) -> anyhow::Result<()> {
        let path = f
            .path
            .to_str()
            .context("library folder paths must be valid Unicode")?;
        self.conn.execute(
            "INSERT INTO library_folders (id, path, is_default) VALUES (?1, ?2, ?3)",
            params![f.id, path, f.is_default],
        )?;
        Ok(())
    }

    pub fn remove_folder(&self, id: &str) -> anyhow::Result<()> {
        self.conn
            .execute("DELETE FROM library_folders WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn installed_apps(&self) -> anyhow::Result<Vec<InstalledApp>> {
        let mut stmt = self.conn.prepare(
            "SELECT app_id, folder_id, install_dir, branch, build_id, size, update_available,
                    platform
             FROM installed_apps ORDER BY app_id",
        )?;
        let mut apps: Vec<InstalledApp> = stmt
            .query_map([], |r| {
                Ok(InstalledApp {
                    app_id: AppId(r.get(0)?),
                    folder_id: r.get(1)?,
                    install_dir: r.get(2)?,
                    branch: r.get(3)?,
                    build_id: r.get(4)?,
                    size: r.get::<_, i64>(5)? as u64,
                    update_available: r.get(6)?,
                    platform: r.get(7)?,
                    depots: Vec::new(),
                    scripts: Vec::new(),
                    script_values: Vec::new(),
                })
            })?
            .collect::<Result<_, _>>()?;
        let mut stmt = self.conn.prepare(
            "SELECT app_id, depot_id, manifest_id, shared_from FROM installed_depots
             ORDER BY app_id, depot_id",
        )?;
        let depots = stmt.query_map([], |r| {
            Ok((
                AppId(r.get(0)?),
                InstalledDepot {
                    depot: DepotId(r.get(1)?),
                    manifest: ManifestId(r.get::<_, i64>(2)? as u64),
                    shared_from: Some(AppId(r.get(3)?)).filter(|a| a.0 != 0),
                },
            ))
        })?;
        for d in depots {
            let (app, depot) = d?;
            if let Some(a) = apps.iter_mut().find(|a| a.app_id == app) {
                a.depots.push(depot);
            }
        }
        let mut stmt = self.conn.prepare(
            "SELECT app_id, depot_id, path, size, sha1 FROM install_scripts
             ORDER BY app_id, depot_id, path",
        )?;
        let scripts = stmt.query_map([], |r| {
            Ok((
                AppId(r.get(0)?),
                ScriptFile {
                    depot: DepotId(r.get(1)?),
                    path: r.get(2)?,
                    size: r.get::<_, i64>(3)? as u64,
                    sha1: r.get(4)?,
                },
            ))
        })?;
        for s in scripts {
            let (app, script) = s?;
            if let Some(a) = apps.iter_mut().find(|a| a.app_id == app) {
                a.scripts.push(script);
            }
        }
        let mut stmt = self
            .conn
            .prepare("SELECT app_id, machine, key, name FROM script_values ORDER BY rowid")?;
        let values = stmt.query_map([], |r| {
            Ok((
                AppId(r.get(0)?),
                ValueRef {
                    key: Key {
                        machine: r.get(1)?,
                        path: r.get(2)?,
                    },
                    name: r.get(3)?,
                },
            ))
        })?;
        for v in values {
            let (app, value) = v?;
            if let Some(a) = apps.iter_mut().find(|a| a.app_id == app) {
                a.script_values.push(value);
            }
        }
        Ok(apps)
    }

    /// Records an app as installed, replacing what was recorded for it.
    pub fn set_installed(&mut self, app: &InstalledApp) -> anyhow::Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM installed_apps WHERE app_id = ?1",
            [app.app_id.0],
        )?;
        tx.execute(
            "INSERT INTO installed_apps
                 (app_id, folder_id, install_dir, branch, build_id, size, update_available,
                  platform)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                app.app_id.0,
                app.folder_id,
                app.install_dir,
                app.branch,
                app.build_id,
                app.size as i64,
                app.update_available,
                app.platform
            ],
        )?;
        for d in &app.depots {
            tx.execute(
                "INSERT INTO installed_depots (app_id, depot_id, manifest_id, shared_from)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    app.app_id.0,
                    d.depot.0,
                    d.manifest.0 as i64,
                    d.shared_from.map_or(0, |a| a.0)
                ],
            )?;
        }
        for f in &app.scripts {
            tx.execute(
                "INSERT INTO install_scripts (app_id, depot_id, path, size, sha1)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![app.app_id.0, f.depot.0, f.path, f.size as i64, f.sha1],
            )?;
        }
        for v in &app.script_values {
            tx.execute(
                "INSERT INTO script_values (app_id, machine, key, name) VALUES (?1, ?2, ?3, ?4)",
                params![app.app_id.0, v.key.machine, v.key.path, v.name],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn set_update_available(&self, app: AppId, available: bool) -> anyhow::Result<()> {
        self.conn.execute(
            "UPDATE installed_apps SET update_available = ?2 WHERE app_id = ?1",
            params![app.0, available],
        )?;
        Ok(())
    }

    pub fn remove_installed(&self, app: AppId) -> anyhow::Result<()> {
        self.conn
            .execute("DELETE FROM installed_apps WHERE app_id = ?1", [app.0])?;
        Ok(())
    }

    pub fn jobs(&self) -> anyhow::Result<Vec<JobRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT job_id, steam_id, app_id, kind, state, priority, folder_id, branch,
                    created, error_code, error_message, remove_compat_data
             FROM jobs ORDER BY job_id",
        )?;
        let rows = stmt.query_map([], |r| {
            let code: i32 = r.get(9)?;
            Ok(JobRow {
                job_id: r.get::<_, i64>(0)? as u64,
                steam_id: SteamId(r.get::<_, i64>(1)? as u64),
                app_id: AppId(r.get(2)?),
                kind: v1::JobKind::try_from(r.get::<_, i32>(3)?).unwrap_or_default(),
                state: v1::JobState::try_from(r.get::<_, i32>(4)?).unwrap_or_default(),
                priority: r.get(5)?,
                folder_id: r.get(6)?,
                branch: r.get(7)?,
                created: r.get::<_, i64>(8)? as u64,
                error: (code != 0)
                    .then(|| {
                        Ok::<_, rusqlite::Error>((
                            v1::ErrorCode::try_from(code).unwrap_or_default(),
                            r.get(10)?,
                        ))
                    })
                    .transpose()?,
                remove_compat_data: r.get(11)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Stores a new job; `job_id` 0 lets the database choose it. Returns
    /// the id.
    pub fn add_job(&self, j: &JobRow) -> anyhow::Result<u64> {
        self.conn.execute(
            "INSERT INTO jobs (steam_id, app_id, kind, state, priority, folder_id, branch, created,
                               remove_compat_data)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                j.steam_id.0 as i64,
                j.app_id.0,
                j.kind as i32,
                j.state as i32,
                j.priority,
                j.folder_id,
                j.branch,
                j.created as i64,
                j.remove_compat_data
            ],
        )?;
        Ok(self.conn.last_insert_rowid() as u64)
    }

    /// Stores a job's state, priority and error.
    pub fn update_job(&self, j: &JobRow) -> anyhow::Result<()> {
        let (code, message) = j
            .error
            .as_ref()
            .map_or((0, ""), |(c, m)| (*c as i32, m.as_str()));
        self.conn.execute(
            "UPDATE jobs SET state = ?2, priority = ?3, error_code = ?4, error_message = ?5
             WHERE job_id = ?1",
            params![j.job_id as i64, j.state as i32, j.priority, code, message],
        )?;
        Ok(())
    }

    pub fn job_journal(&self, job_id: u64) -> anyhow::Result<Option<Vec<u8>>> {
        Ok(self
            .conn
            .query_row(
                "SELECT journal FROM jobs WHERE job_id = ?1",
                [job_id as i64],
                |r| r.get(0),
            )
            .optional()?
            .flatten())
    }

    pub fn set_job_journal(&self, job_id: u64, journal: Option<&[u8]>) -> anyhow::Result<()> {
        self.conn.execute(
            "UPDATE jobs SET journal = ?2 WHERE job_id = ?1",
            params![job_id as i64, journal],
        )?;
        Ok(())
    }

    /// Deletes jobs that are done or cancelled; failed jobs stay until they
    /// are resumed or cancelled (`ipc.md` §5.6).
    pub fn forget_finished_jobs(&self) -> anyhow::Result<()> {
        self.conn.execute(
            "DELETE FROM jobs WHERE state IN (?1, ?2)",
            params![v1::JobState::Done as i32, v1::JobState::Cancelled as i32],
        )?;
        Ok(())
    }
}

fn row_to_account(r: &rusqlite::Row<'_>) -> rusqlite::Result<Account> {
    Ok(Account {
        steam_id: SteamId(r.get::<_, i64>(0)? as u64),
        account_name: r.get(1)?,
        persona_name: r.get(2)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remembers_renames_and_forgets() {
        let mut db = Db::in_memory();
        let a = SteamId::individual(1);
        let b = SteamId::individual(2);
        db.remember(a, "alice").unwrap();
        db.set_persona_name(a, "Alice").unwrap();
        db.remember(b, "bob").unwrap();
        assert_eq!(db.accounts().unwrap().len(), 2);

        // Remembering again keeps the persona name.
        db.remember(a, "alice").unwrap();
        assert_eq!(db.account(a).unwrap().unwrap().persona_name, "Alice");

        // A name moves to the Steam id that now holds it.
        db.remember(b, "alice").unwrap();
        let all = db.accounts().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].steam_id, b);

        db.forget(b).unwrap();
        assert!(db.accounts().unwrap().is_empty());
    }

    #[test]
    fn meta_values() {
        let db = Db::in_memory();
        assert_eq!(db.active_account().unwrap(), None);
        assert_eq!(db.cell_id().unwrap(), 0);
        // Steam ids use all 64 bits; the INTEGER column must not lose any.
        let id = SteamId(u64::MAX - 1);
        db.set_active_account(Some(id)).unwrap();
        db.set_cell_id(4).unwrap();
        assert_eq!(db.active_account().unwrap(), Some(id));
        assert_eq!(db.cell_id().unwrap(), 4);
        db.set_active_account(None).unwrap();
        assert_eq!(db.active_account().unwrap(), None);
    }

    #[test]
    fn high_steam_ids_round_trip_in_rows() {
        let mut db = Db::in_memory();
        let id = SteamId(0xffff_0000_0000_0001);
        db.remember(id, "x").unwrap();
        assert_eq!(db.accounts().unwrap()[0].steam_id, id);
    }

    #[test]
    fn persona_state_defaults_to_online() {
        let db = Db::in_memory();
        let a = SteamId::individual(1);
        assert_eq!(db.persona_state(a).unwrap(), PersonaState::Online);
        db.set_persona_state(a, PersonaState::Invisible).unwrap();
        db.set_persona_state(a, PersonaState::Offline).unwrap();
        assert_eq!(db.persona_state(a).unwrap(), PersonaState::Offline);
    }

    #[test]
    fn upgrades_a_version_1_database() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA_V1).unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
        conn.execute(
            "INSERT INTO accounts (steam_id, account_name) VALUES (1, 'alice')",
            [],
        )
        .unwrap();
        let db = Db::init(conn).unwrap();
        assert_eq!(db.accounts().unwrap().len(), 1);
        db.set_persona_state(SteamId(1), PersonaState::Invisible)
            .unwrap();
        db.set_setting_override("k", Some("v")).unwrap();
        let v: i64 = db
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(v, SCHEMA_VERSION);
    }

    #[test]
    fn setting_overrides() {
        let db = Db::in_memory();
        assert!(db.setting_overrides().unwrap().is_empty());
        db.set_setting_override("b.x", Some("true")).unwrap();
        db.set_setting_override("a.y", Some("1")).unwrap();
        db.set_setting_override("b.x", Some("false")).unwrap();
        assert_eq!(
            db.setting_overrides().unwrap(),
            [("a.y".into(), "1".into()), ("b.x".into(), "false".into())]
        );
        db.set_setting_override("a.y", None).unwrap();
        db.set_setting_override("missing", None).unwrap();
        assert_eq!(db.setting_overrides().unwrap().len(), 1);
    }

    #[test]
    fn folders_installs_and_jobs() {
        let mut db = Db::in_memory();
        let lib = Folder {
            id: "ab".into(),
            path: PathBuf::from("/lib"),
            is_default: true,
        };
        db.add_folder(&lib).unwrap();
        assert!(db.add_folder(&lib).is_err(), "ids are unique");
        assert_eq!(db.folders().unwrap(), [lib]);

        let app = InstalledApp {
            app_id: AppId(480),
            folder_id: "ab".into(),
            install_dir: "Spacewar".into(),
            branch: "public".into(),
            build_id: 7,
            size: 1 << 40,
            update_available: false,
            platform: "linux".into(),
            depots: vec![InstalledDepot {
                depot: DepotId(481),
                manifest: ManifestId(u64::MAX - 1),
                shared_from: None,
            }],
            scripts: vec![ScriptFile {
                depot: DepotId(481),
                path: "_CommonRedist\\x.vdf".into(),
                size: 10,
                sha1: vec![7; 20],
            }],
            script_values: vec![
                ValueRef {
                    key: Key {
                        machine: true,
                        path: "Software\\Example".into(),
                    },
                    name: None,
                },
                ValueRef {
                    key: Key {
                        machine: false,
                        path: "Software\\Example".into(),
                    },
                    name: Some("Path".into()),
                },
            ],
        };
        db.set_installed(&app).unwrap();
        db.set_installed(&app).unwrap();
        db.set_update_available(AppId(480), true).unwrap();
        let got = db.installed_apps().unwrap();
        assert_eq!(got.len(), 1);
        assert!(got[0].update_available);
        assert_eq!(got[0].depots, app.depots);
        assert_eq!(got[0].scripts, app.scripts);
        assert_eq!(got[0].script_values, app.script_values);
        assert!(db.remove_folder("ab").is_err(), "an app is installed there");
        db.remove_installed(AppId(480)).unwrap();
        assert!(db.installed_apps().unwrap().is_empty());

        let mut job = JobRow {
            job_id: 0,
            steam_id: SteamId(u64::MAX),
            app_id: AppId(480),
            kind: v1::JobKind::Install,
            state: v1::JobState::Queued,
            priority: -3,
            folder_id: "ab".into(),
            branch: "public".into(),
            created: 1_700_000_000,
            error: None,
            remove_compat_data: true,
        };
        job.job_id = db.add_job(&job).unwrap();
        db.set_job_journal(job.job_id, Some(b"OSJ1")).unwrap();
        job.state = v1::JobState::Failed;
        job.error = Some((v1::ErrorCode::Network, "timed out".into()));
        db.update_job(&job).unwrap();
        assert_eq!(db.jobs().unwrap(), [job.clone()]);
        assert_eq!(
            db.job_journal(job.job_id).unwrap().as_deref(),
            Some(&b"OSJ1"[..])
        );
        db.forget_finished_jobs().unwrap();
        assert_eq!(db.jobs().unwrap().len(), 1, "failed jobs stay");
        job.state = v1::JobState::Done;
        db.update_job(&job).unwrap();
        db.forget_finished_jobs().unwrap();
        assert!(db.jobs().unwrap().is_empty());
    }

    #[test]
    fn refuses_a_newer_schema() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "user_version", 99).unwrap();
        assert!(Db::init(conn).is_err());
    }
}
