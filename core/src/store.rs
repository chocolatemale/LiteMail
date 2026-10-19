//! Local database (SQLite) and raw message files.
//!
//! Layout under the data directory:
//!   mail.db                          accounts, folders, message index, full-text index, op queue
//!   mail/<account>/<folder>/<uidvalidity>-<uid>.eml   raw RFC 5322 messages

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{Connection, OptionalExtension, params, params_from_iter, types::Value};

use crate::error::{CoreError, Result};
use crate::mime::Parsed;
use crate::types::*;

const SCHEMA_VERSION: i32 = 4;

pub struct Store {
    conn: Mutex<Connection>,
    pub root: PathBuf,
}

#[derive(Debug, Clone)]
pub struct FolderRow {
    pub id: i64,
    pub account_id: i64,
    pub name: String,
    pub role: FolderRole,
    pub sync_enabled: bool,
    pub uidvalidity: Option<u32>,
    pub highest_modseq: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct ServerFolder {
    pub name: String,
    pub delimiter: Option<String>,
    pub role: FolderRole,
    pub selectable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Flags {
    pub seen: bool,
    pub flagged: bool,
    pub answered: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct LocalMsg {
    pub id: i64,
    pub flags: Flags,
}

pub struct NewMessage<'a> {
    pub account_id: i64,
    pub folder_id: i64,
    pub uid: u32,
    pub uidvalidity: u32,
    pub flags: Flags,
    pub internal_date: i64,
    pub size: u64,
    pub parsed: &'a Parsed,
    pub raw: &'a [u8],
}

#[derive(Debug, Clone)]
pub struct MessageRow {
    pub id: i64,
    pub account_id: i64,
    pub folder_id: i64,
    pub uid: u32,
    pub seen: bool,
    pub flagged: bool,
    pub date: i64,
    pub raw_path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalFlag {
    Seen,
    Flagged,
    Answered,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpKind {
    AddFlag(LocalFlag),
    RemoveFlag(LocalFlag),
    Move { target_folder_id: i64 },
    Delete,
}

impl OpKind {
    fn encode(&self) -> (String, Option<i64>) {
        let flag = |f: &LocalFlag| match f {
            LocalFlag::Seen => "seen",
            LocalFlag::Flagged => "flagged",
            LocalFlag::Answered => "answered",
        };
        match self {
            OpKind::AddFlag(f) => (format!("+{}", flag(f)), None),
            OpKind::RemoveFlag(f) => (format!("-{}", flag(f)), None),
            OpKind::Move { target_folder_id } => ("move".into(), Some(*target_folder_id)),
            OpKind::Delete => ("delete".into(), None),
        }
    }

    fn decode(kind: &str, target: Option<i64>) -> Option<OpKind> {
        let flag = |s: &str| match s {
            "seen" => Some(LocalFlag::Seen),
            "flagged" => Some(LocalFlag::Flagged),
            "answered" => Some(LocalFlag::Answered),
            _ => None,
        };
        match kind {
            "move" => Some(OpKind::Move { target_folder_id: target? }),
            "delete" => Some(OpKind::Delete),
            k if k.starts_with('+') => flag(&k[1..]).map(OpKind::AddFlag),
            k if k.starts_with('-') => flag(&k[1..]).map(OpKind::RemoveFlag),
            _ => None,
        }
    }

    pub fn hides_message(&self) -> bool {
        matches!(self, OpKind::Move { .. } | OpKind::Delete)
    }
}

#[derive(Debug, Clone)]
pub struct Op {
    pub id: i64,
    pub folder_id: i64,
    pub uid: u32,
    pub kind: OpKind,
    pub attempts: u32,
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

impl Store {
    pub fn open(root: &Path) -> Result<Store> {
        std::fs::create_dir_all(root.join("mail"))?;
        let conn = Connection::open(root.join("mail.db"))?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;",
        )?;
        let store = Store { conn: Mutex::new(conn), root: root.to_path_buf() };
        store.migrate()?;
        Ok(store)
    }

    fn db(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn migrate(&self) -> Result<()> {
        let conn = self.db();
        let version: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version >= SCHEMA_VERSION {
            return Ok(());
        }
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS accounts(
                id INTEGER PRIMARY KEY,
                email TEXT NOT NULL,
                display_name TEXT NOT NULL,
                provider TEXT NOT NULL,
                imap_host TEXT NOT NULL, imap_port INTEGER NOT NULL, imap_security TEXT NOT NULL,
                smtp_host TEXT NOT NULL, smtp_port INTEGER NOT NULL, smtp_security TEXT NOT NULL,
                username TEXT NOT NULL,
                sync_days INTEGER NOT NULL DEFAULT 30,
                sync_interval INTEGER NOT NULL DEFAULT 5,
                server_saves_sent INTEGER NOT NULL DEFAULT 0,
                signature TEXT NOT NULL DEFAULT '',
                notify INTEGER NOT NULL DEFAULT 1,
                allow_invalid_certs INTEGER NOT NULL DEFAULT 0,
                initial_sync_done INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS folders(
                id INTEGER PRIMARY KEY,
                account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
                name TEXT NOT NULL,
                delimiter TEXT,
                role TEXT NOT NULL,
                sync_enabled INTEGER NOT NULL,
                selectable INTEGER NOT NULL,
                uidvalidity INTEGER,
                highest_modseq INTEGER,
                last_sync INTEGER NOT NULL DEFAULT 0,
                UNIQUE(account_id, name)
            );
            CREATE TABLE IF NOT EXISTS messages(
                id INTEGER PRIMARY KEY,
                account_id INTEGER NOT NULL,
                folder_id INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
                uid INTEGER NOT NULL,
                message_id TEXT,
                subject TEXT NOT NULL,
                from_name TEXT NOT NULL,
                from_addr TEXT NOT NULL,
                to_line TEXT NOT NULL,
                cc_line TEXT NOT NULL,
                date INTEGER NOT NULL,
                size INTEGER NOT NULL,
                seen INTEGER NOT NULL,
                flagged INTEGER NOT NULL,
                answered INTEGER NOT NULL,
                has_attachments INTEGER NOT NULL,
                snippet TEXT NOT NULL,
                raw_path TEXT NOT NULL,
                UNIQUE(folder_id, uid)
            );
            CREATE INDEX IF NOT EXISTS messages_folder_date ON messages(folder_id, date DESC, id DESC);
            CREATE INDEX IF NOT EXISTS messages_date ON messages(date DESC, id DESC);
            CREATE INDEX IF NOT EXISTS messages_msgid ON messages(account_id, message_id);
            CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(subject, people, body, tokenize='trigram');
            CREATE TRIGGER IF NOT EXISTS messages_ad AFTER DELETE ON messages BEGIN
                DELETE FROM messages_fts WHERE rowid = old.id;
            END;
            CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS aliases(
                account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
                name TEXT NOT NULL,
                address TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                UNIQUE(account_id, address)
            );
            CREATE TABLE IF NOT EXISTS ops(
                id INTEGER PRIMARY KEY,
                account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
                folder_id INTEGER NOT NULL,
                uid INTEGER NOT NULL,
                kind TEXT NOT NULL,
                target_folder_id INTEGER,
                attempts INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL
            );
            "#,
        )?;
        conn.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION}"))?;
        Ok(())
    }

    /// Send-from addresses the user added for an account, oldest first.
    pub fn aliases(&self, account_id: i64) -> Result<Vec<EmailAddress>> {
        let db = self.db();
        let mut st = db.prepare("SELECT name, address FROM aliases WHERE account_id = ?1 ORDER BY created_at, rowid")?;
        let rows = st.query_map([account_id], |r| Ok(EmailAddress { name: r.get(0)?, address: r.get(1)? }))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn add_alias(&self, account_id: i64, name: &str, address: &str) -> Result<()> {
        self.db().execute(
            "INSERT INTO aliases(account_id, name, address, created_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(account_id, address) DO UPDATE SET name = excluded.name",
            params![account_id, name, address, chrono::Utc::now().timestamp()],
        )?;
        Ok(())
    }

    pub fn remove_alias(&self, account_id: i64, address: &str) -> Result<()> {
        self.db().execute("DELETE FROM aliases WHERE account_id = ?1 AND lower(address) = lower(?2)", params![account_id, address])?;
        Ok(())
    }

    /// Addresses the account has sent from (its Sent folder), most recent first. The server only
    /// accepts senders the account owns, so these are known to work.
    pub fn sent_from_addresses(&self, account_id: i64) -> Result<Vec<EmailAddress>> {
        let db = self.db();
        let mut st = db.prepare(
            "SELECT m.from_name, m.from_addr FROM messages m JOIN folders f ON f.id = m.folder_id
             WHERE m.account_id = ?1 AND f.role = 'sent' AND m.from_addr != ''
             GROUP BY lower(m.from_addr) ORDER BY max(m.date) DESC LIMIT 20",
        )?;
        let rows = st.query_map([account_id], |r| Ok(EmailAddress { name: r.get(0)?, address: r.get(1)? }))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Random id of this data directory; namespaces keychain items so two data
    /// directories (e.g. a test profile) never share passwords.
    pub fn install_id(&self) -> Result<String> {
        let db = self.db();
        if let Some(v) = db.query_row("SELECT value FROM meta WHERE key = 'install_id'", [], |r| r.get::<_, String>(0)).optional()? {
            return Ok(v);
        }
        use std::hash::{BuildHasher, Hasher};
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_i64(chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default());
        h.write(self.root.to_string_lossy().as_bytes());
        let id = format!("{:012x}", h.finish() & 0xffff_ffff_ffff);
        db.execute("INSERT INTO meta(key, value) VALUES ('install_id', ?1)", [&id])?;
        Ok(id)
    }

    pub fn account_dir(&self, account_id: i64) -> PathBuf {
        self.root.join("mail").join(account_id.to_string())
    }

    pub fn folder_dir(&self, account_id: i64, folder_id: i64) -> PathBuf {
        self.account_dir(account_id).join(folder_id.to_string())
    }

    pub fn absolute(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    // ---------------------------------------------------------------- accounts

    pub fn insert_account(&self, s: &AccountSetup, server_saves_sent: bool) -> Result<i64> {
        let db = self.db();
        db.execute(
            "INSERT INTO accounts(email, display_name, provider, imap_host, imap_port, imap_security,
                smtp_host, smtp_port, smtp_security, username, sync_days, server_saves_sent,
                allow_invalid_certs, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                s.email.trim(),
                s.display_name.trim(),
                s.provider.as_str(),
                s.imap.host.trim(),
                s.imap.port,
                s.imap.security.as_str(),
                s.smtp.host.trim(),
                s.smtp.port,
                s.smtp.security.as_str(),
                s.username.trim(),
                s.sync_days,
                server_saves_sent,
                s.allow_invalid_certs,
                now()
            ],
        )?;
        Ok(db.last_insert_rowid())
    }

    fn row_to_account(r: &rusqlite::Row<'_>) -> rusqlite::Result<Account> {
        Ok(Account {
            id: r.get(0)?,
            email: r.get(1)?,
            display_name: r.get(2)?,
            provider: ProviderKind::parse(&r.get::<_, String>(3)?),
            imap: ServerConfig { host: r.get(4)?, port: r.get(5)?, security: Security::parse(&r.get::<_, String>(6)?) },
            smtp: ServerConfig { host: r.get(7)?, port: r.get(8)?, security: Security::parse(&r.get::<_, String>(9)?) },
            username: r.get(10)?,
            sync_days: r.get(11)?,
            sync_interval_minutes: r.get(12)?,
            server_saves_sent: r.get(13)?,
            signature: r.get(14)?,
            notify: r.get(15)?,
            allow_invalid_certs: r.get(16)?,
        })
    }

    const ACCOUNT_COLS: &'static str = "id, email, display_name, provider, imap_host, imap_port, imap_security,
        smtp_host, smtp_port, smtp_security, username, sync_days, sync_interval, server_saves_sent,
        signature, notify, allow_invalid_certs";

    pub fn accounts(&self) -> Result<Vec<Account>> {
        let db = self.db();
        let mut st = db.prepare(&format!("SELECT {} FROM accounts ORDER BY id", Self::ACCOUNT_COLS))?;
        let rows = st.query_map([], Self::row_to_account)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn account(&self, id: i64) -> Result<Account> {
        let db = self.db();
        Ok(db.query_row(
            &format!("SELECT {} FROM accounts WHERE id = ?1", Self::ACCOUNT_COLS),
            [id],
            Self::row_to_account,
        )?)
    }

    pub fn update_account_settings(&self, id: i64, s: &AccountSettings) -> Result<()> {
        let n = self.db().execute(
            "UPDATE accounts SET display_name = ?2, sync_days = ?3, sync_interval = ?4, signature = ?5, notify = ?6 WHERE id = ?1",
            params![id, s.display_name.trim(), s.sync_days, s.sync_interval_minutes.max(1), s.signature, s.notify],
        )?;
        if n == 0 { Err(CoreError::NotFound) } else { Ok(()) }
    }

    pub fn delete_account(&self, id: i64) -> Result<()> {
        {
            let db = self.db();
            db.execute("DELETE FROM messages WHERE account_id = ?1", [id])?;
            db.execute("DELETE FROM accounts WHERE id = ?1", [id])?;
        }
        let dir = self.account_dir(id);
        if dir.exists() {
            std::fs::remove_dir_all(dir)?;
        }
        Ok(())
    }

    pub fn initial_sync_done(&self, id: i64) -> bool {
        self.db()
            .query_row("SELECT initial_sync_done FROM accounts WHERE id = ?1", [id], |r| r.get(0))
            .unwrap_or(false)
    }

    pub fn set_initial_sync_done(&self, id: i64) -> Result<()> {
        self.db().execute("UPDATE accounts SET initial_sync_done = 1 WHERE id = ?1", [id])?;
        Ok(())
    }

    // ---------------------------------------------------------------- folders

    /// Mirrors the server's folder list. Returns ids of folders that disappeared.
    pub fn sync_folder_list(&self, account_id: i64, list: &[ServerFolder]) -> Result<Vec<i64>> {
        let mut db = self.db();
        let tx = db.transaction()?;
        let existing: HashMap<String, i64> = {
            let mut st = tx.prepare("SELECT name, id FROM folders WHERE account_id = ?1")?;
            st.query_map([account_id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?
        };
        let mut seen = HashSet::new();
        for f in list {
            seen.insert(f.name.clone());
            if let Some(id) = existing.get(&f.name) {
                tx.execute(
                    "UPDATE folders SET delimiter = ?2, role = ?3, selectable = ?4 WHERE id = ?1",
                    params![id, f.delimiter, f.role.as_str(), f.selectable],
                )?;
            } else {
                let enabled = f.selectable && !f.role.is_virtual();
                tx.execute(
                    "INSERT INTO folders(account_id, name, delimiter, role, sync_enabled, selectable) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![account_id, f.name, f.delimiter, f.role.as_str(), enabled, f.selectable],
                )?;
            }
        }
        let gone: Vec<i64> = existing.iter().filter(|(n, _)| !seen.contains(*n)).map(|(_, id)| *id).collect();
        for id in &gone {
            tx.execute("DELETE FROM messages WHERE folder_id = ?1", [id])?;
            tx.execute("DELETE FROM ops WHERE folder_id = ?1 OR target_folder_id = ?1", [id])?;
            tx.execute("DELETE FROM folders WHERE id = ?1", [id])?;
        }
        tx.commit()?;
        drop(db);
        for id in &gone {
            let dir = self.folder_dir(account_id, *id);
            if dir.exists() {
                let _ = std::fs::remove_dir_all(dir);
            }
        }
        Ok(gone)
    }

    pub fn folders(&self, account_id: i64) -> Result<Vec<Folder>> {
        let db = self.db();
        let counts: HashMap<i64, (u32, u32)> = {
            let mut st = db.prepare(
                "SELECT folder_id, SUM(seen = 0), COUNT(*) FROM messages WHERE account_id = ?1 GROUP BY folder_id",
            )?;
            st.query_map([account_id], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))))?
                .collect::<rusqlite::Result<_>>()?
        };
        let mut st = db.prepare(
            "SELECT id, name, delimiter, role, sync_enabled, last_sync FROM folders
             WHERE account_id = ?1 AND selectable = 1",
        )?;
        let mut out: Vec<Folder> = st
            .query_map([account_id], |r| {
                let id: i64 = r.get(0)?;
                let name: String = r.get(1)?;
                let delim: Option<String> = r.get(2)?;
                let role = FolderRole::parse(&r.get::<_, String>(3)?);
                let path = crate::utf7::decode(&name);
                let (display_name, depth) = match delim.as_deref().filter(|d| !d.is_empty()) {
                    Some(d) => {
                        let parts: Vec<&str> = path.split(d).collect();
                        (parts.last().unwrap_or(&"").to_string(), parts.len().saturating_sub(1) as u32)
                    }
                    None => (path.clone(), 0),
                };
                let (unread, total) = counts.get(&id).copied().unwrap_or((0, 0));
                Ok(Folder {
                    id,
                    account_id,
                    display_name,
                    path,
                    name,
                    depth: if role == FolderRole::Custom { depth } else { 0 },
                    role,
                    sync_enabled: r.get(4)?,
                    unread,
                    total,
                    last_sync: r.get(5)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        // Special folders get their familiar Chinese names whatever the server calls them;
        // nested folders whose parent isn't a mailbox show their full path.
        let paths: HashSet<String> = out.iter().map(|f| f.path.clone()).collect();
        for f in out.iter_mut() {
            if let Some(name) = special_name(f.role) {
                f.display_name = name.clone();
                f.path = name;
                f.depth = 0;
            } else if f.depth > 0 {
                let parent = f.path.rsplit_once(['/', '.']).map(|(p, _)| p.to_string()).unwrap_or_default();
                if !paths.contains(&parent) {
                    f.display_name = f.path.clone();
                    f.depth = 0;
                }
            }
        }
        out.sort_by(|a, b| {
            a.role.rank().cmp(&b.role.rank()).then_with(|| a.path.to_lowercase().cmp(&b.path.to_lowercase()))
        });
        Ok(out)
    }

    fn row_to_folder(r: &rusqlite::Row<'_>) -> rusqlite::Result<FolderRow> {
        Ok(FolderRow {
            id: r.get(0)?,
            account_id: r.get(1)?,
            name: r.get(2)?,
            role: FolderRole::parse(&r.get::<_, String>(3)?),
            sync_enabled: r.get(4)?,
            uidvalidity: r.get(5)?,
            highest_modseq: r.get::<_, Option<i64>>(6)?.map(|v| v as u64),
        })
    }

    const FOLDER_COLS: &'static str = "id, account_id, name, role, sync_enabled, uidvalidity, highest_modseq";

    pub fn folder(&self, id: i64) -> Result<FolderRow> {
        Ok(self.db().query_row(
            &format!("SELECT {} FROM folders WHERE id = ?1", Self::FOLDER_COLS),
            [id],
            Self::row_to_folder,
        )?)
    }

    pub fn folder_by_role(&self, account_id: i64, role: FolderRole) -> Result<Option<FolderRow>> {
        Ok(self
            .db()
            .query_row(
                &format!(
                    "SELECT {} FROM folders WHERE account_id = ?1 AND role = ?2 AND selectable = 1 ORDER BY id LIMIT 1",
                    Self::FOLDER_COLS
                ),
                params![account_id, role.as_str()],
                Self::row_to_folder,
            )
            .optional()?)
    }

    /// Folders to sync, inbox first, then sent, drafts and the rest.
    pub fn sync_folders(&self, account_id: i64) -> Result<Vec<FolderRow>> {
        let db = self.db();
        let mut st = db.prepare(&format!(
            "SELECT {} FROM folders WHERE account_id = ?1 AND sync_enabled = 1 AND selectable = 1",
            Self::FOLDER_COLS
        ))?;
        let mut v: Vec<FolderRow> = st.query_map([account_id], Self::row_to_folder)?.collect::<rusqlite::Result<_>>()?;
        v.sort_by_key(|f| (f.role.rank(), f.id));
        Ok(v)
    }

    pub fn set_folder_sync_enabled(&self, id: i64, enabled: bool) -> Result<()> {
        let f = self.folder(id)?;
        self.db().execute("UPDATE folders SET sync_enabled = ?2 WHERE id = ?1", params![id, enabled])?;
        if !enabled {
            self.wipe_folder(f.account_id, id)?;
        }
        Ok(())
    }

    pub fn set_folder_state(&self, id: i64, uidvalidity: Option<u32>, modseq: Option<u64>) -> Result<()> {
        self.db().execute(
            "UPDATE folders SET uidvalidity = ?2, highest_modseq = ?3, last_sync = ?4 WHERE id = ?1",
            params![id, uidvalidity, modseq.map(|m| m as i64), now()],
        )?;
        Ok(())
    }

    /// Drops every local copy in a folder (UIDVALIDITY changed, or sync turned off).
    pub fn wipe_folder(&self, account_id: i64, folder_id: i64) -> Result<()> {
        {
            let db = self.db();
            db.execute("DELETE FROM messages WHERE folder_id = ?1", [folder_id])?;
            db.execute(
                "UPDATE folders SET uidvalidity = NULL, highest_modseq = NULL WHERE id = ?1",
                [folder_id],
            )?;
        }
        let dir = self.folder_dir(account_id, folder_id);
        if dir.exists() {
            std::fs::remove_dir_all(dir)?;
        }
        Ok(())
    }

    // ---------------------------------------------------------------- messages

    pub fn local_uids(&self, folder_id: i64) -> Result<HashMap<u32, LocalMsg>> {
        let db = self.db();
        let mut st = db.prepare("SELECT uid, id, seen, flagged, answered FROM messages WHERE folder_id = ?1")?;
        let rows = st.query_map([folder_id], |r| {
            Ok((
                r.get::<_, u32>(0)?,
                LocalMsg { id: r.get(1)?, flags: Flags { seen: r.get(2)?, flagged: r.get(3)?, answered: r.get(4)? } },
            ))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Writes the raw file, then indexes it. Returns the local id.
    pub fn insert_message(&self, m: &NewMessage<'_>) -> Result<i64> {
        let dir = self.folder_dir(m.account_id, m.folder_id);
        std::fs::create_dir_all(&dir)?;
        let rel = format!("mail/{}/{}/{}-{}.eml", m.account_id, m.folder_id, m.uidvalidity, m.uid);
        let path = self.root.join(&rel);
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, m.raw)?;
        std::fs::rename(&tmp, &path)?;

        let p = m.parsed;
        let join = |v: &[EmailAddress]| {
            v.iter()
                .map(|a| if a.name.is_empty() { a.address.clone() } else { format!("{} <{}>", a.name, a.address) })
                .collect::<Vec<_>>()
                .join(", ")
        };
        let to_line = join(&p.to);
        let cc_line = join(&p.cc);
        let people = format!("{} {} {} {}", p.from.name, p.from.address, to_line, cc_line);

        let mut db = self.db();
        let tx = db.transaction()?;
        // A message re-fetched under the same uid replaces the old index entry.
        tx.execute("DELETE FROM messages WHERE folder_id = ?1 AND uid = ?2", params![m.folder_id, m.uid])?;
        tx.execute(
            "INSERT INTO messages(account_id, folder_id, uid, message_id, subject, from_name, from_addr,
                to_line, cc_line, date, size, seen, flagged, answered, has_attachments, snippet, raw_path)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
            params![
                m.account_id,
                m.folder_id,
                m.uid,
                p.message_id,
                p.subject,
                p.from.name,
                p.from.address,
                to_line,
                cc_line,
                m.internal_date,
                m.size as i64,
                m.flags.seen,
                m.flags.flagged,
                m.flags.answered,
                p.has_attachments,
                p.snippet,
                rel
            ],
        )?;
        let id = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO messages_fts(rowid, subject, people, body) VALUES (?1, ?2, ?3, ?4)",
            params![id, p.subject, people, p.search_text],
        )?;
        tx.commit()?;
        Ok(id)
    }

    /// Removes index rows and raw files for the given uids.
    pub fn delete_uids(&self, folder_id: i64, uids: &[u32]) -> Result<usize> {
        if uids.is_empty() {
            return Ok(0);
        }
        let mut paths = Vec::new();
        {
            let mut db = self.db();
            let tx = db.transaction()?;
            {
                let mut sel = tx.prepare("SELECT raw_path FROM messages WHERE folder_id = ?1 AND uid = ?2")?;
                let mut del = tx.prepare("DELETE FROM messages WHERE folder_id = ?1 AND uid = ?2")?;
                for uid in uids {
                    if let Some(p) = sel.query_row(params![folder_id, uid], |r| r.get::<_, String>(0)).optional()? {
                        paths.push(p);
                    }
                    del.execute(params![folder_id, uid])?;
                }
            }
            tx.commit()?;
        }
        for p in &paths {
            let _ = std::fs::remove_file(self.root.join(p));
        }
        Ok(paths.len())
    }

    /// Applies server flags. Returns how many rows changed.
    pub fn update_flags(&self, folder_id: i64, updates: &[(u32, Flags)]) -> Result<usize> {
        let mut db = self.db();
        let tx = db.transaction()?;
        let mut changed = 0;
        {
            let mut st = tx.prepare(
                "UPDATE messages SET seen = ?3, flagged = ?4, answered = ?5
                 WHERE folder_id = ?1 AND uid = ?2 AND (seen != ?3 OR flagged != ?4 OR answered != ?5)",
            )?;
            for (uid, f) in updates {
                changed += st.execute(params![folder_id, uid, f.seen, f.flagged, f.answered])?;
            }
        }
        tx.commit()?;
        Ok(changed)
    }

    pub fn message_row(&self, id: i64) -> Result<MessageRow> {
        Ok(self.db().query_row(
            "SELECT id, account_id, folder_id, uid, seen, flagged, raw_path, date FROM messages WHERE id = ?1",
            [id],
            |r| {
                Ok(MessageRow {
                    id: r.get(0)?,
                    account_id: r.get(1)?,
                    folder_id: r.get(2)?,
                    uid: r.get(3)?,
                    seen: r.get(4)?,
                    flagged: r.get(5)?,
                    date: r.get(7)?,
                    raw_path: self.root.join(r.get::<_, String>(6)?),
                })
            },
        )?)
    }

    pub fn read_raw(&self, id: i64) -> Result<(MessageRow, Vec<u8>)> {
        let row = self.message_row(id)?;
        let raw = std::fs::read(&row.raw_path)?;
        Ok((row, raw))
    }

    /// Sets a flag locally; returns the rows that actually changed.
    pub fn set_local_flag(&self, ids: &[i64], flag: LocalFlag, value: bool) -> Result<Vec<MessageRow>> {
        let col = match flag {
            LocalFlag::Seen => "seen",
            LocalFlag::Flagged => "flagged",
            LocalFlag::Answered => "answered",
        };
        let mut changed = Vec::new();
        for id in ids {
            let n = self.db().execute(
                &format!("UPDATE messages SET {col} = ?2 WHERE id = ?1 AND {col} != ?2"),
                params![id, value],
            )?;
            if n > 0 {
                changed.push(self.message_row(*id)?);
            }
        }
        Ok(changed)
    }

    /// Removes messages locally (moved or deleted by the user). Returns the removed rows.
    pub fn remove_local(&self, ids: &[i64]) -> Result<Vec<MessageRow>> {
        let mut out = Vec::new();
        for id in ids {
            let Ok(row) = self.message_row(*id) else { continue };
            self.db().execute("DELETE FROM messages WHERE id = ?1", [id])?;
            let _ = std::fs::remove_file(&row.raw_path);
            out.push(row);
        }
        Ok(out)
    }

    pub fn unread_inbox_total(&self) -> Result<u32> {
        Ok(self.db().query_row(
            "SELECT COUNT(*) FROM messages WHERE seen = 0 AND folder_id IN (SELECT id FROM folders WHERE role = 'inbox')",
            [],
            |r| r.get(0),
        )?)
    }

    pub fn storage(&self, account_id: Option<i64>) -> Result<StorageInfo> {
        let (sql, args): (&str, Vec<Value>) = match account_id {
            Some(id) => ("SELECT COUNT(*), COALESCE(SUM(size), 0) FROM messages WHERE account_id = ?1", vec![id.into()]),
            None => ("SELECT COUNT(*), COALESCE(SUM(size), 0) FROM messages", vec![]),
        };
        Ok(self.db().query_row(sql, params_from_iter(args), |r| {
            Ok(StorageInfo { messages: r.get::<_, i64>(0)? as u64, bytes: r.get::<_, i64>(1)? as u64 })
        })?)
    }

    pub fn query(&self, q: &MessageQuery) -> Result<Vec<MessageSummary>> {
        let mut wh: Vec<String> = Vec::new();
        let mut args: Vec<Value> = Vec::new();
        const NOT_TRASH: &str = "SELECT id FROM folders WHERE role NOT IN ('trash', 'junk')";
        const UNIQUE: &str = "(m.message_id IS NULL OR m.id = (SELECT MIN(d.id) FROM messages d
            WHERE d.account_id = m.account_id AND d.message_id = m.message_id
            AND d.folder_id IN (SELECT id FROM folders WHERE role NOT IN ('trash', 'junk'))))";
        match &q.mailbox {
            Mailbox::Folder { id } => {
                wh.push("m.folder_id = ?".into());
                args.push((*id).into());
            }
            Mailbox::AllInboxes => wh.push("m.folder_id IN (SELECT id FROM folders WHERE role = 'inbox')".into()),
            Mailbox::Unread => {
                wh.push("m.seen = 0 AND m.folder_id IN (SELECT id FROM folders WHERE role = 'inbox')".into())
            }
            // Gmail labels put one message in several folders: show it once.
            Mailbox::Flagged => wh.push(format!("m.flagged = 1 AND m.folder_id IN ({NOT_TRASH}) AND {UNIQUE}")),
            Mailbox::Everything => wh.push(format!("m.folder_id IN ({NOT_TRASH}) AND {UNIQUE}")),
        }
        if let Some(search) = q.search.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            let (sql, mut a) = search_clause(search);
            if !sql.is_empty() {
                wh.push(sql);
                args.append(&mut a);
            }
        }
        if let (Some(d), Some(id)) = (q.before_date, q.before_id) {
            wh.push("(m.date < ? OR (m.date = ? AND m.id < ?))".into());
            args.push(d.into());
            args.push(d.into());
            args.push(id.into());
        }
        let sql = format!(
            "SELECT m.id, m.account_id, m.folder_id, m.subject, m.from_name, m.from_addr, m.to_line, m.date,
                    m.snippet, m.seen, m.flagged, m.answered, m.has_attachments, m.size
             FROM messages m WHERE {} ORDER BY m.date DESC, m.id DESC LIMIT {}",
            if wh.is_empty() { "1".to_string() } else { wh.join(" AND ") },
            q.limit.clamp(1, 2000)
        );
        let db = self.db();
        let mut st = db.prepare(&sql)?;
        let rows = st.query_map(params_from_iter(args), |r| {
            Ok(MessageSummary {
                id: r.get(0)?,
                account_id: r.get(1)?,
                folder_id: r.get(2)?,
                subject: r.get(3)?,
                from_name: r.get(4)?,
                from_addr: r.get(5)?,
                to_line: r.get(6)?,
                date: r.get(7)?,
                snippet: r.get(8)?,
                seen: r.get(9)?,
                flagged: r.get(10)?,
                answered: r.get(11)?,
                has_attachments: r.get(12)?,
                size: r.get::<_, i64>(13)? as u64,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Counts for every smart mailbox: (all inboxes unread, unread total, flagged total).
    pub fn smart_counts(&self) -> Result<(u32, u32)> {
        let db = self.db();
        let unread: u32 = db.query_row(
            "SELECT COUNT(*) FROM messages WHERE seen = 0 AND folder_id IN (SELECT id FROM folders WHERE role = 'inbox')",
            [],
            |r| r.get(0),
        )?;
        let flagged: u32 = db.query_row(
            "SELECT COUNT(DISTINCT COALESCE(account_id || ':' || message_id, 'id:' || id)) FROM messages
             WHERE flagged = 1 AND folder_id IN (SELECT id FROM folders WHERE role NOT IN ('trash', 'junk'))",
            [],
            |r| r.get(0),
        )?;
        Ok((unread, flagged))
    }

    /// People to suggest while typing a recipient: everyone you wrote to (weighted) plus
    /// everyone who wrote to you, best matches first.
    pub fn contacts(&self, query: &str, limit: usize) -> Result<Vec<EmailAddress>> {
        let q = query.trim().to_lowercase();
        let db = self.db();
        let mut score: HashMap<String, (EmailAddress, i64)> = HashMap::new();
        let mut add = |name: &str, addr: &str, w: i64| {
            let key = addr.trim().to_lowercase();
            if key.is_empty() || !key.contains('@') || key.contains("noreply") || key.contains("no-reply") {
                return;
            }
            let e = score.entry(key).or_insert_with(|| (EmailAddress { name: String::new(), address: addr.trim().to_string() }, 0));
            if e.0.name.is_empty() && !name.trim().is_empty() {
                e.0.name = name.trim().to_string();
            }
            e.1 += w;
        };
        {
            let mut st = db.prepare(
                "SELECT to_line, cc_line FROM messages WHERE folder_id IN (SELECT id FROM folders WHERE role = 'sent')
                 ORDER BY date DESC LIMIT 3000",
            )?;
            let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
            for row in rows {
                let (to, cc) = row?;
                for part in to.split(", ").chain(cc.split(", ")) {
                    let (name, addr) = split_display(part);
                    add(&name, &addr, 5);
                }
            }
        }
        {
            let mut st = db.prepare("SELECT from_name, from_addr FROM messages ORDER BY date DESC LIMIT 5000")?;
            let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
            for row in rows {
                let (name, addr) = row?;
                add(&name, &addr, 1);
            }
        }
        let mut v: Vec<(EmailAddress, i64)> = score
            .into_values()
            .filter(|(a, _)| q.is_empty() || a.address.to_lowercase().contains(&q) || a.name.to_lowercase().contains(&q))
            .collect();
        v.sort_by(|a, b| {
            let pa = a.0.address.to_lowercase().starts_with(&q) || a.0.name.to_lowercase().starts_with(&q);
            let pb = b.0.address.to_lowercase().starts_with(&q) || b.0.name.to_lowercase().starts_with(&q);
            pb.cmp(&pa).then(b.1.cmp(&a.1)).then(a.0.address.cmp(&b.0.address))
        });
        Ok(v.into_iter().take(limit).map(|(a, _)| a).collect())
    }

    // ---------------------------------------------------------------- op queue

    pub fn add_op(&self, account_id: i64, folder_id: i64, uid: u32, kind: &OpKind) -> Result<()> {
        let (k, target) = kind.encode();
        let db = self.db();
        // A newer flag op on the same message supersedes the opposite one.
        if let OpKind::AddFlag(_) | OpKind::RemoveFlag(_) = kind {
            let opposite = if k.starts_with('+') { k.replacen('+', "-", 1) } else { k.replacen('-', "+", 1) };
            db.execute(
                "DELETE FROM ops WHERE folder_id = ?1 AND uid = ?2 AND kind IN (?3, ?4)",
                params![folder_id, uid, k, opposite],
            )?;
        }
        db.execute(
            "INSERT INTO ops(account_id, folder_id, uid, kind, target_folder_id, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![account_id, folder_id, uid, k, target, now()],
        )?;
        Ok(())
    }

    pub fn ops(&self, account_id: i64) -> Result<Vec<Op>> {
        let db = self.db();
        let mut st = db.prepare(
            "SELECT id, folder_id, uid, kind, target_folder_id, attempts FROM ops WHERE account_id = ?1 ORDER BY id",
        )?;
        let rows = st.query_map([account_id], |r| {
            Ok((r.get::<_, i64>(0)?, r.get(1)?, r.get(2)?, r.get::<_, String>(3)?, r.get(4)?, r.get(5)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, folder_id, uid, kind, target, attempts) = row?;
            match OpKind::decode(&kind, target) {
                Some(kind) => out.push(Op { id, folder_id, uid, kind, attempts }),
                None => {
                    db.execute("DELETE FROM ops WHERE id = ?1", [id])?;
                }
            }
        }
        Ok(out)
    }

    pub fn delete_op(&self, id: i64) -> Result<()> {
        self.db().execute("DELETE FROM ops WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn bump_op(&self, id: i64) -> Result<()> {
        self.db().execute("UPDATE ops SET attempts = attempts + 1 WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Uids with queued changes: hidden ones (move/delete) and ones with pending flag edits.
    pub fn pending_uids(&self, folder_id: i64) -> Result<(HashSet<u32>, HashSet<u32>)> {
        let db = self.db();
        let mut st = db.prepare("SELECT uid, kind FROM ops WHERE folder_id = ?1")?;
        let mut hidden = HashSet::new();
        let mut flags = HashSet::new();
        for row in st.query_map([folder_id], |r| Ok((r.get::<_, u32>(0)?, r.get::<_, String>(1)?)))? {
            let (uid, kind) = row?;
            if kind == "move" || kind == "delete" {
                hidden.insert(uid);
            } else {
                flags.insert(uid);
            }
        }
        Ok((hidden, flags))
    }
}

pub fn special_name(role: FolderRole) -> Option<String> {
    Some(match role {
        FolderRole::Inbox => t!("Inbox", "收件箱"),
        FolderRole::Sent => t!("Sent", "已发送"),
        FolderRole::Drafts => t!("Drafts", "草稿"),
        FolderRole::Archive => t!("Archive", "归档"),
        FolderRole::Junk => t!("Junk", "垃圾邮件"),
        FolderRole::Trash => t!("Trash", "废纸篓"),
        FolderRole::All => t!("All Mail", "所有邮件"),
        FolderRole::Flagged => t!("Starred", "星标邮件"),
        FolderRole::Important => t!("Important", "重要邮件"),
        FolderRole::Custom => return None,
    })
}

/// Splits "Name <addr>" (as stored in to_line) into its parts.
fn split_display(s: &str) -> (String, String) {
    let s = s.trim();
    match (s.rfind('<'), s.rfind('>')) {
        (Some(l), Some(r)) if l < r => (s[..l].trim().trim_matches('"').to_string(), s[l + 1..r].trim().to_string()),
        _ => (String::new(), s.to_string()),
    }
}

/// Builds the WHERE fragment for a search string.
/// Supports quoted phrases and from: to: subject: has:attachment is:unread is:read is:flagged.
pub fn search_clause(input: &str) -> (String, Vec<Value>) {
    let mut parts: Vec<String> = Vec::new();
    let mut args: Vec<Value> = Vec::new();
    let mut fts_terms: Vec<String> = Vec::new();
    for token in tokenize(input) {
        let lower = token.to_lowercase();
        let like = |s: &str| Value::Text(format!("%{}%", s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")));
        if let Some(v) = lower.strip_prefix("from:").or_else(|| lower.strip_prefix("发件人:")) {
            parts.push("(m.from_name LIKE ? ESCAPE '\\' OR m.from_addr LIKE ? ESCAPE '\\')".into());
            args.push(like(v));
            args.push(like(v));
        } else if let Some(v) = lower.strip_prefix("to:").or_else(|| lower.strip_prefix("收件人:")) {
            parts.push("(m.to_line LIKE ? ESCAPE '\\' OR m.cc_line LIKE ? ESCAPE '\\')".into());
            args.push(like(v));
            args.push(like(v));
        } else if let Some(v) = lower.strip_prefix("subject:").or_else(|| lower.strip_prefix("主题:")) {
            parts.push("m.subject LIKE ? ESCAPE '\\'".into());
            args.push(like(v));
        } else if lower == "has:attachment" || lower == "has:附件" {
            parts.push("m.has_attachments = 1".into());
        } else if lower == "is:unread" || lower == "is:未读" {
            parts.push("m.seen = 0".into());
        } else if lower == "is:read" || lower == "is:已读" {
            parts.push("m.seen = 1".into());
        } else if lower == "is:flagged" || lower == "is:starred" || lower == "is:旗标" {
            parts.push("m.flagged = 1".into());
        } else if token.chars().count() >= 3 {
            fts_terms.push(format!("\"{}\"", token.replace('"', "\"\"")));
        } else {
            parts.push(
                "(m.subject LIKE ? ESCAPE '\\' OR m.from_name LIKE ? ESCAPE '\\' OR m.from_addr LIKE ? ESCAPE '\\'
                  OR m.id IN (SELECT rowid FROM messages_fts WHERE body LIKE ? ESCAPE '\\'))"
                    .into(),
            );
            for _ in 0..4 {
                args.push(like(&token));
            }
        }
    }
    if !fts_terms.is_empty() {
        parts.push("m.id IN (SELECT rowid FROM messages_fts WHERE messages_fts MATCH ?)".into());
        args.push(Value::Text(fts_terms.join(" ")));
    }
    (parts.join(" AND "), args)
}

pub(crate) fn tokenize(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in input.chars() {
        match c {
            '"' | '“' | '”' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mime;

    fn setup() -> (tempfile::TempDir, Store, i64, i64) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let acct = store
            .insert_account(
                &AccountSetup {
                    email: "me@example.com".into(),
                    display_name: "Me".into(),
                    provider: ProviderKind::Other,
                    imap: ServerConfig { host: "imap.example.com".into(), port: 993, security: Security::Tls },
                    smtp: ServerConfig { host: "smtp.example.com".into(), port: 465, security: Security::Tls },
                    username: "me".into(),
                    password: String::new(),
                    sync_days: 30,
                    allow_invalid_certs: false,
                },
                false,
            )
            .unwrap();
        store
            .sync_folder_list(
                acct,
                &[
                    ServerFolder { name: "INBOX".into(), delimiter: Some("/".into()), role: FolderRole::Inbox, selectable: true },
                    ServerFolder { name: "Trash".into(), delimiter: Some("/".into()), role: FolderRole::Trash, selectable: true },
                    ServerFolder { name: "[Gmail]/All Mail".into(), delimiter: Some("/".into()), role: FolderRole::All, selectable: true },
                    ServerFolder { name: "&mHl27g-/2026".into(), delimiter: Some("/".into()), role: FolderRole::Custom, selectable: true },
                ],
            )
            .unwrap();
        let inbox = store.folder_by_role(acct, FolderRole::Inbox).unwrap().unwrap().id;
        (dir, store, acct, inbox)
    }

    fn add(store: &Store, acct: i64, folder: i64, uid: u32, date: i64, subject: &str, body: &str, from: &str) -> i64 {
        let raw = format!("From: {from}\r\nTo: me@example.com\r\nSubject: {subject}\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{body}\r\n");
        let parsed = mime::parse(raw.as_bytes());
        store
            .insert_message(&NewMessage {
                account_id: acct,
                folder_id: folder,
                uid,
                uidvalidity: 1,
                flags: Flags::default(),
                internal_date: date,
                size: raw.len() as u64,
                parsed: &parsed,
                raw: raw.as_bytes(),
            })
            .unwrap()
    }

    fn q(store: &Store, mailbox: Mailbox, search: Option<&str>) -> Vec<String> {
        store
            .query(&MessageQuery { mailbox, search: search.map(Into::into), limit: 100, before_date: None, before_id: None })
            .unwrap()
            .into_iter()
            .map(|m| m.subject)
            .collect()
    }

    #[test]
    fn folders_roles_and_defaults() {
        let (_d, store, acct, _) = setup();
        let folders = store.folders(acct).unwrap();
        let names: Vec<_> = folders.iter().map(|f| f.display_name.as_str()).collect();
        assert_eq!(names, vec!["Inbox", "Trash", "All Mail", "项目/2026"], "parent 项目 is not a mailbox");
        let zh: Vec<String> = crate::i18n::with_language(true, || store.folders(acct).unwrap().into_iter().map(|f| f.display_name).collect());
        assert_eq!(zh, vec!["收件箱", "废纸篓", "所有邮件", "项目/2026"]);
        let all = folders.iter().find(|f| f.role == FolderRole::All).unwrap();
        assert!(!all.sync_enabled, "virtual Gmail folders are off by default");
        let custom = folders.iter().find(|f| f.role == FolderRole::Custom).unwrap();
        assert_eq!(custom.path, "项目/2026");
        assert_eq!(custom.depth, 0);

        // with the parent present, the child nests under it
        store
            .sync_folder_list(acct, &[
                ServerFolder { name: "INBOX".into(), delimiter: Some("/".into()), role: FolderRole::Inbox, selectable: true },
                ServerFolder { name: "&mHl27g-".into(), delimiter: Some("/".into()), role: FolderRole::Custom, selectable: true },
                ServerFolder { name: "&mHl27g-/2026".into(), delimiter: Some("/".into()), role: FolderRole::Custom, selectable: true },
            ])
            .unwrap();
        let nested = store.folders(acct).unwrap();
        let child = nested.iter().find(|f| f.path == "项目/2026").unwrap();
        assert_eq!((child.display_name.as_str(), child.depth), ("2026", 1));

        // folder removed on server disappears locally
        let gone = store
            .sync_folder_list(acct, &[ServerFolder { name: "INBOX".into(), delimiter: None, role: FolderRole::Inbox, selectable: true }])
            .unwrap();
        assert_eq!(gone.len(), 2);
        assert_eq!(store.folders(acct).unwrap().len(), 1);
    }

    #[test]
    fn insert_query_paginate_and_counts() {
        let (_d, store, acct, inbox) = setup();
        for i in 0..5 {
            add(&store, acct, inbox, 10 + i, 1000 + i as i64, &format!("Msg {i}"), "body", "a@b.c");
        }
        let all = store
            .query(&MessageQuery { mailbox: Mailbox::Folder { id: inbox }, search: None, limit: 2, before_date: None, before_id: None })
            .unwrap();
        assert_eq!(all.iter().map(|m| m.subject.as_str()).collect::<Vec<_>>(), vec!["Msg 4", "Msg 3"]);
        let next = store
            .query(&MessageQuery {
                mailbox: Mailbox::Folder { id: inbox },
                search: None,
                limit: 2,
                before_date: Some(all[1].date),
                before_id: Some(all[1].id),
            })
            .unwrap();
        assert_eq!(next.iter().map(|m| m.subject.as_str()).collect::<Vec<_>>(), vec!["Msg 2", "Msg 1"]);
        let f = &store.folders(acct).unwrap()[0];
        assert_eq!((f.unread, f.total), (5, 5));
        assert_eq!(store.unread_inbox_total().unwrap(), 5);
        assert_eq!(store.storage(Some(acct)).unwrap().messages, 5);
    }

    #[test]
    fn full_text_search_chinese_and_operators() {
        let (_d, store, acct, inbox) = setup();
        add(&store, acct, inbox, 1, 100, "周会纪要", "明天下午三点开项目复盘会议", "张三 <zhang@corp.cn>");
        add(&store, acct, inbox, 2, 200, "Invoice #42", "Your receipt from Apple", "Apple <no_reply@apple.com>");
        add(&store, acct, inbox, 3, 300, "Lunch?", "要不要一起吃饭", "Bob <bob@example.com>");
        let mb = || Mailbox::Folder { id: inbox };
        assert_eq!(q(&store, mb(), Some("复盘会议")), vec!["周会纪要"], "CJK substring in body");
        assert_eq!(q(&store, mb(), Some("吃饭")), vec!["Lunch?"], "two-char CJK falls back to LIKE");
        assert_eq!(q(&store, mb(), Some("receipt")), vec!["Invoice #42"]);
        assert_eq!(q(&store, mb(), Some("RECEIPT apple")), vec!["Invoice #42"], "case-insensitive, AND");
        assert_eq!(q(&store, mb(), Some("from:张三")), vec!["周会纪要"]);
        assert_eq!(q(&store, mb(), Some("from:bob")), vec!["Lunch?"]);
        assert_eq!(q(&store, mb(), Some("subject:invoice")), vec!["Invoice #42"]);
        assert_eq!(q(&store, mb(), Some("\"Your receipt\"")), vec!["Invoice #42"]);
        assert!(q(&store, mb(), Some("nothing-like-this")).is_empty());
        assert_eq!(q(&store, mb(), Some("5%")).len(), 0, "LIKE wildcards are escaped");
        assert_eq!(q(&store, mb(), Some("is:unread")).len(), 3);
        // fts rows go away with the message
        store.delete_uids(inbox, &[1]).unwrap();
        assert!(q(&store, mb(), Some("复盘会议")).is_empty());
    }

    #[test]
    fn flags_and_smart_mailboxes() {
        let (_d, store, acct, inbox) = setup();
        let trash = store.folder_by_role(acct, FolderRole::Trash).unwrap().unwrap().id;
        let a = add(&store, acct, inbox, 1, 100, "A", "x", "a@b.c");
        let b = add(&store, acct, trash, 1, 100, "B", "x", "a@b.c");
        store.set_local_flag(&[a, b], LocalFlag::Flagged, true).unwrap();
        assert_eq!(q(&store, Mailbox::Flagged, None), vec!["A"], "trash excluded from Flagged");
        assert_eq!(q(&store, Mailbox::Unread, None), vec!["A"]);
        let changed = store.set_local_flag(&[a], LocalFlag::Seen, true).unwrap();
        assert_eq!(changed.len(), 1);
        assert!(store.set_local_flag(&[a], LocalFlag::Seen, true).unwrap().is_empty(), "no-op when unchanged");
        assert!(q(&store, Mailbox::Unread, None).is_empty());
        let n = store.update_flags(inbox, &[(1, Flags { seen: false, flagged: false, answered: true })]).unwrap();
        assert_eq!(n, 1);
        assert_eq!(store.smart_counts().unwrap(), (1, 0));
    }

    #[test]
    fn contacts_rank_people_you_write_to() {
        let (_d, store, acct, inbox) = setup();
        store
            .sync_folder_list(acct, &[
                ServerFolder { name: "INBOX".into(), delimiter: None, role: FolderRole::Inbox, selectable: true },
                ServerFolder { name: "Sent".into(), delimiter: None, role: FolderRole::Sent, selectable: true },
            ])
            .unwrap();
        let sent = store.folder_by_role(acct, FolderRole::Sent).unwrap().unwrap().id;
        add(&store, acct, inbox, 1, 100, "a", "x", "Alice Zhang <alice@x.com>");
        add(&store, acct, inbox, 2, 101, "b", "x", "Bob <bob@x.com>");
        add(&store, acct, inbox, 3, 102, "c", "x", "Bob <bob@x.com>");
        add(&store, acct, inbox, 4, 103, "d", "x", "no-reply@shop.com");
        let raw = "From: me@example.com\r\nTo: \"Carol\" <carol@x.com>\r\nSubject: s\r\n\r\nx";
        let parsed = mime::parse(raw.as_bytes());
        store
            .insert_message(&NewMessage { account_id: acct, folder_id: sent, uid: 9, uidvalidity: 1, flags: Flags::default(),
                internal_date: 104, size: 1, parsed: &parsed, raw: raw.as_bytes() })
            .unwrap();
        let all: Vec<String> = store.contacts("", 10).unwrap().into_iter().map(|a| a.address).collect();
        assert_eq!(all, vec!["carol@x.com", "bob@x.com", "alice@x.com", "me@example.com"]);
        let a = store.contacts("ali", 5).unwrap();
        assert_eq!(a[0].name, "Alice Zhang");
        assert!(store.contacts("shop", 5).unwrap().is_empty(), "no-reply addresses skipped");
        assert_eq!(store.contacts("zhang", 5).unwrap()[0].address, "alice@x.com", "matches names too");
    }

    #[test]
    fn gmail_labels_do_not_duplicate_smart_views() {
        let (_d, store, acct, inbox) = setup();
        let label = store.folders(acct).unwrap().into_iter().find(|f| f.role == FolderRole::Custom).unwrap().id;
        let raw = "From: a@b.c\r\nMessage-ID: <same@x>\r\nSubject: Labeled\r\n\r\nbody text here";
        let parsed = mime::parse(raw.as_bytes());
        for (folder, uid) in [(inbox, 1), (label, 7)] {
            store
                .insert_message(&NewMessage { account_id: acct, folder_id: folder, uid, uidvalidity: 1,
                    flags: Flags { seen: false, flagged: true, answered: false }, internal_date: 100, size: 1,
                    parsed: &parsed, raw: raw.as_bytes() })
                .unwrap();
        }
        assert_eq!(q(&store, Mailbox::Flagged, None), vec!["Labeled"]);
        assert_eq!(q(&store, Mailbox::Everything, Some("body text")), vec!["Labeled"]);
        assert_eq!(q(&store, Mailbox::Folder { id: label }, None), vec!["Labeled"], "each folder still lists it");
        assert_eq!(store.smart_counts().unwrap().1, 1);
    }

    #[test]
    fn op_queue_supersedes_and_hides() {
        let (_d, store, acct, inbox) = setup();
        store.add_op(acct, inbox, 7, &OpKind::AddFlag(LocalFlag::Seen)).unwrap();
        store.add_op(acct, inbox, 7, &OpKind::RemoveFlag(LocalFlag::Seen)).unwrap();
        store.add_op(acct, inbox, 8, &OpKind::Move { target_folder_id: 99 }).unwrap();
        let ops = store.ops(acct).unwrap();
        assert_eq!(ops.len(), 2);
        assert_eq!(ops[0].kind, OpKind::RemoveFlag(LocalFlag::Seen));
        assert_eq!(ops[1].kind, OpKind::Move { target_folder_id: 99 });
        let (hidden, flags) = store.pending_uids(inbox).unwrap();
        assert!(hidden.contains(&8) && flags.contains(&7));
        store.delete_op(ops[0].id).unwrap();
        assert_eq!(store.ops(acct).unwrap().len(), 1);
    }

    #[test]
    fn wipe_and_delete_account_remove_files() {
        let (dir, store, acct, inbox) = setup();
        let id = add(&store, acct, inbox, 1, 100, "A", "x", "a@b.c");
        let path = store.message_row(id).unwrap().raw_path;
        assert!(path.exists());
        store.wipe_folder(acct, inbox).unwrap();
        assert!(!path.exists());
        add(&store, acct, inbox, 2, 100, "B", "x", "a@b.c");
        store.delete_account(acct).unwrap();
        assert!(store.accounts().unwrap().is_empty());
        assert!(!dir.path().join("mail").join(acct.to_string()).exists());
        assert_eq!(store.storage(None).unwrap().messages, 0);
    }

    #[test]
    fn install_id_is_stable_per_directory() {
        let (dir, store, _, _) = setup();
        let a = store.install_id().unwrap();
        assert_eq!(a.len(), 12);
        assert_eq!(store.install_id().unwrap(), a);
        drop(store);
        assert_eq!(Store::open(dir.path()).unwrap().install_id().unwrap(), a);
        let (_d2, other, _, _) = setup();
        assert_ne!(other.install_id().unwrap(), a);
    }

    #[test]
    fn reopen_keeps_data() {
        let (dir, store, acct, inbox) = setup();
        add(&store, acct, inbox, 1, 100, "Persisted", "x", "a@b.c");
        drop(store);
        let store = Store::open(dir.path()).unwrap();
        assert_eq!(q(&store, Mailbox::AllInboxes, None), vec!["Persisted"]);
    }
}
