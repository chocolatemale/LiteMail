//! End-to-end tests against a real IMAP server (Dovecot) and an SMTP sink.
//! Start them with `scripts/test.sh`; without LITEMAIL_IT_ROOT these tests are skipped.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use futures::TryStreamExt;
use litemail_core::error::CoreError;
use litemail_core::secrets::MemorySecrets;
use litemail_core::types::*;
use litemail_core::{CoreListener, MailCore};

const IMAP: u16 = 31143;
const IMAPS: u16 = 31993;
const IMAP_STARTTLS: u16 = 31144;
const SMTP: u16 = 31587;
const SMTPS: u16 = 31465;

fn server_root() -> Option<PathBuf> {
    std::env::var("LITEMAIL_IT_ROOT").ok().map(PathBuf::from)
}

macro_rules! need_server {
    () => {
        match server_root() {
            Some(r) => r,
            None => {
                eprintln!("skipped: start the test servers with scripts/test.sh");
                return;
            }
        }
    };
}

fn unique_user(tag: &str) -> String {
    static N: AtomicU32 = AtomicU32::new(0);
    let nanos = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_nanos() % 1_000_000_000;
    format!("{tag}{}x{}x{}@localhost", std::process::id(), N.fetch_add(1, Ordering::Relaxed), nanos)
}

#[derive(Default)]
struct Events(Mutex<Vec<CoreEvent>>);

impl CoreListener for Events {
    fn on_event(&self, event: CoreEvent) {
        self.0.lock().unwrap().push(event);
    }
}

impl Events {
    fn new_mail_count(&self, account_id: i64) -> usize {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|e| matches!(e, CoreEvent::NewMail { account_id: a, .. } if *a == account_id))
            .count()
    }
}

fn setup_for(email: &str, imap: (u16, Security), smtp: (u16, Security), days: u32) -> AccountSetup {
    AccountSetup {
        email: email.into(),
        display_name: email.split('@').next().unwrap().into(),
        provider: ProviderKind::Other,
        imap: ServerConfig { host: "127.0.0.1".into(), port: imap.0, security: imap.1 },
        smtp: ServerConfig { host: "127.0.0.1".into(), port: smtp.0, security: smtp.1 },
        username: email.into(),
        password: "test".into(),
        sync_days: days,
        allow_invalid_certs: true,
    }
}

fn plain(email: &str, days: u32) -> AccountSetup {
    setup_for(email, (IMAP, Security::None), (SMTP, Security::None), days)
}

struct Harness {
    _dir: tempfile::TempDir,
    data: PathBuf,
    core: Arc<MailCore>,
    events: Arc<Events>,
}

impl Harness {
    fn new() -> Harness {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().to_path_buf();
        let events = Arc::new(Events::default());
        let core = MailCore::with_secrets(data.clone(), events.clone(), Arc::new(MemorySecrets::default())).unwrap();
        Harness { _dir: dir, data, core, events }
    }

    fn add(&self, setup: AccountSetup) -> Account {
        let a = self.core.add_account(setup).expect("add account");
        assert!(wait(20, || self.folder(a.id, FolderRole::Inbox).is_some() && self.idle(a.id)), "initial sync");
        a
    }

    fn idle(&self, account_id: i64) -> bool {
        matches!(self.core.sync_phase(account_id), SyncPhase::Idle { last_sync } if last_sync > 0)
    }

    fn folder(&self, account_id: i64, role: FolderRole) -> Option<Folder> {
        self.core.folders(account_id).into_iter().find(|f| f.role == role)
    }

    fn fid(&self, account_id: i64, role: FolderRole) -> i64 {
        self.folder(account_id, role).unwrap_or_else(|| panic!("no {role:?} folder")).id
    }

    fn list(&self, mailbox: Mailbox, search: Option<&str>) -> Vec<MessageSummary> {
        self.core.messages(MessageQuery { mailbox, search: search.map(Into::into), limit: 5000, before_date: None, before_id: None })
    }

    fn subjects(&self, folder_id: i64) -> Vec<String> {
        self.list(Mailbox::Folder { id: folder_id }, None).into_iter().map(|m| m.subject).collect()
    }

    fn find(&self, folder_id: i64, subject: &str) -> Option<MessageSummary> {
        self.list(Mailbox::Folder { id: folder_id }, None).into_iter().find(|m| m.subject == subject)
    }

    fn eml_files(&self) -> usize {
        fn walk(p: &Path) -> usize {
            let Ok(rd) = std::fs::read_dir(p) else { return 0 };
            rd.flatten()
                .map(|e| {
                    let p = e.path();
                    if p.is_dir() { walk(&p) } else if p.extension().is_some_and(|x| x == "eml") { 1 } else { 0 }
                })
                .sum()
        }
        walk(&self.data.join("mail"))
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.core.shutdown();
        if std::thread::panicking() {
            let log = std::fs::read_to_string(self.data.join("logs/core.log")).unwrap_or_default();
            eprintln!("---- core.log ----\n{log}");
        }
    }
}

fn wait(secs: u64, f: impl Fn() -> bool) -> bool {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    f()
}

fn mail(subject: &str, body: &str, from: &str) -> String {
    format!(
        "From: {from}\r\nTo: me@localhost\r\nSubject: {subject}\r\nDate: Sun, 18 Oct 2026 09:41:00 +0800\r\n\
         Message-ID: <{}@test>\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{body}\r\n",
        unique_user("m").replace('@', ".")
    )
}

/// Drops a message straight into the user's Maildir with a chosen received date.
fn seed(root: &Path, user: &str, folder: &str, raw: &str, days_ago: f64, flags: &str) {
    static N: AtomicU32 = AtomicU32::new(0);
    let base = root.join("home").join(user).join("Maildir");
    let dir = if folder == "INBOX" { base } else { base.join(format!(".{folder}")) };
    for sub in ["cur", "new", "tmp"] {
        std::fs::create_dir_all(dir.join(sub)).unwrap();
    }
    let name = format!("{}.S{}.seed:2,{flags}", 1_700_000_000 + N.fetch_add(1, Ordering::Relaxed), std::process::id());
    let path = dir.join("cur").join(name);
    std::fs::write(&path, raw.replace("\r\n", "\n").replace('\n', "\r\n")).unwrap();
    let when = SystemTime::now() - Duration::from_secs_f64(days_ago * 86_400.0);
    std::fs::File::options().write(true).open(&path).unwrap().set_modified(when).unwrap();
}

/// Runs a closure against a fresh plaintext IMAP session for `user`.
fn on_server<T>(user: &str, f: impl for<'a> FnOnce(&'a mut litemail_core::imap::Session) -> futures::future::BoxFuture<'a, T>) -> T {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(async {
        let cfg = ServerConfig { host: "127.0.0.1".into(), port: IMAP, security: Security::None };
        let (mut s, _) = litemail_core::imap::login(&cfg, false, user, "test").await.unwrap();
        let out = f(&mut s).await;
        let _ = s.logout().await;
        out
    })
}

fn server_search(user: &str, folder: &str, query: &str) -> HashSet<u32> {
    let (folder, query) = (folder.to_string(), query.to_string());
    on_server(user, move |s| {
        Box::pin(async move {
            s.select(&folder).await.unwrap();
            s.uid_search(&query).await.unwrap()
        })
    })
}

// --------------------------------------------------------------------------------------

#[test]
fn connects_with_tls_and_starttls_and_rejects_bad_password() {
    let _root = need_server!();
    let h = Harness::new();
    let u = unique_user("tls");
    h.core.verify_account(setup_for(&u, (IMAPS, Security::Tls), (SMTPS, Security::Tls), 30)).expect("implicit TLS");
    h.core
        .verify_account(setup_for(&u, (IMAP_STARTTLS, Security::StartTls), (SMTP, Security::StartTls), 30))
        .expect("STARTTLS");
    // A pasted password with a trailing line break (or spaces) still signs in.
    let mut pasted = plain(&u, 30);
    pasted.password = " test\n".into();
    h.core.verify_account(pasted.clone()).expect("pasted password with newline");
    let added = h.core.add_account(pasted).expect("add with pasted password");
    h.core.update_password(added.id, "test\r\n".into()).expect("update with pasted password");
    let mut bad = plain(&u, 30);
    bad.password = "wrong".into();
    // Dovecot accepts any password in the static passdb? No: only "test".
    match h.core.verify_account(bad) {
        Err(CoreError::Auth { .. }) => {}
        other => panic!("expected auth error, got {other:?}"),
    }
    // Self-signed certificates are refused unless explicitly allowed (and only for localhost).
    let mut strict = setup_for(&u, (IMAPS, Security::Tls), (SMTPS, Security::Tls), 30);
    strict.allow_invalid_certs = false;
    assert!(matches!(h.core.verify_account(strict), Err(CoreError::Network { .. })));
}

#[test]
fn keeps_exactly_the_last_n_days() {
    let root = need_server!();
    let u = unique_user("win");
    for (s, days) in [("A", 0.5), ("B", 5.0), ("C", 20.0), ("D", 45.0)] {
        seed(&root, &u, "INBOX", &mail(s, "x", "x@localhost"), days, "S");
    }
    let h = Harness::new();
    let a = h.add(plain(&u, 30));
    let inbox = h.fid(a.id, FolderRole::Inbox);
    assert_eq!(h.subjects(inbox), vec!["A", "B", "C"], "30-day window");
    assert_eq!(h.eml_files(), 3);

    let settings = |days| AccountSettings {
        display_name: a.display_name.clone(),
        sync_days: days,
        sync_interval_minutes: 5,
        signature: String::new(),
        notify: true,
    };
    h.core.update_account(a.id, settings(7)).unwrap();
    assert!(wait(15, || h.subjects(inbox) == vec!["A", "B"]), "shrunk to 7 days: {:?}", h.subjects(inbox));
    assert!(wait(5, || h.eml_files() == 2), "pruned files removed from disk");
    assert_eq!(h.core.storage(Some(a.id)).messages, 2);

    h.core.update_account(a.id, settings(0)).unwrap();
    assert!(wait(15, || h.subjects(inbox).len() == 4), "0 = keep everything");
    // Nothing was deleted on the server.
    assert_eq!(server_search(&u, "INBOX", "ALL").len(), 4);
}

#[test]
fn local_actions_reach_the_server_and_server_changes_come_back() {
    let root = need_server!();
    let u = unique_user("act");
    for s in ["Alpha", "Bravo", "Charlie", "Delta"] {
        seed(&root, &u, "INBOX", &mail(s, "body", "x@localhost"), 1.0, "");
    }
    let h = Harness::new();
    let a = h.add(plain(&u, 30));
    let inbox = h.fid(a.id, FolderRole::Inbox);
    let id = |s: &str| h.find(inbox, s).unwrap().id;

    h.core.mark_seen(vec![id("Alpha")], true).unwrap();
    assert!(h.find(inbox, "Alpha").unwrap().seen, "local update is immediate");
    h.core.mark_flagged(vec![id("Bravo")], true).unwrap();
    assert!(wait(10, || server_search(&u, "INBOX", "SEEN").len() == 1), "\\Seen reached server");
    assert!(wait(10, || server_search(&u, "INBOX", "FLAGGED").len() == 1), "\\Flagged reached server");

    h.core.delete_messages(vec![id("Charlie")]).unwrap();
    assert!(h.find(inbox, "Charlie").is_none());
    assert!(wait(10, || server_search(&u, "Trash", "ALL").len() == 1), "moved to Trash on server");
    let trash = h.fid(a.id, FolderRole::Trash);
    assert!(wait(10, || h.find(trash, "Charlie").is_some()), "shows up in local Trash");

    h.core.archive(vec![id("Delta")]).unwrap();
    assert!(wait(10, || server_search(&u, "Archive", "ALL").len() == 1), "archived");
    assert_eq!(server_search(&u, "INBOX", "ALL").len(), 2);

    // Deleting from Trash removes it for good.
    let charlie = h.find(trash, "Charlie").unwrap().id;
    h.core.delete_messages(vec![charlie]).unwrap();
    assert!(wait(10, || server_search(&u, "Trash", "ALL").is_empty()), "expunged");

    // Another client clears the flag; we pick it up (CONDSTORE path).
    on_server(&u, |s| {
        Box::pin(async move {
            s.select("INBOX").await.unwrap();
            s.uid_store("1:*", "-FLAGS.SILENT (\\Flagged)").await.unwrap().try_collect::<Vec<_>>().await.unwrap();
        })
    });
    h.core.sync_now(Some(a.id));
    assert!(wait(10, || !h.find(inbox, "Bravo").unwrap().flagged), "server-side flag change synced");

    // Mark all read.
    h.core.mark_all_seen(Mailbox::Folder { id: inbox }).unwrap();
    assert!(wait(10, || server_search(&u, "INBOX", "UNSEEN").is_empty()));
}

#[test]
fn send_reply_forward_between_two_accounts_with_idle_push() {
    let root = need_server!();
    let alice = unique_user("alice");
    let bob = unique_user("bob");
    let carol = unique_user("carol");
    let h = Harness::new();
    let a = h.add(plain(&alice, 30));
    let b = h.add(setup_for(&bob, (IMAPS, Security::Tls), (SMTPS, Security::Tls), 30));
    let a_inbox = h.fid(a.id, FolderRole::Inbox);
    let b_inbox = h.fid(b.id, FolderRole::Inbox);
    // Let IDLE connections come up.
    std::thread::sleep(Duration::from_secs(6));

    let att = h.data.join("报告.pdf");
    std::fs::write(&att, b"%PDF-1.4 test").unwrap();
    let mut d = h.core.new_draft(a.id).unwrap();
    d.to = format!("Bob <{bob}>");
    d.bcc = carol.clone();
    d.subject = "季度计划 Q4".into();
    d.body = "你好 Bob，\n\n见附件。\n.\n".into();
    d.attachments = vec![att.to_string_lossy().into_owned()];
    h.core.send(d).expect("send");

    // Bob gets it through IDLE, without any manual sync.
    let t0 = Instant::now();
    assert!(wait(20, || h.find(b_inbox, "季度计划 Q4").is_some()), "IDLE push to Bob");
    eprintln!("IDLE delivery took {:?}", t0.elapsed());
    let got = h.find(b_inbox, "季度计划 Q4").unwrap();
    assert!(got.has_attachments);
    let detail = h.core.message(got.id, false).unwrap();
    assert_eq!(detail.attachments.len(), 1);
    assert_eq!(detail.attachments[0].filename, "报告.pdf");
    assert!(detail.html.contains("见附件。"));
    let saved = h.data.join("saved.pdf");
    h.core.save_attachment(got.id, detail.attachments[0].index, saved.to_string_lossy().into_owned()).unwrap();
    assert_eq!(std::fs::read(&saved).unwrap(), b"%PDF-1.4 test");

    // Bcc: delivered to Carol, but no Bcc header anywhere.
    let carol_new = root.join("home").join(&carol).join("Maildir/new");
    assert_eq!(std::fs::read_dir(&carol_new).unwrap().count(), 1, "Bcc recipient got a copy");
    let src = h.core.message_source(got.id).unwrap();
    assert!(!src.to_lowercase().contains("bcc:"), "Bcc header leaked");
    assert!(!src.contains(&carol));

    // Alice's Sent folder has exactly one copy.
    let a_sent = h.fid(a.id, FolderRole::Sent);
    assert!(wait(10, || h.subjects(a_sent) == vec!["季度计划 Q4"]), "saved to Sent");
    h.core.sync_now(Some(a.id));
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(h.subjects(a_sent).len(), 1, "no duplicate in Sent");

    // Bob replies; Alice gets it and gets notified.
    let reply = h.core.reply_draft(got.id, false).unwrap();
    assert_eq!(reply.subject, "Re: 季度计划 Q4");
    assert!(reply.to.contains(&alice));
    let mut reply = reply;
    reply.body = format!("收到！{}", reply.body);
    h.core.send(reply).unwrap();
    assert!(wait(20, || h.find(a_inbox, "Re: 季度计划 Q4").is_some()), "reply arrives");
    assert!(wait(5, || h.events.new_mail_count(a.id) >= 1), "NewMail event for Alice");
    assert!(wait(10, || h.find(b_inbox, "季度计划 Q4").unwrap().answered), "original marked answered");
    assert!(wait(10, || server_search(&bob, "INBOX", "ANSWERED").len() == 1));
    let r = h.find(a_inbox, "Re: 季度计划 Q4").unwrap();
    let r_src = h.core.message_source(r.id).unwrap();
    assert!(r_src.contains("In-Reply-To: <"), "threaded");
    assert!(h.core.message(r.id, false).unwrap().html.contains("<blockquote>"), "quoted original rendered");

    // Bob forwards to Alice: the attachment travels along.
    let mut fwd = h.core.forward_draft(got.id).unwrap();
    fwd.to = alice.clone();
    h.core.send(fwd).unwrap();
    assert!(wait(20, || h.find(a_inbox, "Fwd: 季度计划 Q4").is_some()), "forward arrives");
    let f = h.find(a_inbox, "Fwd: 季度计划 Q4").unwrap();
    let fd = h.core.message(f.id, false).unwrap();
    assert_eq!(fd.attachments.iter().map(|x| x.filename.as_str()).collect::<Vec<_>>(), vec!["报告.pdf"]);

    // Rejected recipient surfaces as an error and nothing is stored.
    let mut bad = h.core.new_draft(a.id).unwrap();
    bad.to = "reject@localhost".into();
    bad.subject = "nope".into();
    assert!(matches!(h.core.send(bad), Err(CoreError::Server { .. })));
}

/// A TCP proxy we can cut to simulate going offline.
struct Proxy {
    rt: tokio::runtime::Runtime,
    port: u16,
    tasks: Arc<Mutex<Vec<tokio::task::AbortHandle>>>,
}

impl Proxy {
    fn new() -> Proxy {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap();
        let port = rt.block_on(async { tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap().port() });
        let p = Proxy { rt, port, tasks: Arc::new(Mutex::new(Vec::new())) };
        p.start();
        p
    }

    fn start(&self) {
        let port = self.port;
        let tasks = self.tasks.clone();
        let listener = self.rt.block_on(async move {
            let s = tokio::net::TcpSocket::new_v4().unwrap();
            s.set_reuseaddr(true).unwrap();
            s.bind(format!("127.0.0.1:{port}").parse().unwrap()).unwrap();
            s.listen(64).unwrap()
        });
        let tasks2 = tasks.clone();
        let h = self.rt.spawn(async move {
            loop {
                let Ok((mut c, _)) = listener.accept().await else { return };
                let h = tokio::spawn(async move {
                    if let Ok(mut s) = tokio::net::TcpStream::connect(("127.0.0.1", IMAP)).await {
                        let _ = tokio::io::copy_bidirectional(&mut c, &mut s).await;
                    }
                });
                tasks2.lock().unwrap().push(h.abort_handle());
            }
        });
        tasks.lock().unwrap().push(h.abort_handle());
    }

    fn stop(&self) {
        for t in self.tasks.lock().unwrap().drain(..) {
            t.abort();
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[test]
fn queues_actions_offline_and_replays_after_reconnect() {
    let root = need_server!();
    let u = unique_user("off");
    seed(&root, &u, "INBOX", &mail("One", "x", "x@localhost"), 1.0, "");
    seed(&root, &u, "INBOX", &mail("Two", "x", "x@localhost"), 1.0, "");
    let proxy = Proxy::new();
    let h = Harness::new();
    let a = h.add(setup_for(&u, (proxy.port, Security::None), (SMTP, Security::None), 30));
    let inbox = h.fid(a.id, FolderRole::Inbox);

    proxy.stop();
    let one = h.find(inbox, "One").unwrap().id;
    h.core.mark_seen(vec![one], true).unwrap();
    h.core.mark_flagged(vec![one], true).unwrap();
    assert!(h.find(inbox, "One").unwrap().seen, "works offline");
    assert!(
        wait(15, || matches!(h.core.sync_phase(a.id), SyncPhase::Offline { .. })),
        "reports offline: {:?}",
        h.core.sync_phase(a.id)
    );
    assert!(server_search(&u, "INBOX", "SEEN").is_empty(), "nothing reached the server yet");

    proxy.start();
    {
        use std::io::Read;
        let mut c = std::net::TcpStream::connect(("127.0.0.1", proxy.port)).expect("proxy accepts after restart");
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut buf = [0u8; 64];
        let n = c.read(&mut buf).expect("greeting through proxy");
        assert!(buf[..n].starts_with(b"* OK"), "greeting: {:?}", String::from_utf8_lossy(&buf[..n]));
    }
    h.core.network_changed();
    assert!(wait(20, || server_search(&u, "INBOX", "SEEN FLAGGED").len() == 1), "queued ops replayed");
    assert!(wait(10, || h.idle(a.id)), "back online");
    assert!(h.find(inbox, "One").unwrap().seen, "local state kept, not overwritten by stale server flags");
}

#[test]
fn chinese_folder_names_and_per_folder_sync_toggle() {
    let _root = need_server!();
    let u = unique_user("cn");
    on_server(&u, |s| {
        Box::pin(async move {
            s.create("&mHl27g-").await.unwrap();
            s.create("&mHl27g-/2026").await.unwrap();
            let raw = "From: z@localhost\r\nSubject: =?UTF-8?B?5ZGo5Lya?=\r\n\r\nhi\r\n";
            s.append("&mHl27g-/2026", None, None, raw.as_bytes()).await.unwrap();
        })
    });
    let h = Harness::new();
    let a = h.add(plain(&u, 30));
    let f = h.core.folders(a.id).into_iter().find(|f| f.path == "项目/2026").expect("decoded folder path");
    assert_eq!(f.display_name, "2026");
    assert_eq!(f.depth, 1);
    assert!(wait(10, || h.subjects(f.id) == vec!["周会"]));

    h.core.set_folder_sync(f.id, false).unwrap();
    assert!(h.subjects(f.id).is_empty(), "local copies dropped");
    h.core.sync_now(Some(a.id));
    std::thread::sleep(Duration::from_secs(2));
    assert!(h.subjects(f.id).is_empty(), "stays off");
    h.core.set_folder_sync(f.id, true).unwrap();
    assert!(wait(10, || h.subjects(f.id) == vec!["周会"]), "re-downloaded");
}

#[test]
fn renders_gbk_html_and_searches_after_sync() {
    let root = need_server!();
    let u = unique_user("gbk");
    let gbk = "From: =?GBK?B?1cXI/Q==?= <zhangsan@example.cn>\r\nTo: me@localhost\r\nSubject: =?GBK?B?1ty74bzN0qo=?=\r\n\
               MIME-Version: 1.0\r\nContent-Type: text/plain; charset=GBK\r\nContent-Transfer-Encoding: base64\r\n\r\n\
               tPO80rrDo6zD98zsz8LO58j9tePKx9bcu+Ghow==\r\n";
    seed(&root, &u, "INBOX", gbk, 1.0, "");
    let html = "From: Shop <shop@example.com>\r\nSubject: Sale\r\nMIME-Version: 1.0\r\n\
                Content-Type: text/html; charset=utf-8\r\n\r\n<p style=\"color:#333\">Big sale</p><img src=\"https://track.example/p.gif\">\r\n";
    seed(&root, &u, "INBOX", html, 2.0, "");
    let h = Harness::new();
    let a = h.add(plain(&u, 30));
    let inbox = h.fid(a.id, FolderRole::Inbox);

    let m = h.find(inbox, "周会纪要").expect("GBK subject decoded");
    assert_eq!(m.from_name, "张三");
    assert!(m.snippet.starts_with("大家好"));
    let found = h.list(Mailbox::AllInboxes, Some("三点"));
    assert_eq!(found.len(), 1, "2-char CJK search");
    let found = h.list(Mailbox::AllInboxes, Some("明天下午"));
    assert_eq!(found.len(), 1, "FTS CJK search");
    assert_eq!(h.list(Mailbox::AllInboxes, Some("from:shop")).len(), 1);

    let sale = h.find(inbox, "Sale").unwrap();
    let d = h.core.message(sale.id, false).unwrap();
    assert!(d.has_remote_content);
    assert!(d.html.contains("img-src data:;"), "remote blocked by default");
    assert!(d.html.contains("class=\"designed\""));
    let d = h.core.message(sale.id, true).unwrap();
    assert!(d.html.contains("img-src data: https: http:;"), "allowed on request");
    assert_eq!(h.core.smart_counts().inbox_unread, 2);
}

#[test]
fn removing_an_account_deletes_local_data() {
    let root = need_server!();
    let u = unique_user("rm");
    seed(&root, &u, "INBOX", &mail("Bye", "x", "x@localhost"), 1.0, "");
    let h = Harness::new();
    let a = h.add(plain(&u, 30));
    assert_eq!(h.eml_files(), 1);
    assert!(matches!(h.core.add_account(plain(&u, 30)), Err(CoreError::InvalidInput { .. })), "no duplicates");
    h.core.remove_account(a.id).unwrap();
    assert!(h.core.accounts().is_empty());
    assert_eq!(h.eml_files(), 0);
    assert!(!h.data.join("mail").join(a.id.to_string()).exists());
    assert_eq!(server_search(&u, "INBOX", "ALL").len(), 1, "server untouched");
}

#[test]
fn syncs_two_thousand_messages_quickly() {
    let root = need_server!();
    let u = unique_user("bulk");
    for i in 0..2000 {
        let body = format!("Message number {i}. 这是第 {i} 封测试邮件，包含一些中文内容用于搜索。\n{}", "lorem ipsum ".repeat(40));
        seed(&root, &u, "INBOX", &mail(&format!("Bulk {i:04}"), &body, "bulk@localhost"), 1.0 + i as f64 / 200.0, "S");
    }
    let h = Harness::new();
    let t0 = Instant::now();
    let a = h.core.add_account(plain(&u, 30)).unwrap();
    assert!(wait(120, || h.folder(a.id, FolderRole::Inbox).is_some_and(|f| f.total == 2000) && h.idle(a.id)), "all synced");
    let took = t0.elapsed();
    eprintln!("2000 messages synced in {took:?}");
    assert!(took < Duration::from_secs(60), "too slow: {took:?}");

    let inbox = h.fid(a.id, FolderRole::Inbox);
    let t1 = Instant::now();
    let page = h.core.messages(MessageQuery { mailbox: Mailbox::Folder { id: inbox }, search: None, limit: 200, before_date: None, before_id: None });
    let list_time = t1.elapsed();
    assert_eq!(page.len(), 200);
    assert_eq!(page[0].subject, "Bulk 0000");
    let t2 = Instant::now();
    let hits = h.list(Mailbox::AllInboxes, Some("第 1234 封"));
    let search_time = t2.elapsed();
    assert_eq!(hits.len(), 1, "phrase search");
    eprintln!("list 200: {list_time:?}, search: {search_time:?}");
    assert!(list_time < Duration::from_millis(100));
    assert!(search_time < Duration::from_millis(300));

    // Second sync with nothing new is cheap (CONDSTORE: no flag refetch).
    let t3 = Instant::now();
    h.core.sync_now(Some(a.id));
    std::thread::sleep(Duration::from_millis(200));
    assert!(wait(30, || h.idle(a.id)));
    eprintln!("no-op resync took {:?}", t3.elapsed());
    assert!(t3.elapsed() < Duration::from_secs(5));
}

#[test]
fn searches_the_server_for_older_mail_without_keeping_it() {
    let root = need_server!();
    let h = Harness::new();
    let u = unique_user("remote");
    seed(&root, &u, "INBOX", &mail("Recent kayak note", "kept on this Mac", "amy@example.com"), 2.0, "");
    seed(&root, &u, "INBOX", &mail("Old invoice", "Invoice for the blue kayak", "shop@example.com"), 200.0, "");
    seed(&root, &u, "INBOX", &mail("旧的周会纪要", "讨论了独木舟的预算", "张三 <zhang@example.com>"), 300.0, "S");
    seed(&root, &u, "Archive", &mail("Archived kayak receipt", "paid", "shop@example.com"), 400.0, "S");
    let a = h.add(plain(&u, 30));
    let local_files = h.eml_files();
    let local_count = h.list(Mailbox::Everything, None).len();

    let subjects = |v: &[MessageSummary]| v.iter().map(|m| m.subject.clone()).collect::<Vec<_>>();
    // Only mail older than the window comes back, newest first, from every searchable folder.
    let hits = h.core.search_server("kayak".into(), Mailbox::Everything).unwrap();
    assert_eq!(subjects(&hits), ["Old invoice", "Archived kayak receipt"]);
    assert!(hits.iter().all(|m| m.id < 0 && m.account_id == a.id));
    // Scoped to one folder.
    let inbox = h.fid(a.id, FolderRole::Inbox);
    assert_eq!(subjects(&h.core.search_server("kayak".into(), Mailbox::Folder { id: inbox }).unwrap()), ["Old invoice"]);
    // Chinese terms travel as literals; from: works.
    assert_eq!(subjects(&h.core.search_server("独木舟".into(), Mailbox::Everything).unwrap()), ["旧的周会纪要"]);
    assert_eq!(subjects(&h.core.search_server("from:zhang".into(), Mailbox::Everything).unwrap()), ["旧的周会纪要"]);
    // The same message keeps its id across searches.
    assert_eq!(h.core.search_server("invoice".into(), Mailbox::Everything).unwrap()[0].id, hits[0].id);

    // Opening downloads it into memory only.
    let d = h.core.message(hits[0].id, false).unwrap();
    assert!(d.html.contains("blue kayak"), "{}", d.html);
    assert!(h.core.reply_draft(hits[0].id, false).unwrap().subject.starts_with("Re:"));
    assert_eq!(h.eml_files(), local_files, "nothing written to disk");
    assert_eq!(h.list(Mailbox::Everything, None).len(), local_count, "nothing added to the local store");
    // Reading it doesn't mark it read on the server (EXAMINE + BODY.PEEK).
    assert_eq!(server_search(&u, "INBOX", "UNSEEN SUBJECT \"Old invoice\"").len(), 1);
}

#[test]
fn send_from_aliases_found_added_and_used() {
    let root = need_server!();
    let h = Harness::new();
    let u = unique_user("alias");
    // Sent from another of the user's addresses before (e.g. in Apple Mail).
    let other = format!("other.{u}");
    seed(&root, &u, "Sent", &mail("Earlier", "sent from my other address", &format!("Alex <{other}>")), 1.0, "S");
    let mut setup = plain(&u, 30);
    setup.display_name = "Alex".into();
    let a = h.add(setup);
    let list = |h: &Harness| h.core.send_addresses(a.id).into_iter().map(|s| (s.address, s.kind)).collect::<Vec<_>>();
    assert_eq!(list(&h), vec![(u.clone(), SendAddressKind::Primary), (other.clone(), SendAddressKind::Found)]);

    h.core.add_send_address(a.id, "Alex Work".into(), "alex@work.example".into()).unwrap();
    assert!(h.core.add_send_address(a.id, String::new(), "not an address".into()).is_err());
    assert_eq!(list(&h)[1], ("alex@work.example".into(), SendAddressKind::Added), "added before found");

    // Sending from an alias: header and envelope use it.
    let mut d = h.core.new_draft(a.id).unwrap();
    d.to = format!("Bob <bob.{u}>");
    d.subject = "From my alias".into();
    d.from = Some(EmailAddress { name: "Alex Work".into(), address: "alex@work.example".into() });
    h.core.send(d).unwrap();
    let log = std::fs::read_to_string(root.join("smtp-log.jsonl")).unwrap();
    let line = log.lines().rev().find(|l| l.contains(&format!("bob.{u}"))).expect("delivered");
    assert!(line.contains("\"from\": \"alex@work.example\""), "{line}");

    h.core.remove_send_address(a.id, "ALEX@work.example".into()).unwrap();
    assert_eq!(list(&h).len(), 2);
    // Removing the account removes its aliases with it.
    h.core.add_send_address(a.id, String::new(), "x@work.example".into()).unwrap();
    h.core.remove_account(a.id).unwrap();
    assert!(h.core.send_addresses(a.id).is_empty());
}
