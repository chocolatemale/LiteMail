//! Searching mail that isn't kept on this Mac.
//!
//! Each account keeps the last N days locally; anything older stays on the server. A search
//! asks the server (`UID SEARCH … BEFORE <window start>`, Gmail: `X-GM-RAW`) and fetches only
//! the header fields needed for the list. A message's body is downloaded when it is opened and
//! kept in memory (capped), never written to disk, so searching doesn't grow the local store.
//!
//! Hits get negative ids, stable per (account, folder, uid) for the life of the process, so the
//! existing message APIs (reader, attachments, reply, forward, source) work on them unchanged.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use async_imap::imap_proto::{MailboxDatum, Response};

use crate::error::Result;
use crate::imap::{self, Seg, Session, quote, uid_set};
use crate::mime;
use crate::store::tokenize;
use crate::types::{EmailAddress, Folder, FolderRole, Mailbox, MessageSummary};

/// Newest matches fetched per folder, and kept overall.
pub const PER_FOLDER: usize = 40;
pub const MAX_RESULTS: usize = 80;
/// Opened server messages kept in memory.
const BODY_CACHE_BYTES: usize = 40 * 1024 * 1024;

const HEADER_ITEMS: &str = "(UID FLAGS INTERNALDATE RFC822.SIZE BODY.PEEK[HEADER.FIELDS (FROM TO CC SUBJECT DATE CONTENT-TYPE)])";

#[derive(Debug, Clone)]
pub struct Hit {
    pub summary: MessageSummary,
    /// Server (modified UTF-7) mailbox name.
    pub folder_name: String,
    pub uid: u32,
}

#[derive(Default)]
pub struct Cache {
    ids: HashMap<(i64, i64, u32), i64>,
    hits: HashMap<i64, Hit>,
    bodies: VecDeque<(i64, Arc<Vec<u8>>)>,
    body_bytes: usize,
}

impl Cache {
    /// Stores the hits and returns their summaries with ids filled in.
    pub fn insert(&mut self, hits: Vec<Hit>) -> Vec<MessageSummary> {
        if self.hits.len() > 20_000 {
            self.hits.clear();
        }
        hits.into_iter()
            .map(|mut h| {
                let key = (h.summary.account_id, h.summary.folder_id, h.uid);
                let next = -(self.ids.len() as i64) - 1;
                let id = *self.ids.entry(key).or_insert(next);
                h.summary.id = id;
                let s = h.summary.clone();
                self.hits.insert(id, h);
                s
            })
            .collect()
    }

    pub fn hit(&self, id: i64) -> Option<Hit> {
        self.hits.get(&id).cloned()
    }

    pub fn body(&self, id: i64) -> Option<Arc<Vec<u8>>> {
        self.bodies.iter().find(|(i, _)| *i == id).map(|(_, b)| b.clone())
    }

    pub fn put_body(&mut self, id: i64, raw: Arc<Vec<u8>>) {
        self.body_bytes += raw.len();
        self.bodies.push_back((id, raw));
        while self.body_bytes > BODY_CACHE_BYTES && self.bodies.len() > 1 {
            if let Some((_, old)) = self.bodies.pop_front() {
                self.body_bytes -= old.len();
            }
        }
    }
}

/// The SEARCH command for a query. `attachments_only` asks the caller to keep only messages
/// whose top-level type is multipart/mixed (IMAP has no attachment search; Gmail does).
#[derive(Debug, PartialEq, Eq)]
pub struct Criteria {
    pub segs: Vec<Seg>,
    pub attachments_only: bool,
}

fn astring(v: &str) -> Seg {
    if v.is_ascii() && !v.chars().any(|c| c.is_control()) { Seg::Text(quote(v)) } else { Seg::Lit(v.to_string()) }
}

/// Builds `UID SEARCH …` for LiteMail's search syntax (words, quoted phrases, from: to:
/// subject: has:attachment is:unread is:read is:flagged — English or Chinese prefixes).
/// `before` limits the search to mail older than the local window.
pub fn criteria(query: &str, gmail: bool, before: Option<i64>) -> Criteria {
    let tokens = tokenize(query);
    let mut segs = vec![Seg::Text("UID SEARCH".into())];
    if !query.is_ascii() {
        segs.push(Seg::Text(" CHARSET UTF-8".into()));
    }
    if gmail {
        // Gmail understands nearly the same syntax natively.
        let mut raw: Vec<String> = tokens
            .iter()
            .map(|t| {
                let lower = t.to_lowercase();
                let (key, val) = match lower.split_once(':') {
                    Some((k, _)) => (k.to_string(), t[t.find(':').unwrap() + 1..].to_string()),
                    None => (String::new(), t.clone()),
                };
                let key = match key.as_str() {
                    "发件人" => "from",
                    "收件人" => "to",
                    "主题" => "subject",
                    k => k,
                }
                .to_string();
                let val = match (key.as_str(), val.to_lowercase().as_str()) {
                    ("is", "flagged" | "旗标") => "starred".to_string(),
                    ("is", "未读") => "unread".to_string(),
                    ("is", "已读") => "read".to_string(),
                    ("has", "附件") => "attachment".to_string(),
                    _ => val,
                };
                let val = if val.contains(char::is_whitespace) { format!("\"{val}\"") } else { val };
                if key.is_empty() { val } else { format!("{key}:{val}") }
            })
            .collect();
        if let Some(ts) = before {
            if let Some(d) = chrono::DateTime::from_timestamp(ts, 0) {
                raw.push(format!("before:{}", d.format("%Y/%m/%d")));
            }
        }
        segs.push(Seg::Text(" X-GM-RAW ".into()));
        segs.push(astring(&raw.join(" ")));
        return Criteria { segs, attachments_only: false };
    }

    let mut attachments_only = false;
    let mut any = false;
    for t in &tokens {
        let lower = t.to_lowercase();
        let value = |prefixes: &[&str]| prefixes.iter().find_map(|p| lower.strip_prefix(p).map(|_| t[t.find(':').unwrap() + 1..].to_string()));
        if let Some(v) = value(&["from:", "发件人:"]) {
            segs.push(Seg::Text(" FROM ".into()));
            segs.push(astring(&v));
        } else if let Some(v) = value(&["to:", "收件人:"]) {
            segs.push(Seg::Text(" OR TO ".into()));
            segs.push(astring(&v));
            segs.push(Seg::Text(" CC ".into()));
            segs.push(astring(&v));
        } else if let Some(v) = value(&["subject:", "主题:"]) {
            segs.push(Seg::Text(" SUBJECT ".into()));
            segs.push(astring(&v));
        } else if lower == "has:attachment" || lower == "has:附件" {
            attachments_only = true;
            continue;
        } else if lower == "is:unread" || lower == "is:未读" {
            segs.push(Seg::Text(" UNSEEN".into()));
        } else if lower == "is:read" || lower == "is:已读" {
            segs.push(Seg::Text(" SEEN".into()));
        } else if lower == "is:flagged" || lower == "is:starred" || lower == "is:旗标" {
            segs.push(Seg::Text(" FLAGGED".into()));
        } else {
            segs.push(Seg::Text(" TEXT ".into()));
            segs.push(astring(t));
        }
        any = true;
    }
    if let Some(ts) = before {
        segs.push(Seg::Text(format!(" BEFORE {}", imap::imap_date(ts))));
        any = true;
    }
    if !any {
        segs.push(Seg::Text(" ALL".into()));
    }
    Criteria { segs, attachments_only }
}

/// Folders worth searching for a mailbox scope.
pub fn folders_for(folders: &[Folder], mailbox: &Mailbox) -> Vec<Folder> {
    match mailbox {
        Mailbox::Folder { id } => folders.iter().filter(|f| f.id == *id).cloned().collect(),
        Mailbox::AllInboxes | Mailbox::Unread | Mailbox::Flagged => folders.iter().filter(|f| f.role == FolderRole::Inbox).cloned().collect(),
        _ => {
            // Gmail's All Mail already contains everything.
            if let Some(all) = folders.iter().find(|f| f.role == FolderRole::All) {
                return vec![all.clone()];
            }
            folders
                .iter()
                .filter(|f| !matches!(f.role, FolderRole::Trash | FolderRole::Junk | FolderRole::Drafts | FolderRole::Flagged | FolderRole::Important))
                .cloned()
                .collect()
        }
    }
}

/// Searches one folder; `local` are UIDs already kept on this Mac (skipped).
pub async fn search_folder(session: &mut Session, folder: &Folder, crit: &Criteria, local: &HashSet<u32>) -> Result<Vec<Hit>> {
    imap::simple(session, &format!("EXAMINE {}", quote(&folder.name))).await?;
    let mut found: Vec<u32> = Vec::new();
    imap::exec_segments(session, &crit.segs, |r| {
        if let Response::MailboxData(MailboxDatum::Search(ids)) = r {
            found.extend(ids.iter().copied());
        }
    })
    .await?;
    found.retain(|u| !local.contains(u));
    found.sort_unstable_by(|a, b| b.cmp(a));
    found.truncate(PER_FOLDER);
    if found.is_empty() {
        return Ok(Vec::new());
    }
    let label = folder.path.clone();
    let mut hits = Vec::new();
    for f in imap::uid_fetch(session, &uid_set(&found), HEADER_ITEMS).await? {
        let (Some(uid), Some(header)) = (f.uid, f.header.as_deref()) else { continue };
        let mut head = header.to_vec();
        head.extend_from_slice(b"\r\n");
        let p = mime::parse(&head);
        let lower = String::from_utf8_lossy(header).to_ascii_lowercase();
        let has_attachments = lower.contains("multipart/mixed");
        if crit.attachments_only && !has_attachments {
            continue;
        }
        let flags = f.flags.unwrap_or_default();
        let to_line = p
            .to
            .iter()
            .map(|a: &EmailAddress| if a.name.is_empty() { a.address.clone() } else { format!("{} <{}>", a.name, a.address) })
            .collect::<Vec<_>>()
            .join(", ");
        hits.push(Hit {
            summary: MessageSummary {
                id: 0,
                account_id: folder.account_id,
                folder_id: folder.id,
                subject: p.subject.clone(),
                from_name: p.from.name.clone(),
                from_addr: p.from.address.clone(),
                to_line,
                date: f.internal_date.or(p.date).unwrap_or_default(),
                snippet: t!("On the server · {label}", "在服务器上 · {label}"),
                seen: flags.seen,
                flagged: flags.flagged,
                answered: flags.answered,
                has_attachments,
                size: f.size.unwrap_or_default() as u64,
            },
            folder_name: folder.name.clone(),
            uid,
        });
    }
    Ok(hits)
}

/// Downloads one message (read-only: EXAMINE + BODY.PEEK, the \Seen flag is left alone).
pub async fn fetch_body(session: &mut Session, folder_name: &str, uid: u32) -> Result<Vec<u8>> {
    imap::simple(session, &format!("EXAMINE {}", quote(folder_name))).await?;
    let items = imap::uid_fetch(session, &uid.to_string(), "(UID BODY.PEEK[])").await?;
    items
        .into_iter()
        .find(|f| f.uid == Some(uid))
        .and_then(|f| f.body)
        .ok_or_else(|| crate::error::CoreError::server(t!("The server no longer has this message", "服务器上已经没有这封邮件了")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(c: &Criteria) -> String {
        c.segs
            .iter()
            .map(|s| match s {
                Seg::Text(t) => t.clone(),
                Seg::Lit(l) => format!("{{{}}}<{l}>", l.len()),
            })
            .collect()
    }

    #[test]
    fn builds_imap_search() {
        let before = Some(1_789_000_000); // 2026-09-10
        assert_eq!(text(&criteria("invoice", false, before)), "UID SEARCH TEXT \"invoice\" BEFORE 10-Sep-2026");
        assert_eq!(
            text(&criteria("from:bob subject:\"Q4 plan\" is:unread", false, None)),
            "UID SEARCH FROM \"bob\" SUBJECT \"Q4 plan\" UNSEEN"
        );
        assert_eq!(text(&criteria("to:amy", false, None)), "UID SEARCH OR TO \"amy\" CC \"amy\"");
        // Non-ASCII goes as a literal, with CHARSET.
        assert_eq!(text(&criteria("发件人:张三 周会", false, None)), "UID SEARCH CHARSET UTF-8 FROM {6}<张三> TEXT {6}<周会>");
        let c = criteria("has:attachment", false, None);
        assert!(c.attachments_only);
        assert_eq!(text(&c), "UID SEARCH ALL");
    }

    #[test]
    fn builds_gmail_search() {
        let c = criteria("from:bob is:flagged \"red car\"", true, Some(1_789_000_000));
        assert_eq!(text(&c), "UID SEARCH X-GM-RAW \"from:bob is:starred \\\"red car\\\" before:2026/09/10\"");
        assert_eq!(text(&criteria("主题:周会", true, None)), "UID SEARCH CHARSET UTF-8 X-GM-RAW {14}<subject:周会>");
    }

    #[test]
    fn cache_ids_are_negative_and_stable() {
        let hit = |acct: i64, folder: i64, uid: u32| Hit {
            summary: MessageSummary {
                id: 0,
                account_id: acct,
                folder_id: folder,
                subject: String::new(),
                from_name: String::new(),
                from_addr: String::new(),
                to_line: String::new(),
                date: 0,
                snippet: String::new(),
                seen: true,
                flagged: false,
                answered: false,
                has_attachments: false,
                size: 0,
            },
            folder_name: "INBOX".into(),
            uid,
        };
        let mut c = Cache::default();
        let a = c.insert(vec![hit(1, 1, 10), hit(1, 1, 11)]);
        assert!(a.iter().all(|s| s.id < 0));
        let b = c.insert(vec![hit(1, 1, 11)]);
        assert_eq!(b[0].id, a[1].id, "same message, same id across searches");
        c.put_body(a[0].id, Arc::new(vec![0; 10]));
        assert_eq!(c.body(a[0].id).unwrap().len(), 10);
        assert!(c.body(a[1].id).is_none());
    }
}
