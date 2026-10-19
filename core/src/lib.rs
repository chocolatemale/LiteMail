//! LiteMail core: IMAP sync, local storage and search, sending. Exposed to Swift via UniFFI.

uniffi::setup_scaffolding!();

#[macro_use]
pub mod i18n;
pub mod compose;
pub mod error;
pub mod imap;
pub mod log;
pub mod mime;
pub mod providers;
pub mod reader;
pub mod secrets;
pub mod server_search;
pub mod smtp;
pub mod store;
pub mod sync;
pub mod types;
pub mod utf7;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use error::{CoreError, Result};
use secrets::{FileSecrets, Keychain, SecretStore};
use store::{LocalFlag, MessageRow, OpKind, Store};
use sync::{Cmd, Ctx, Worker};
use types::*;

/// Implemented by the UI. Called from background threads.
#[uniffi::export(with_foreign)]
pub trait CoreListener: Send + Sync {
    fn on_event(&self, event: CoreEvent);
}

/// Sets the language for strings the core produces ("en", "zh-Hans", …). Call before `MailCore::new`.
#[uniffi::export]
pub fn set_language(tag: String) {
    i18n::set_language(&tag);
}

/// Use a 24-hour clock in times the core writes (reading pane, reply headers).
#[uniffi::export]
pub fn set_hour24(on: bool) {
    i18n::set_hour24(on);
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct SmartCounts {
    pub inbox_unread: u32,
    pub flagged: u32,
}

/// Total attachment size accepted for sending. Most providers reject 25 MB+.
const MAX_ATTACHMENT_BYTES: u64 = 25 * 1024 * 1024;

#[derive(uniffi::Object)]
pub struct MailCore {
    rt: tokio::runtime::Runtime,
    ctx: Arc<Ctx>,
    workers: Mutex<HashMap<i64, Worker>>,
    /// Server search results (negative ids) and the bodies opened from them, in memory only.
    remote: Mutex<server_search::Cache>,
}

impl MailCore {
    /// Constructor used by tests: custom secret store, optional autostart of sync.
    pub fn with_secrets(data_dir: PathBuf, listener: Arc<dyn CoreListener>, secrets: Arc<dyn SecretStore>) -> Result<Arc<Self>> {
        log::init(data_dir.join("logs").join("core.log"));
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("litemail-core")
            .enable_all()
            .build()
            .map_err(|e| CoreError::Storage { message: e.to_string() })?;
        let store = Arc::new(Store::open(&data_dir)?);
        let ctx = Arc::new(Ctx { store, secrets, listener });
        let core = Arc::new(MailCore { rt, ctx, workers: Mutex::new(HashMap::new()), remote: Mutex::new(Default::default()) });
        for account in core.ctx.store.accounts()? {
            core.start_worker(account.id);
        }
        log_info!("core started, {} accounts", core.workers.lock().unwrap().len());
        Ok(core)
    }

    fn start_worker(&self, account_id: i64) {
        let w = Worker::spawn(self.rt.handle(), self.ctx.clone(), account_id);
        self.workers.lock().unwrap().insert(account_id, w);
    }

    fn send_cmd(&self, account_id: i64, cmd: Cmd) {
        if let Some(w) = self.workers.lock().unwrap().get(&account_id) {
            w.send(cmd);
        }
    }

    fn emit(&self, e: CoreEvent) {
        self.ctx.emit(e);
    }

    /// A message's row and raw bytes: from the store, or — for a server search hit (negative id) —
    /// from memory, downloading it on first use. Server messages are never written to disk.
    fn read_raw(&self, id: i64) -> Result<(MessageRow, Vec<u8>)> {
        if id >= 0 {
            return self.ctx.store.read_raw(id);
        }
        let hit = self.remote.lock().unwrap().hit(id).ok_or(CoreError::NotFound)?;
        let cached = self.remote.lock().unwrap().body(id);
        let raw = match cached {
            Some(b) => b,
            None => {
                let account = self.ctx.store.account(hit.summary.account_id)?;
                let password = self.ctx.secrets.get(account.id)?;
                let raw = self.rt.block_on(async {
                    let (mut session, _) = imap::login(&account.imap, account.allow_invalid_certs, &account.username, &password).await?;
                    let raw = server_search::fetch_body(&mut session, &hit.folder_name, hit.uid).await;
                    let _ = session.logout().await;
                    raw
                })?;
                let raw = Arc::new(raw);
                self.remote.lock().unwrap().put_body(id, raw.clone());
                raw
            }
        };
        let s = &hit.summary;
        let row = MessageRow {
            id,
            account_id: s.account_id,
            folder_id: s.folder_id,
            uid: hit.uid,
            seen: s.seen,
            flagged: s.flagged,
            date: s.date,
            raw_path: PathBuf::new(),
        };
        Ok((row, raw.to_vec()))
    }

    fn rows(&self, ids: &[i64]) -> Vec<MessageRow> {
        ids.iter().filter_map(|id| self.ctx.store.message_row(*id).ok()).collect()
    }

    fn changed(&self, accounts: impl IntoIterator<Item = i64>) {
        let mut v: Vec<i64> = accounts.into_iter().collect();
        v.sort_unstable();
        v.dedup();
        for a in v {
            self.emit(CoreEvent::MessagesChanged { account_id: a });
            self.emit(CoreEvent::FoldersChanged { account_id: a });
            self.send_cmd(a, Cmd::FlushOps);
        }
    }

    fn set_flag(&self, ids: &[i64], flag: LocalFlag, value: bool) -> Result<()> {
        let changed = self.ctx.store.set_local_flag(ids, flag, value)?;
        for row in &changed {
            let kind = if value { OpKind::AddFlag(flag) } else { OpKind::RemoveFlag(flag) };
            self.ctx.store.add_op(row.account_id, row.folder_id, row.uid, &kind)?;
        }
        self.changed(changed.iter().map(|r| r.account_id));
        Ok(())
    }

    fn move_rows(&self, rows: Vec<(MessageRow, OpKind)>) -> Result<()> {
        let ids: Vec<i64> = rows.iter().map(|(r, _)| r.id).collect();
        for (row, kind) in &rows {
            self.ctx.store.add_op(row.account_id, row.folder_id, row.uid, kind)?;
        }
        self.ctx.store.remove_local(&ids)?;
        self.changed(rows.iter().map(|(r, _)| r.account_id));
        Ok(())
    }

    /// Pasted passwords often carry a line break or stray spaces at the ends; IMAP can't send
    /// control characters, so drop them before anything else sees the password.
    fn clean_password(password: &str) -> String {
        password.chars().filter(|c| !c.is_control()).collect::<String>().trim().to_string()
    }

    fn validate(setup: &AccountSetup) -> Result<()> {
        let email = setup.email.trim();
        if !email.contains('@') || email.starts_with('@') || email.ends_with('@') {
            return Err(CoreError::invalid(t!("Enter a full email address", "请输入完整的邮箱地址")));
        }
        if setup.imap.host.trim().is_empty() || setup.smtp.host.trim().is_empty() {
            return Err(CoreError::invalid(t!("Enter the incoming and outgoing servers", "请填写收信和发信服务器")));
        }
        if setup.imap.port == 0 || setup.smtp.port == 0 {
            return Err(CoreError::invalid(t!("Invalid port", "端口无效")));
        }
        if setup.password.is_empty() {
            return Err(CoreError::invalid(t!("Enter a password", "请输入密码")));
        }
        Ok(())
    }

    /// Checks IMAP and SMTP, retrying once on a transient network failure.
    async fn verify(setup: &AccountSetup) -> Result<()> {
        match Self::verify_once(setup).await {
            Err(e) if e.is_connection_error() => {
                tokio::time::sleep(std::time::Duration::from_millis(800)).await;
                Self::verify_once(setup).await
            }
            other => other,
        }
    }

    async fn verify_once(setup: &AccountSetup) -> Result<()> {
        let user = if setup.username.trim().is_empty() { setup.email.trim() } else { setup.username.trim() };
        let (mut session, _) = imap::login(&setup.imap, setup.allow_invalid_certs, user, &setup.password)
            .await
            .map_err(|e| match e {
                CoreError::Auth { message } => CoreError::Auth { message: t!("Incoming server: {message}", "收信服务器：{message}") },
                e => e,
            })?;
        let _ = session.logout().await;
        smtp::check(&setup.smtp, setup.allow_invalid_certs, user, &setup.password).await
    }
}

#[uniffi::export]
impl MailCore {
    /// Opens (or creates) the data directory and starts syncing every account.
    #[uniffi::constructor]
    pub fn new(data_dir: String, listener: Arc<dyn CoreListener>) -> Result<Arc<Self>> {
        let dir = PathBuf::from(data_dir);
        // Mail lives here: only this user may enter the directory.
        std::fs::create_dir_all(&dir)?;
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        }
        if let Some(path) = std::env::var_os("LITEMAIL_TEST_SECRETS_FILE") {
            return Self::with_secrets(dir, listener, Arc::new(FileSecrets::new(PathBuf::from(path))));
        }
        let keychain = Keychain::new(Store::open(&dir)?.install_id()?);
        log::init(dir.join("logs").join("core.log"));
        secrets::import_password_file(&keychain, &dir.join("passwords"));
        Self::with_secrets(dir, listener, Arc::new(keychain))
    }

    pub fn version(&self) -> String {
        env!("CARGO_PKG_VERSION").to_string()
    }

    // ------------------------------------------------------------ accounts

    pub fn providers(&self) -> Vec<ProviderPreset> {
        providers::presets()
    }

    pub fn suggest_settings(&self, email: String) -> ProviderPreset {
        providers::suggest(&email)
    }

    /// Logs in to IMAP and SMTP without saving anything.
    pub fn verify_account(&self, mut setup: AccountSetup) -> Result<()> {
        setup.password = Self::clean_password(&setup.password);
        Self::validate(&setup)?;
        self.rt.block_on(Self::verify(&setup))
    }

    pub fn add_account(&self, mut setup: AccountSetup) -> Result<Account> {
        setup.password = Self::clean_password(&setup.password);
        Self::validate(&setup)?;
        if setup.username.trim().is_empty() {
            setup.username = setup.email.trim().to_string();
        }
        if setup.display_name.trim().is_empty() {
            setup.display_name = setup.email.split('@').next().unwrap_or_default().to_string();
        }
        let lower = setup.email.trim().to_lowercase();
        if self.ctx.store.accounts()?.iter().any(|a| a.email.to_lowercase() == lower) {
            return Err(CoreError::invalid(t!("This account has already been added", "这个账户已经添加过了")));
        }
        self.rt.block_on(Self::verify(&setup))?;
        let saves_sent = match setup.provider {
            ProviderKind::Gmail => true,
            ProviderKind::ICloud => false,
            ProviderKind::Other => providers::suggest(&setup.email).server_saves_sent,
        };
        let id = self.ctx.store.insert_account(&setup, saves_sent)?;
        if let Err(e) = self.ctx.secrets.set(id, &setup.password) {
            let _ = self.ctx.store.delete_account(id);
            return Err(e);
        }
        self.start_worker(id);
        log_info!("account {id} added ({:?})", setup.provider);
        self.emit(CoreEvent::AccountsChanged);
        self.ctx.store.account(id)
    }

    pub fn accounts(&self) -> Vec<Account> {
        self.ctx.store.accounts().unwrap_or_default()
    }

    pub fn update_account(&self, account_id: i64, settings: AccountSettings) -> Result<()> {
        self.ctx.store.update_account_settings(account_id, &settings)?;
        self.emit(CoreEvent::AccountsChanged);
        self.send_cmd(account_id, Cmd::SyncAll);
        Ok(())
    }

    /// Replaces the stored password after checking that it works.
    pub fn update_password(&self, account_id: i64, password: String) -> Result<()> {
        let password = Self::clean_password(&password);
        if password.is_empty() {
            return Err(CoreError::invalid(t!("Enter a password", "请输入密码")));
        }
        let a = self.ctx.store.account(account_id)?;
        let setup = AccountSetup {
            email: a.email.clone(),
            display_name: a.display_name.clone(),
            provider: a.provider,
            imap: a.imap.clone(),
            smtp: a.smtp.clone(),
            username: a.username.clone(),
            password: password.clone(),
            sync_days: a.sync_days,
            allow_invalid_certs: a.allow_invalid_certs,
        };
        self.rt.block_on(Self::verify(&setup))?;
        self.ctx.secrets.set(account_id, &password)?;
        self.send_cmd(account_id, Cmd::Reconnect);
        if let Some(w) = self.workers.lock().unwrap().get(&account_id) {
            w.restart_idle();
        }
        Ok(())
    }

    pub fn remove_account(&self, account_id: i64) -> Result<()> {
        if let Some(w) = self.workers.lock().unwrap().remove(&account_id) {
            w.stop();
        }
        self.ctx.store.delete_account(account_id)?;
        self.ctx.secrets.delete(account_id);
        log_info!("account {account_id} removed");
        self.emit(CoreEvent::AccountsChanged);
        Ok(())
    }

    // ------------------------------------------------------------ folders & messages

    pub fn folders(&self, account_id: i64) -> Vec<Folder> {
        self.ctx.store.folders(account_id).unwrap_or_default()
    }

    pub fn set_folder_sync(&self, folder_id: i64, enabled: bool) -> Result<()> {
        let f = self.ctx.store.folder(folder_id)?;
        self.ctx.store.set_folder_sync_enabled(folder_id, enabled)?;
        self.emit(CoreEvent::FoldersChanged { account_id: f.account_id });
        self.emit(CoreEvent::MessagesChanged { account_id: f.account_id });
        if enabled {
            self.send_cmd(f.account_id, Cmd::SyncFolder(folder_id));
        }
        Ok(())
    }

    pub fn messages(&self, query: MessageQuery) -> Vec<MessageSummary> {
        match self.ctx.store.query(&query) {
            Ok(v) => v,
            Err(e) => {
                log_warn!("query failed: {e}");
                Vec::new()
            }
        }
    }

    pub fn message(&self, message_id: i64, allow_remote: bool) -> Result<MessageDetail> {
        let (row, raw) = self.read_raw(message_id)?;
        let account = self.ctx.store.account(row.account_id)?;
        let folder = self.ctx.store.folders(row.account_id)?.into_iter().find(|f| f.id == row.folder_id);
        let p = mime::parse(&raw);
        let attachments = mime::attachments(&raw);
        let account_label = match account.provider {
            ProviderKind::ICloud => "iCloud".to_string(),
            ProviderKind::Gmail => "Gmail".to_string(),
            ProviderKind::Other => account.email.clone(),
        };
        let kicker = match &folder {
            Some(f) => format!("{account_label} · {}", f.path),
            None => account_label,
        };
        let date = p.date.unwrap_or(row.date);
        let (html, has_remote_content) = reader::document(
            &raw,
            &p,
            &attachments,
            &reader::HeaderInfo { kicker, account_email: &account.email, date, allow_remote },
        );
        Ok(MessageDetail {
            id: row.id,
            account_id: row.account_id,
            folder_id: row.folder_id,
            subject: p.subject.clone(),
            from: p.from.clone(),
            to: p.to.clone(),
            cc: p.cc.clone(),
            reply_to: p.reply_to.clone(),
            date,
            seen: row.seen,
            flagged: row.flagged,
            attachments,
            has_remote_content,
            html,
            list_unsubscribe: p.list_unsubscribe,
        })
    }

    /// Recipient suggestions for the composer.
    pub fn contacts(&self, query: String, limit: u32) -> Vec<EmailAddress> {
        self.ctx.store.contacts(&query, limit as usize).unwrap_or_default()
    }

    /// Raw RFC 5322 source, for "View Source".
    pub fn message_source(&self, message_id: i64) -> Result<String> {
        let (_, raw) = self.read_raw(message_id)?;
        Ok(String::from_utf8_lossy(&raw).into_owned())
    }

    pub fn mark_seen(&self, ids: Vec<i64>, seen: bool) -> Result<()> {
        self.set_flag(&ids, LocalFlag::Seen, seen)
    }

    pub fn mark_flagged(&self, ids: Vec<i64>, flagged: bool) -> Result<()> {
        self.set_flag(&ids, LocalFlag::Flagged, flagged)
    }

    /// Marks every message in a mailbox as read.
    pub fn mark_all_seen(&self, mailbox: Mailbox) -> Result<()> {
        let unread = self.ctx.store.query(&MessageQuery {
            mailbox,
            search: Some("is:unread".into()),
            limit: 2000,
            before_date: None,
            before_id: None,
        })?;
        let ids: Vec<i64> = unread.iter().map(|m| m.id).collect();
        self.set_flag(&ids, LocalFlag::Seen, true)
    }

    pub fn move_messages(&self, ids: Vec<i64>, target_folder_id: i64) -> Result<()> {
        let target = self.ctx.store.folder(target_folder_id)?;
        let rows = self
            .rows(&ids)
            .into_iter()
            .filter(|r| r.account_id == target.account_id && r.folder_id != target.id)
            .map(|r| (r, OpKind::Move { target_folder_id }))
            .collect();
        self.move_rows(rows)
    }

    /// Moves to the account's archive (Gmail: All Mail). Fails if the account has none.
    pub fn archive(&self, ids: Vec<i64>) -> Result<()> {
        let mut rows = Vec::new();
        for r in self.rows(&ids) {
            let target = match self.ctx.store.folder_by_role(r.account_id, FolderRole::Archive)? {
                Some(f) => f,
                None => match self.ctx.store.folder_by_role(r.account_id, FolderRole::All)? {
                    Some(f) => f,
                    None => return Err(CoreError::invalid(t!("This account has no Archive folder", "这个账户没有归档文件夹"))),
                },
            };
            if target.id != r.folder_id {
                rows.push((r, OpKind::Move { target_folder_id: target.id }));
            }
        }
        self.move_rows(rows)
    }

    /// Moves to Trash; messages already in Trash (or accounts without one) are deleted for good.
    pub fn delete_messages(&self, ids: Vec<i64>) -> Result<()> {
        let mut rows = Vec::new();
        for r in self.rows(&ids) {
            let current = self.ctx.store.folder(r.folder_id)?;
            let trash = self.ctx.store.folder_by_role(r.account_id, FolderRole::Trash)?;
            let kind = match trash {
                Some(t) if current.role != FolderRole::Trash && t.id != r.folder_id => OpKind::Move { target_folder_id: t.id },
                _ => OpKind::Delete,
            };
            rows.push((r, kind));
        }
        self.move_rows(rows)
    }

    fn discard_server_draft(&self, id: i64) -> Result<()> {
        let rows: Vec<(MessageRow, OpKind)> = self.rows(&[id]).into_iter().map(|r| (r, OpKind::Delete)).collect();
        self.move_rows(rows)
    }

    pub fn mark_junk(&self, ids: Vec<i64>, junk: bool) -> Result<()> {
        let mut rows = Vec::new();
        for r in self.rows(&ids) {
            let role = if junk { FolderRole::Junk } else { FolderRole::Inbox };
            let Some(target) = self.ctx.store.folder_by_role(r.account_id, role)? else {
                return Err(CoreError::invalid(t!("This account has no Junk folder", "这个账户没有垃圾邮件文件夹")));
            };
            if target.id != r.folder_id {
                rows.push((r, OpKind::Move { target_folder_id: target.id }));
            }
        }
        self.move_rows(rows)
    }

    pub fn save_attachment(&self, message_id: i64, index: u32, path: String) -> Result<()> {
        let (_, raw) = self.read_raw(message_id)?;
        let (_, _, bytes) = mime::attachment(&raw, index).ok_or(CoreError::NotFound)?;
        std::fs::write(path, bytes)?;
        Ok(())
    }

    /// Writes an attachment to a private temp folder (for Quick Look / open) and returns the path.
    pub fn attachment_temp_file(&self, message_id: i64, index: u32) -> Result<String> {
        let (_, raw) = self.read_raw(message_id)?;
        let (name, _, bytes) = mime::attachment(&raw, index).ok_or(CoreError::NotFound)?;
        let dir = std::env::temp_dir().join("LiteMail").join(format!("{message_id}-{index}"));
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(name);
        std::fs::write(&path, bytes)?;
        Ok(path.to_string_lossy().into_owned())
    }

    // ------------------------------------------------------------ compose

    pub fn new_draft(&self, account_id: i64) -> Result<Draft> {
        Ok(compose::new_draft(&self.ctx.store.account(account_id)?))
    }

    pub fn reply_draft(&self, message_id: i64, all: bool) -> Result<Draft> {
        let (row, raw) = self.read_raw(message_id)?;
        let account = self.ctx.store.account(row.account_id)?;
        let mine = self.own_addresses(account.id);
        Ok(compose::reply_draft(&account, &mine, &raw, message_id, chrono::Utc::now().timestamp(), all))
    }

    pub fn forward_draft(&self, message_id: i64) -> Result<Draft> {
        let (row, raw) = self.read_raw(message_id)?;
        let account = self.ctx.store.account(row.account_id)?;
        Ok(compose::forward_draft(&account, &raw, message_id, chrono::Utc::now().timestamp()))
    }

    /// Sends a draft. Blocks until the SMTP server accepted it.
    pub fn send(&self, draft: Draft) -> Result<()> {
        let account = self.ctx.store.account(draft.account_id)?;
        let password = self.ctx.secrets.get(account.id)?;
        let mut files: Vec<compose::Payload> = Vec::new();
        let mut total = 0u64;
        for path in &draft.attachments {
            let bytes = std::fs::read(path).map_err(|e| CoreError::invalid(t!("Couldn't read the attachment {path}: {e}", "无法读取附件 {path}：{e}")))?;
            total += bytes.len() as u64;
            let name = std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or(t!("Attachment", "附件"));
            files.push((name, compose::mime_for(path), bytes));
        }
        if let Some(src) = draft.forward_from {
            if let Ok((_, raw)) = self.read_raw(src) {
                for a in mime::all_attachments(&raw) {
                    total += a.2.len() as u64;
                    files.push(a);
                }
            }
        }
        if total > MAX_ATTACHMENT_BYTES {
            return Err(CoreError::invalid(t!(
                "Attachments total {:.1} MB; most providers reject anything over 25 MB",
                "附件共 {:.1} MB，超过 25 MB，大多数邮件服务商会拒收",
                total as f64 / 1_048_576.0
            )));
        }
        let out = compose::build(&account, &draft, files)?;
        self.rt.block_on(smtp::send(&account.smtp, account.allow_invalid_certs, &account.username, &password, &out.envelope, &out.raw))?;
        log_info!("account {}: message sent", account.id);

        if let Some(orig) = draft.reply_to_message {
            let _ = self.set_flag(&[orig], LocalFlag::Answered, true);
        }
        if let Some(sent) = self.ctx.store.folder_by_role(account.id, FolderRole::Sent)? {
            if account.server_saves_sent {
                self.send_cmd(account.id, Cmd::SyncFolder(sent.id));
            } else {
                self.send_cmd(
                    account.id,
                    Cmd::Append { folder_id: sent.id, raw: out.raw, message_id: out.message_id, flags: "(\\Seen)" },
                );
            }
        }
        if let Some(old) = draft.draft_source {
            let _ = self.discard_server_draft(old);
        }
        Ok(())
    }

    /// Opens a server draft for editing.
    pub fn edit_draft(&self, message_id: i64) -> Result<Draft> {
        let (row, raw) = self.read_raw(message_id)?;
        let account = self.ctx.store.account(row.account_id)?;
        let mine = self.own_addresses(account.id);
        Ok(compose::edit_draft(&account, &mine, &raw, message_id))
    }

    /// Saves a draft to the account's Drafts folder (replacing the one it continues).
    pub fn save_draft(&self, draft: Draft) -> Result<()> {
        let account = self.ctx.store.account(draft.account_id)?;
        let Some(drafts) = self.ctx.store.folder_by_role(account.id, FolderRole::Drafts)? else {
            return Err(CoreError::invalid(t!("This account has no Drafts folder", "这个账户没有草稿文件夹")));
        };
        let mut files: Vec<compose::Payload> = Vec::new();
        for path in &draft.attachments {
            if let Ok(bytes) = std::fs::read(path) {
                let name = std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or(t!("Attachment", "附件"));
                files.push((name, compose::mime_for(path), bytes));
            }
        }
        if let Some(src) = draft.forward_from {
            if let Ok((_, raw)) = self.read_raw(src) {
                files.extend(mime::all_attachments(&raw));
            }
        }
        let out = compose::build_message(&account, &draft, files, false)?;
        self.send_cmd(
            account.id,
            Cmd::Append { folder_id: drafts.id, raw: out.raw, message_id: out.message_id, flags: "(\\Seen \\Draft)" },
        );
        if let Some(old) = draft.draft_source {
            let _ = self.discard_server_draft(old);
        }
        Ok(())
    }


    // ------------------------------------------------------------ send-from addresses

    /// Every address an account can send from: its own, the ones the user added, and the ones
    /// LiteMail found — addresses it has sent from before (Sent folder) and, for iCloud, the
    /// @icloud.com twin every @me.com / @mac.com account has. Main address first.
    pub fn send_addresses(&self, account_id: i64) -> Vec<SendAddress> {
        let Ok(account) = self.ctx.store.account(account_id) else { return Vec::new() };
        let name = account.display_name.clone();
        let mut out = vec![SendAddress { account_id, name: name.clone(), address: account.email.clone(), kind: SendAddressKind::Primary }];
        let mut push = |a: EmailAddress, kind: SendAddressKind| {
            let key = a.address.trim().to_lowercase();
            if key.contains('@') && !out.iter().any(|o| o.address.to_lowercase() == key) {
                let n = if a.name.trim().is_empty() { name.clone() } else { a.name.trim().to_string() };
                out.push(SendAddress { account_id, name: n, address: a.address.trim().to_string(), kind });
            }
        };
        for a in self.ctx.store.aliases(account_id).unwrap_or_default() {
            push(a, SendAddressKind::Added);
        }
        if account.provider == ProviderKind::ICloud {
            if let Some((local, domain)) = account.email.rsplit_once('@') {
                let d = domain.to_ascii_lowercase();
                if d == "me.com" || d == "mac.com" {
                    push(EmailAddress { name: String::new(), address: format!("{local}@icloud.com") }, SendAddressKind::Found);
                }
                if d == "mac.com" {
                    push(EmailAddress { name: String::new(), address: format!("{local}@me.com") }, SendAddressKind::Found);
                }
            }
        }
        for a in self.ctx.store.sent_from_addresses(account_id).unwrap_or_default() {
            push(a, SendAddressKind::Found);
        }
        out
    }

    fn own_addresses(&self, account_id: i64) -> Vec<EmailAddress> {
        self.send_addresses(account_id).into_iter().map(|a| EmailAddress { name: a.name, address: a.address }).collect()
    }

    pub fn add_send_address(&self, account_id: i64, name: String, address: String) -> Result<()> {
        let address = address.trim().to_string();
        let ok = address.split_once('@').is_some_and(|(l, d)| !l.is_empty() && d.contains('.') && !d.starts_with('.') && !d.ends_with('.'))
            && !address.chars().any(|c| c.is_whitespace() || c == '<' || c == '>' || c == ',');
        if !ok {
            return Err(CoreError::invalid(t!("Enter a full email address", "请输入完整的邮箱地址")));
        }
        self.ctx.store.account(account_id)?;
        self.ctx.store.add_alias(account_id, name.trim(), &address)?;
        self.emit(CoreEvent::AccountsChanged);
        Ok(())
    }

    pub fn remove_send_address(&self, account_id: i64, address: String) -> Result<()> {
        self.ctx.store.remove_alias(account_id, &address)?;
        self.emit(CoreEvent::AccountsChanged);
        Ok(())
    }

    // ------------------------------------------------------------ server search

    /// Searches the servers for mail older than each account's local window (accounts that keep
    /// everything are skipped — their mail is all here). Returns header-only summaries with
    /// negative ids; open one with `message` to download it into memory. Blocks: call off the
    /// main thread.
    pub fn search_server(&self, query: String, mailbox: Mailbox) -> Result<Vec<MessageSummary>> {
        let query = query.trim().to_string();
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let only = match &mailbox {
            Mailbox::Folder { id } => Some(self.ctx.store.folder(*id)?.account_id),
            _ => None,
        };
        let mut jobs = Vec::new();
        for account in self.ctx.store.accounts()? {
            if account.sync_days == 0 || only.is_some_and(|a| a != account.id) {
                continue;
            }
            let folders = server_search::folders_for(&self.ctx.store.folders(account.id)?, &mailbox);
            if folders.is_empty() {
                continue;
            }
            let mut q = query.clone();
            match mailbox {
                Mailbox::Unread => q.push_str(" is:unread"),
                Mailbox::Flagged => q.push_str(" is:flagged"),
                _ => {}
            }
            let locals: Vec<std::collections::HashSet<u32>> =
                folders.iter().map(|f| self.ctx.store.local_uids(f.id).map(|m| m.into_keys().collect()).unwrap_or_default()).collect();
            let password = self.ctx.secrets.get(account.id);
            jobs.push(async move {
                let password = password?;
                let (mut session, caps) = imap::login(&account.imap, account.allow_invalid_certs, &account.username, &password).await?;
                // Same window start as the sync's SINCE, so nothing falls between the two.
                let before = sync::window_start(account.sync_days);
                let crit = server_search::criteria(&q, caps.gmail, before);
                let mut hits = Vec::new();
                for (folder, local) in folders.iter().zip(&locals) {
                    match server_search::search_folder(&mut session, folder, &crit, local).await {
                        Ok(h) => hits.extend(h),
                        Err(e) if e.is_connection_error() => return Err(e),
                        Err(e) => log_warn!("account {}: server search in {} failed: {e}", account.id, folder.path),
                    }
                }
                let _ = session.logout().await;
                Ok::<_, CoreError>(hits)
            });
        }
        if jobs.is_empty() {
            return Ok(Vec::new());
        }
        let started = std::time::Instant::now();
        let results = self.rt.block_on(futures::future::join_all(jobs));
        let mut hits = Vec::new();
        let mut first_err = None;
        let mut ok = 0;
        for r in results {
            match r {
                Ok(h) => {
                    ok += 1;
                    hits.extend(h);
                }
                Err(e) => {
                    log_warn!("server search failed: {e}");
                    first_err.get_or_insert(e);
                }
            }
        }
        if ok == 0 {
            if let Some(e) = first_err {
                return Err(e);
            }
        }
        hits.sort_by(|a, b| b.summary.date.cmp(&a.summary.date));
        hits.truncate(server_search::MAX_RESULTS);
        log_info!("server search: {} results in {:.1}s", hits.len(), started.elapsed().as_secs_f64());
        Ok(self.remote.lock().unwrap().insert(hits))
    }

    // ------------------------------------------------------------ sync

    pub fn sync_now(&self, account_id: Option<i64>) {
        let workers = self.workers.lock().unwrap();
        for (id, w) in workers.iter() {
            if account_id.is_none() || account_id == Some(*id) {
                w.send(Cmd::SyncAll);
            }
        }
    }

    /// Call after wake from sleep or when the network path changes.
    pub fn network_changed(&self) {
        let workers = self.workers.lock().unwrap();
        for w in workers.values() {
            w.restart_idle();
            w.send(Cmd::SyncAll);
        }
    }

    pub fn sync_phase(&self, account_id: i64) -> SyncPhase {
        self.workers
            .lock()
            .unwrap()
            .get(&account_id)
            .map(|w| w.phase())
            .unwrap_or(SyncPhase::Idle { last_sync: 0 })
    }

    pub fn smart_counts(&self) -> SmartCounts {
        let (inbox_unread, flagged) = self.ctx.store.smart_counts().unwrap_or((0, 0));
        SmartCounts { inbox_unread, flagged }
    }

    pub fn storage(&self, account_id: Option<i64>) -> StorageInfo {
        self.ctx.store.storage(account_id).unwrap_or(StorageInfo { messages: 0, bytes: 0 })
    }

    pub fn shutdown(&self) {
        let workers: Vec<Worker> = self.workers.lock().unwrap().drain().map(|(_, w)| w).collect();
        self.rt.block_on(async {
            for w in workers {
                w.stop_and_wait().await;
            }
        });
    }
}
