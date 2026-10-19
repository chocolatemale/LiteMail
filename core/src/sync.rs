//! Per-account sync worker.
//!
//! One task per account owns the main IMAP connection and processes commands in order:
//! queued user actions are replayed first, then folders are reconciled against the
//! "keep the last N days" window. A second connection IDLEs on the inbox and pokes the
//! worker when the server reports changes.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep, sleep_until, timeout};

use crate::error::{CoreError, Result};
use crate::imap::{self, COMMAND_TIMEOUT, Caps, Session, imap_date, quote, simple, uid_chunks, uid_set};
use crate::secrets::SecretStore;
use crate::store::{FolderRow, LocalFlag, NewMessage, Op, OpKind, Store};
use crate::types::{CoreEvent, FolderRole, SyncPhase};
use crate::{CoreListener, log_info, log_warn, mime};

/// Messages per FETCH and bytes per FETCH; keeps memory flat while downloading.
const BATCH_MESSAGES: usize = 50;
const BATCH_BYTES: u64 = 8 * 1024 * 1024;
/// Close the main connection after this long without work.
const IDLE_CLOSE: Duration = Duration::from_secs(120);
const IDLE_RENEW: Duration = Duration::from_secs(25 * 60);

pub struct Ctx {
    pub store: Arc<Store>,
    pub secrets: Arc<dyn SecretStore>,
    pub listener: Arc<dyn CoreListener>,
}

impl Ctx {
    pub fn emit(&self, e: CoreEvent) {
        self.listener.on_event(e);
    }
}

#[derive(Debug)]
pub enum Cmd {
    /// Full sync requested by the user or after a settings change.
    SyncAll,
    /// Periodic full sync (skipped while the password is known to be wrong).
    Tick,
    SyncFolder(i64),
    FlushOps,
    /// Save a message to a folder unless the server already stored it (same Message-ID).
    Append { folder_id: i64, raw: Vec<u8>, message_id: String, flags: &'static str },
    /// Drop the connection and retry login (password changed).
    Reconnect,
    Stop,
}

pub struct Worker {
    tx: mpsc::UnboundedSender<Cmd>,
    pub phase: Arc<Mutex<SyncPhase>>,
    main: JoinHandle<()>,
    idle: Mutex<Option<JoinHandle<()>>>,
    ctx: Arc<Ctx>,
    account_id: i64,
    rt: tokio::runtime::Handle,
}

impl Worker {
    pub fn spawn(rt: &tokio::runtime::Handle, ctx: Arc<Ctx>, account_id: i64) -> Worker {
        let (tx, rx) = mpsc::unbounded_channel();
        let phase = Arc::new(Mutex::new(SyncPhase::Idle { last_sync: 0 }));
        let main = rt.spawn(run(ctx.clone(), account_id, rx, phase.clone()));
        let idle = rt.spawn(idle_loop(ctx.clone(), account_id, tx.clone()));
        let _ = tx.send(Cmd::SyncAll);
        Worker { tx, phase, main, idle: Mutex::new(Some(idle)), ctx, account_id, rt: rt.clone() }
    }

    pub fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }

    pub fn phase(&self) -> SyncPhase {
        self.phase.lock().unwrap().clone()
    }

    /// Restart the IDLE connection (after wake from sleep or a network change).
    pub fn restart_idle(&self) {
        let mut g = self.idle.lock().unwrap();
        if let Some(h) = g.take() {
            h.abort();
        }
        *g = Some(self.rt.spawn(idle_loop(self.ctx.clone(), self.account_id, self.tx.clone())));
    }

    pub fn stop(self) {
        let _ = self.tx.send(Cmd::Stop);
        if let Some(h) = self.idle.lock().unwrap().take() {
            h.abort();
        }
        drop(self.main);
    }

    pub async fn stop_and_wait(self) {
        let _ = self.tx.send(Cmd::Stop);
        if let Some(h) = self.idle.lock().unwrap().take() {
            h.abort();
        }
        let _ = timeout(Duration::from_secs(10), self.main).await;
    }
}

#[derive(Default)]
struct Plan {
    full: bool,
    manual: bool,
    folders: BTreeSet<i64>,
    appends: Vec<(i64, Vec<u8>, String, &'static str)>,
    reconnect: bool,
    stop: bool,
}

impl Plan {
    fn add(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::SyncAll => {
                self.full = true;
                self.manual = true;
            }
            Cmd::Tick => self.full = true,
            Cmd::SyncFolder(id) => {
                self.folders.insert(id);
            }
            Cmd::FlushOps => {}
            Cmd::Append { folder_id, raw, message_id, flags } => self.appends.push((folder_id, raw, message_id, flags)),
            Cmd::Reconnect => {
                self.reconnect = true;
                self.full = true;
                self.manual = true;
            }
            Cmd::Stop => self.stop = true,
        }
    }

    fn is_empty_work(&self) -> bool {
        !self.full && self.folders.is_empty() && self.appends.is_empty()
    }
}

fn set_phase(ctx: &Ctx, account_id: i64, phase: &Mutex<SyncPhase>, p: SyncPhase) {
    let mut g = phase.lock().unwrap();
    if *g != p {
        *g = p.clone();
        drop(g);
        ctx.emit(CoreEvent::SyncState { account_id, phase: p });
    }
}

/// First moment kept on this Mac for a "last N days" window (None: keep everything).
pub fn window_start(sync_days: u32) -> Option<i64> {
    (sync_days > 0).then(|| now() - sync_days as i64 * 86_400)
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

async fn open_session(ctx: &Ctx, account_id: i64) -> Result<(Session, Caps)> {
    let account = ctx.store.account(account_id)?;
    let password = ctx.secrets.get(account_id)?;
    imap::login(&account.imap, account.allow_invalid_certs, &account.username, &password).await
}

async fn run(ctx: Arc<Ctx>, account_id: i64, mut rx: mpsc::UnboundedReceiver<Cmd>, phase: Arc<Mutex<SyncPhase>>) {
    let mut conn: Option<(Session, Caps)> = None;
    let mut next_tick = Instant::now() + Duration::from_secs(3600);
    let mut close_at = Instant::now() + IDLE_CLOSE;
    let mut backoff = Duration::from_secs(15);
    let mut auth_failed = false;
    let mut carry = Plan::default();

    loop {
        let first = tokio::select! {
            c = rx.recv() => match c { Some(c) => Some(c), None => return },
            _ = sleep_until(next_tick) => Some(Cmd::Tick),
            _ = sleep_until(close_at), if conn.is_some() => {
                if let Some((mut s, _)) = conn.take() {
                    let _ = timeout(Duration::from_secs(5), s.logout()).await;
                }
                None
            }
        };
        let Some(first) = first else { continue };

        let mut plan = std::mem::take(&mut carry);
        plan.add(first);
        while let Ok(c) = rx.try_recv() {
            plan.add(c);
        }
        if plan.stop {
            if let Some((mut s, _)) = conn.take() {
                let _ = timeout(Duration::from_secs(5), s.logout()).await;
            }
            return;
        }
        let Ok(account) = ctx.store.account(account_id) else { return };
        let interval = Duration::from_secs(account.sync_interval_minutes.max(1) as u64 * 60);
        if plan.reconnect {
            conn = None;
            auth_failed = false;
        }
        if auth_failed && !plan.manual {
            // Don't hammer the server with a wrong password; wait for the user.
            next_tick = Instant::now() + interval;
            continue;
        }
        let has_ops = !ctx.store.ops(account_id).map(|o| o.is_empty()).unwrap_or(true);
        if plan.is_empty_work() && !has_ops {
            continue;
        }

        let started = Instant::now();
        let mut attempt = 0;
        loop {
            attempt += 1;
            if conn.is_none() {
                if plan.full {
                    set_phase(&ctx, account_id, &phase, SyncPhase::Connecting);
                }
                match open_session(&ctx, account_id).await {
                    Ok(c) => conn = Some(c),
                    Err(CoreError::Auth { message }) => {
                        log_warn!("account {account_id}: login failed: {message}");
                        auth_failed = true;
                        set_phase(&ctx, account_id, &phase, SyncPhase::AuthFailed { message });
                        next_tick = Instant::now() + interval;
                        break;
                    }
                    Err(e) => {
                        log_warn!("account {account_id}: connect failed: {e}");
                        set_phase(&ctx, account_id, &phase, SyncPhase::Offline { message: e.to_string() });
                        carry = plan;
                        next_tick = Instant::now() + backoff;
                        backoff = (backoff * 2).min(Duration::from_secs(300));
                        break;
                    }
                }
            }
            auth_failed = false;
            if plan.full || plan.manual {
                set_phase(&ctx, account_id, &phase, SyncPhase::Checking);
            }
            log_info!(
                "account {account_id}: run full={} folders={} appends={} attempt={attempt}",
                plan.full,
                plan.folders.len(),
                plan.appends.len()
            );
            let (session, caps) = conn.as_mut().unwrap();
            let caps = caps.clone();
            match execute(&ctx, account_id, session, &caps, &mut plan, &phase).await {
                Ok(()) => {
                    log_info!("account {account_id}: sync done in {:.1}s", started.elapsed().as_secs_f64());
                    backoff = Duration::from_secs(15);
                    if plan.full {
                        next_tick = Instant::now() + interval;
                    }
                    set_phase(&ctx, account_id, &phase, SyncPhase::Idle { last_sync: now() });
                    break;
                }
                Err(e) if e.is_connection_error() => {
                    log_info!("account {account_id}: connection error: {e}");
                    conn = None;
                    if attempt < 2 {
                        // The kept-alive connection may have been closed by the server.
                        continue;
                    }
                    log_warn!("account {account_id}: sync failed: {e}");
                    set_phase(&ctx, account_id, &phase, SyncPhase::Offline { message: e.to_string() });
                    carry = plan;
                    next_tick = Instant::now() + backoff;
                    backoff = (backoff * 2).min(Duration::from_secs(300));
                    break;
                }
                Err(e) => {
                    log_warn!("account {account_id}: sync error: {e}");
                    set_phase(&ctx, account_id, &phase, SyncPhase::Idle { last_sync: now() });
                    if plan.full {
                        next_tick = Instant::now() + interval;
                    }
                    break;
                }
            }
        }
        close_at = Instant::now() + IDLE_CLOSE;
    }
}

async fn execute(
    ctx: &Ctx,
    account_id: i64,
    session: &mut Session,
    caps: &Caps,
    plan: &mut Plan,
    phase: &Mutex<SyncPhase>,
) -> Result<()> {
    let touched = flush_ops(ctx, account_id, session, caps).await?;
    plan.folders.extend(touched);

    while let Some((folder_id, raw, message_id, flags)) = plan.appends.first().cloned() {
        if let Ok(folder) = ctx.store.folder(folder_id) {
            if already_saved(session, &folder.name, &message_id).await? {
                plan.folders.insert(folder_id);
                plan.appends.remove(0);
                continue;
            }
            match timeout(COMMAND_TIMEOUT, session.append(&folder.name, Some(flags), None, &raw)).await? {
                Ok(()) => {
                    plan.folders.insert(folder_id);
                }
                Err(e) => {
                    let e: CoreError = e.into();
                    if e.is_connection_error() {
                        return Err(e);
                    }
                    log_warn!("append to {} failed: {e}", folder.name);
                }
            }
        }
        plan.appends.remove(0);
    }

    let initial_done = ctx.store.initial_sync_done(account_id);
    let mut folders: Vec<FolderRow> = Vec::new();
    if plan.full {
        let list = imap::list_folders(session).await?;
        ctx.store.sync_folder_list(account_id, &list)?;
        ctx.emit(CoreEvent::FoldersChanged { account_id });
        folders = ctx.store.sync_folders(account_id)?;
    } else {
        for id in &plan.folders {
            if let Ok(f) = ctx.store.folder(*id) {
                if f.account_id == account_id && f.sync_enabled {
                    folders.push(f);
                }
            }
        }
    }

    let mut new_mail: Vec<i64> = Vec::new();
    for folder in &folders {
        match sync_folder(ctx, account_id, session, caps, folder, phase, plan.full).await {
            Ok(outcome) => {
                if outcome.changed {
                    ctx.emit(CoreEvent::MessagesChanged { account_id });
                    ctx.emit(CoreEvent::FoldersChanged { account_id });
                }
                new_mail.extend(outcome.new_unseen_inbox);
            }
            Err(e) if e.is_connection_error() => return Err(e),
            Err(e) => log_warn!("folder {} skipped: {e}", folder.name),
        }
    }
    plan.folders.clear();

    if plan.full && !initial_done {
        ctx.store.set_initial_sync_done(account_id)?;
    } else if !new_mail.is_empty() && initial_done {
        notify_new_mail(ctx, account_id, &new_mail);
    }
    Ok(())
}

/// Some servers (Gmail, many others) file sent mail themselves; don't store it twice.
async fn already_saved(session: &mut Session, folder: &str, message_id: &str) -> Result<bool> {
    if message_id.is_empty() || message_id.contains(['"', '\\', '\r', '\n']) {
        return Ok(false);
    }
    let lenient = |e: CoreError| if e.is_connection_error() { Err(e) } else { Ok(false) };
    if let Err(e) = imap::select(session, folder, false).await {
        return lenient(e);
    }
    match imap::uid_search(session, &format!("HEADER Message-ID \"<{message_id}>\"")).await {
        Ok(uids) => Ok(!uids.is_empty()),
        Err(e) => lenient(e),
    }
}

fn notify_new_mail(ctx: &Ctx, account_id: i64, ids: &[i64]) {
    let Ok(account) = ctx.store.account(account_id) else { return };
    if !account.notify {
        return;
    }
    let Some(&latest) = ids.iter().max() else { return };
    let Ok((_, raw)) = ctx.store.read_raw(latest) else { return };
    let p = mime::parse(&raw);
    let from = if p.from.name.is_empty() { p.from.address.clone() } else { p.from.name.clone() };
    ctx.emit(CoreEvent::NewMail { account_id, count: ids.len() as u32, message_id: latest, from, subject: p.subject });
}

struct Outcome {
    changed: bool,
    new_unseen_inbox: Vec<i64>,
}

fn label(folder: &FolderRow) -> String {
    match folder.role {
        FolderRole::Inbox => crate::store::special_name(FolderRole::Inbox).unwrap_or_default(),
        _ => {
            let d = crate::utf7::decode(&folder.name);
            d.rsplit(['/', '.']).next().unwrap_or(&d).to_string()
        }
    }
}

async fn sync_folder(
    ctx: &Ctx,
    account_id: i64,
    session: &mut Session,
    caps: &Caps,
    folder: &FolderRow,
    phase: &Mutex<SyncPhase>,
    report_progress: bool,
) -> Result<Outcome> {
    let store = &ctx.store;
    let mbox = imap::select(session, &folder.name, caps.condstore).await?;
    let mut known_modseq = folder.highest_modseq;
    let mut changed = false;
    if folder.uidvalidity.is_some() && mbox.uidvalidity != folder.uidvalidity {
        log_info!("{}: UIDVALIDITY changed, refetching", folder.name);
        store.wipe_folder(account_id, folder.id)?;
        known_modseq = None;
        changed = true;
    }
    let uidvalidity = mbox.uidvalidity.unwrap_or(0);

    let account = store.account(account_id)?;
    let query = if account.sync_days == 0 {
        "ALL".to_string()
    } else {
        format!("SINCE {}", imap_date(window_start(account.sync_days).unwrap_or_default()))
    };
    let server: HashSet<u32> = if mbox.exists == 0 { HashSet::new() } else { imap::uid_search(session, &query).await? };

    let local = store.local_uids(folder.id)?;
    let (hidden, pending_flags) = store.pending_uids(folder.id)?;

    // 1. Gone from the server, or older than the window: drop the local copy.
    let gone: Vec<u32> = local.keys().filter(|u| !server.contains(u)).copied().collect();
    if store.delete_uids(folder.id, &gone)? > 0 {
        changed = true;
    }

    // 2. Flag changes on messages we already have.
    let existing: Vec<u32> =
        local.keys().filter(|u| server.contains(u) && !pending_flags.contains(u)).copied().collect();
    let unchanged = caps.condstore && mbox.highest_modseq.is_some() && known_modseq == mbox.highest_modseq;
    if !existing.is_empty() && !unchanged {
        let query = match (caps.condstore, known_modseq) {
            (true, Some(ms)) => format!("(UID FLAGS) (CHANGEDSINCE {ms})"),
            _ => "(UID FLAGS)".to_string(),
        };
        let wanted: HashSet<u32> = existing.iter().copied().collect();
        let mut updates = Vec::new();
        for chunk in uid_chunks(&existing, 2000) {
            let set = if known_modseq.is_some() && caps.condstore { "1:*".to_string() } else { uid_set(&chunk) };
            for f in imap::uid_fetch(session, &set, &query).await? {
                if let (Some(uid), Some(flags)) = (f.uid, f.flags) {
                    if wanted.contains(&uid) {
                        updates.push((uid, flags));
                    }
                }
            }
            if set == "1:*" {
                break;
            }
        }
        if store.update_flags(folder.id, &updates)? > 0 {
            changed = true;
        }
    }

    // 3. New messages, newest first, in batches bounded by count and bytes.
    let mut new: Vec<u32> =
        server.iter().filter(|u| !local.contains_key(u) && !hidden.contains(u)).copied().collect();
    new.sort_unstable_by(|a, b| b.cmp(a));
    let mut new_unseen_inbox = Vec::new();
    if !new.is_empty() {
        let mut sizes: HashMap<u32, u64> = HashMap::new();
        for chunk in uid_chunks(&new, 2000) {
            for f in imap::uid_fetch(session, &uid_set(&chunk), "(UID RFC822.SIZE)").await? {
                if let (Some(uid), Some(size)) = (f.uid, f.size) {
                    sizes.insert(uid, size as u64);
                }
            }
        }
        let mut batches: Vec<Vec<u32>> = Vec::new();
        let mut cur: Vec<u32> = Vec::new();
        let mut cur_bytes = 0u64;
        for uid in &new {
            let size = sizes.get(uid).copied().unwrap_or(0);
            if !cur.is_empty() && (cur.len() >= BATCH_MESSAGES || cur_bytes + size > BATCH_BYTES) {
                batches.push(std::mem::take(&mut cur));
                cur_bytes = 0;
            }
            cur.push(*uid);
            cur_bytes += size;
        }
        if !cur.is_empty() {
            batches.push(cur);
        }

        let total = new.len() as u32;
        let mut done = 0u32;
        let name = label(folder);
        let recent_cutoff = now() - 2 * 86_400;
        for (bi, batch) in batches.iter().enumerate() {
            if report_progress || total > 20 {
                set_phase(ctx, account_id, phase, SyncPhase::Syncing { folder: name.clone(), done, total });
            }
            let fetched = imap::uid_fetch(session, &uid_set(batch), "(UID FLAGS INTERNALDATE RFC822.SIZE BODY.PEEK[])").await?;
            for f in fetched {
                let (Some(uid), Some(body)) = (f.uid, f.body.as_deref()) else { continue };
                let flags = f.flags.unwrap_or_default();
                let internal_date = f.internal_date.unwrap_or_else(now);
                let parsed = mime::parse(body);
                let id = store.insert_message(&NewMessage {
                    account_id,
                    folder_id: folder.id,
                    uid,
                    uidvalidity,
                    flags,
                    internal_date,
                    size: f.size.map(|s| s as u64).unwrap_or(body.len() as u64),
                    parsed: &parsed,
                    raw: body,
                })?;
                done += 1;
                changed = true;
                if folder.role == FolderRole::Inbox && !flags.seen && internal_date > recent_cutoff {
                    new_unseen_inbox.push(id);
                }
            }
            // Show messages as they arrive: first batch right away, then every few batches.
            if bi == 0 || bi % 4 == 3 {
                ctx.emit(CoreEvent::MessagesChanged { account_id });
                ctx.emit(CoreEvent::FoldersChanged { account_id });
            }
        }
    }

    store.set_folder_state(folder.id, Some(uidvalidity), mbox.highest_modseq)?;
    Ok(Outcome { changed, new_unseen_inbox })
}

/// Replays queued user actions. Returns folders that should be re-synced.
async fn flush_ops(ctx: &Ctx, account_id: i64, session: &mut Session, caps: &Caps) -> Result<Vec<i64>> {
    let ops = ctx.store.ops(account_id)?;
    if ops.is_empty() {
        return Ok(Vec::new());
    }
    // Group consecutive ops with the same folder and kind into one command.
    let mut groups: Vec<Vec<Op>> = Vec::new();
    for op in ops {
        match groups.last_mut() {
            Some(g) if g[0].folder_id == op.folder_id && g[0].kind == op.kind => g.push(op),
            _ => groups.push(vec![op]),
        }
    }
    let mut touched = Vec::new();
    let mut selected: Option<i64> = None;
    for group in groups {
        let first = &group[0];
        let Ok(folder) = ctx.store.folder(first.folder_id) else {
            for op in &group {
                ctx.store.delete_op(op.id)?;
            }
            continue;
        };
        if selected != Some(folder.id) {
            match imap::select(session, &folder.name, false).await {
                Ok(_) => selected = Some(folder.id),
                Err(e) => {
                    if e.is_connection_error() {
                        return Err(e);
                    }
                    for op in &group {
                        ctx.store.delete_op(op.id)?;
                    }
                    continue;
                }
            }
        }
        let uids: Vec<u32> = group.iter().map(|o| o.uid).collect();
        let set = uid_set(&uids);
        let result = apply_op(ctx, session, caps, &first.kind, &set).await;
        match result {
            Ok(target) => {
                for op in &group {
                    ctx.store.delete_op(op.id)?;
                }
                if let Some(t) = target {
                    touched.push(t);
                }
            }
            Err(e) if e.is_connection_error() => return Err(e),
            Err(e) => {
                log_warn!("op {:?} on {} failed: {e}", first.kind, folder.name);
                for op in &group {
                    if op.attempts >= 2 {
                        ctx.store.delete_op(op.id)?;
                    } else {
                        ctx.store.bump_op(op.id)?;
                    }
                }
            }
        }
    }
    Ok(touched)
}

fn flag_name(f: LocalFlag) -> &'static str {
    match f {
        LocalFlag::Seen => "\\Seen",
        LocalFlag::Flagged => "\\Flagged",
        LocalFlag::Answered => "\\Answered",
    }
}

async fn store_flags(session: &mut Session, set: &str, query: &str) -> Result<()> {
    simple(session, &format!("UID STORE {set} {query}")).await
}

async fn expunge(session: &mut Session, caps: &Caps, set: &str) -> Result<()> {
    if caps.uidplus { simple(session, &format!("UID EXPUNGE {set}")).await } else { simple(session, "EXPUNGE").await }
}

async fn apply_op(ctx: &Ctx, session: &mut Session, caps: &Caps, kind: &OpKind, set: &str) -> Result<Option<i64>> {
    match kind {
        OpKind::AddFlag(f) => {
            store_flags(session, set, &format!("+FLAGS.SILENT ({})", flag_name(*f))).await?;
            Ok(None)
        }
        OpKind::RemoveFlag(f) => {
            store_flags(session, set, &format!("-FLAGS.SILENT ({})", flag_name(*f))).await?;
            Ok(None)
        }
        OpKind::Move { target_folder_id } => {
            let target = ctx.store.folder(*target_folder_id)?;
            if caps.moves {
                simple(session, &format!("UID MOVE {set} {}", quote(&target.name))).await?;
            } else {
                simple(session, &format!("UID COPY {set} {}", quote(&target.name))).await?;
                store_flags(session, set, "+FLAGS.SILENT (\\Deleted)").await?;
                expunge(session, caps, set).await?;
            }
            Ok(Some(target.id))
        }
        OpKind::Delete => {
            store_flags(session, set, "+FLAGS.SILENT (\\Deleted)").await?;
            expunge(session, caps, set).await?;
            Ok(None)
        }
    }
}

/// Keeps a second connection in IDLE on the inbox and asks the worker to sync on changes.
async fn idle_loop(ctx: Arc<Ctx>, account_id: i64, tx: mpsc::UnboundedSender<Cmd>) {
    let mut backoff = Duration::from_secs(30);
    // Let the first full sync go first.
    sleep(Duration::from_secs(5)).await;
    loop {
        if ctx.store.account(account_id).is_err() {
            return;
        }
        match idle_session(&ctx, account_id, &tx).await {
            Ok(false) => return, // server has no IDLE; periodic sync covers it
            Ok(true) => backoff = Duration::from_secs(30),
            Err(CoreError::Auth { .. }) => sleep(Duration::from_secs(600)).await,
            Err(e) => {
                log_info!("account {account_id}: idle reconnect in {:?}: {e}", backoff);
                sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(300));
            }
        }
    }
}

async fn idle_session(ctx: &Ctx, account_id: i64, tx: &mpsc::UnboundedSender<Cmd>) -> Result<bool> {
    let (mut session, caps) = open_session(ctx, account_id).await?;
    if !caps.idle {
        let _ = session.logout().await;
        return Ok(false);
    }
    let inbox = ctx.store.folder_by_role(account_id, FolderRole::Inbox)?;
    let name = inbox.as_ref().map(|f| f.name.clone()).unwrap_or_else(|| "INBOX".into());
    imap::select(&mut session, &name, false).await?;
    loop {
        let mut handle = session.idle();
        timeout(COMMAND_TIMEOUT, handle.init()).await??;
        let (wait, _stop) = handle.wait_with_timeout(IDLE_RENEW);
        let res = wait.await;
        session = timeout(COMMAND_TIMEOUT, handle.done()).await??;
        match res? {
            async_imap::extensions::idle::IdleResponse::NewData(_) => {
                // Coalesce bursts (a new message often arrives as EXISTS + FETCH + RECENT).
                sleep(Duration::from_millis(800)).await;
                let id = match &inbox {
                    Some(f) => f.id,
                    None => match ctx.store.folder_by_role(account_id, FolderRole::Inbox)? {
                        Some(f) => f.id,
                        None => continue,
                    },
                };
                if tx.send(Cmd::SyncFolder(id)).is_err() {
                    return Ok(true);
                }
            }
            _ => {}
        }
    }
}
