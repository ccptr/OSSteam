// SPDX-License-Identifier: GPL-2.0-or-later
//! The daemon state database (SPEC D-6). It holds no secrets: tokens live
//! in the keyring (`secrets`).

use std::path::Path;

use anyhow::Context;
use ossteam_types::SteamId;
use rusqlite::{Connection, OptionalExtension, params};

pub const FILE_NAME: &str = "state.db";

/// The schema version this build writes, kept in `PRAGMA user_version`.
const SCHEMA_VERSION: i64 = 1;

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
            0 => {
                conn.execute_batch(SCHEMA_V1)?;
                conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
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
    fn refuses_a_newer_schema() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "user_version", 99).unwrap();
        assert!(Db::init(conn).is_err());
    }
}
