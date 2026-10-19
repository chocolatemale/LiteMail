//! IMAP connection setup (implicit TLS, STARTTLS, plain for localhost) and small helpers.

use std::fmt::Debug;
use std::time::Duration;

use async_imap::imap_proto::{AttributeValue, MailboxDatum, Response, ResponseCode, Status};
use async_imap::types::NameAttribute;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_native_tls::native_tls;

use crate::error::{CoreError, Result};
use crate::store::{Flags, ServerFolder};
use crate::types::{FolderRole, Security, ServerConfig};

pub trait Io: AsyncRead + AsyncWrite + Unpin + Send + Debug {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send + Debug> Io for T {}

pub type Stream = Box<dyn Io>;
pub type Session = async_imap::Session<Stream>;

pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(90);

#[derive(Debug, Clone, Default)]
pub struct Caps {
    pub idle: bool,
    pub moves: bool,
    pub uidplus: bool,
    pub condstore: bool,
    /// Gmail's extensions (X-GM-RAW search).
    pub gmail: bool,
}

pub fn is_local_host(host: &str) -> bool {
    matches!(host.trim().to_ascii_lowercase().as_str(), "localhost" | "127.0.0.1" | "::1")
}

pub fn tls_connector(host: &str, allow_invalid: bool) -> Result<tokio_native_tls::TlsConnector> {
    let mut b = native_tls::TlsConnector::builder();
    if allow_invalid && is_local_host(host) {
        b.danger_accept_invalid_certs(true).danger_accept_invalid_hostnames(true);
    }
    Ok(tokio_native_tls::TlsConnector::from(b.build()?))
}

async fn tcp(cfg: &ServerConfig) -> Result<TcpStream> {
    let addr = (cfg.host.trim(), cfg.port);
    let s = timeout(CONNECT_TIMEOUT, TcpStream::connect(addr))
        .await?
        .map_err(|e| CoreError::network(t!("Couldn't connect to {}:{} ({e})", "无法连接 {}:{}（{e}）", cfg.host, cfg.port)))?;
    s.set_nodelay(true).ok();
    Ok(s)
}

async fn greeting(client: &mut async_imap::Client<Stream>) -> Result<()> {
    match timeout(CONNECT_TIMEOUT, client.read_response()).await? {
        Ok(Some(_)) => Ok(()),
        Ok(None) => Err(CoreError::network(t!("The server didn't send an IMAP greeting", "服务器没有回应 IMAP 问候"))),
        Err(e) => Err(CoreError::network(e.to_string())),
    }
}

pub async fn connect(cfg: &ServerConfig, allow_invalid: bool) -> Result<async_imap::Client<Stream>> {
    let host = cfg.host.trim();
    match cfg.security {
        Security::Tls => {
            let tcp = tcp(cfg).await?;
            let tls = tls_connector(host, allow_invalid)?;
            let s = timeout(CONNECT_TIMEOUT, tls.connect(host, tcp)).await??;
            let mut client = async_imap::Client::new(Box::new(s) as Stream);
            greeting(&mut client).await?;
            Ok(client)
        }
        Security::StartTls => {
            let tcp = tcp(cfg).await?;
            let mut client = async_imap::Client::new(Box::new(tcp) as Stream);
            greeting(&mut client).await?;
            timeout(COMMAND_TIMEOUT, client.run_command_and_check_ok("STARTTLS", None))
                .await?
                .map_err(|e| CoreError::network(t!("The server doesn't support STARTTLS: {e}", "服务器不支持 STARTTLS：{e}")))?;
            let inner = client.into_inner();
            let tls = tls_connector(host, allow_invalid)?;
            let s = timeout(CONNECT_TIMEOUT, tls.connect(host, inner)).await??;
            Ok(async_imap::Client::new(Box::new(s) as Stream))
        }
        Security::None => {
            if !is_local_host(host) {
                return Err(CoreError::invalid(t!("Unencrypted connections are only allowed to this Mac", "不加密的连接只允许用于本机测试服务器")));
            }
            let tcp = tcp(cfg).await?;
            let mut client = async_imap::Client::new(Box::new(tcp) as Stream);
            greeting(&mut client).await?;
            Ok(client)
        }
    }
}

pub async fn login(cfg: &ServerConfig, allow_invalid: bool, user: &str, password: &str) -> Result<(Session, Caps)> {
    let client = connect(cfg, allow_invalid).await?;
    let mut session = match timeout(COMMAND_TIMEOUT, client.login(user.trim(), password)).await? {
        Ok(s) => s,
        Err((async_imap::error::Error::No(msg), _)) | Err((async_imap::error::Error::Bad(msg), _)) => {
            let text = crate::error::server_text(&msg);
            return Err(if is_temporary_login_failure(&text) {
                CoreError::network(t!("The server is temporarily unavailable: {text}", "服务器暂时不可用：{text}"))
            } else {
                CoreError::Auth { message: auth_message(&text) }
            });
        }
        Err((e, _)) => return Err(e.into()),
    };
    let caps = timeout(COMMAND_TIMEOUT, session.capabilities()).await??;
    if caps.has_str("ID") {
        // NetEase (163/126) rejects SELECT with "Unsafe Login" until the client identifies itself.
        let id = format!("ID (\"name\" \"LiteMail\" \"version\" \"{}\" \"vendor\" \"LiteMail\")", env!("CARGO_PKG_VERSION"));
        let _ = exec(&mut session, &id, |_| {}).await;
    }
    let caps = Caps {
        idle: caps.has_str("IDLE"),
        moves: caps.has_str("MOVE"),
        uidplus: caps.has_str("UIDPLUS"),
        condstore: caps.has_str("CONDSTORE") || caps.has_str("QRESYNC"),
        gmail: caps.has_str("X-GM-EXT-1"),
    };
    Ok((session, caps))
}

/// RFC 5530 codes and provider wording for "try again later" (not a credentials problem).
pub fn is_temporary_login_failure(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    ["[unavailable]", "[serverbug]", "[inuse]", "[limit]", "temporar", "try again", "too many simultaneous", "system error"]
        .iter()
        .any(|p| t.contains(p))
}

fn auth_message(server: &str) -> String {
    let s = server.trim();
    if s.is_empty() {
        t!("Wrong user name or password", "用户名或密码不正确")
    } else {
        t!("Sign-in was refused: {s}", "登录被拒绝：{s}")
    }
}

pub fn role_from(name: &str, attrs: &[NameAttribute<'_>]) -> FolderRole {
    for a in attrs {
        match a {
            NameAttribute::Sent => return FolderRole::Sent,
            NameAttribute::Drafts => return FolderRole::Drafts,
            NameAttribute::Trash => return FolderRole::Trash,
            NameAttribute::Junk => return FolderRole::Junk,
            NameAttribute::Archive => return FolderRole::Archive,
            NameAttribute::All => return FolderRole::All,
            NameAttribute::Flagged => return FolderRole::Flagged,
            NameAttribute::Extension(e) if e.eq_ignore_ascii_case("\\Important") => return FolderRole::Important,
            _ => {}
        }
    }
    if name.eq_ignore_ascii_case("INBOX") {
        return FolderRole::Inbox;
    }
    // Servers without SPECIAL-USE: guess from common names.
    let decoded = crate::utf7::decode(name);
    let leaf = decoded.rsplit(['/', '.']).next().unwrap_or(&decoded).to_lowercase();
    match leaf.as_str() {
        "sent" | "sent messages" | "sent items" | "sent mail" | "已发送" | "已发送邮件" => FolderRole::Sent,
        "drafts" | "draft" | "草稿" | "草稿箱" => FolderRole::Drafts,
        "trash" | "deleted messages" | "deleted items" | "deleted" | "bin" | "已删除" | "废纸篓" | "垃圾桶" => FolderRole::Trash,
        "junk" | "spam" | "junk e-mail" | "junk email" | "垃圾邮件" => FolderRole::Junk,
        "archive" | "archives" | "归档" => FolderRole::Archive,
        _ => FolderRole::Custom,
    }
}

/// Apple Notes keeps notes in a top-level "Notes" mailbox (iCloud, and any IMAP account that
/// syncs Notes). They aren't mail, so LiteMail never lists or syncs that folder.
pub fn is_notes_store(name: &str) -> bool {
    name.eq_ignore_ascii_case("Notes")
}

pub async fn list_folders(session: &mut Session) -> Result<Vec<ServerFolder>> {
    let mut out = Vec::new();
    let mut seen_roles = std::collections::HashSet::new();
    exec(session, "LIST \"\" \"*\"", |r| {
        let Response::MailboxData(MailboxDatum::List(d)) = r else { return };
        if is_notes_store(&d.name) {
            return;
        }
        let attrs = &d.name_attributes;
        let selectable = !attrs.iter().any(|a| matches!(a, NameAttribute::NoSelect))
            && !attrs.iter().any(|a| matches!(a, NameAttribute::Extension(e) if e.eq_ignore_ascii_case("\\NonExistent")));
        let mut role = role_from(&d.name, attrs);
        // Only one folder per special role; later duplicates are treated as normal folders.
        if role != FolderRole::Custom && selectable && !seen_roles.insert(role) {
            role = FolderRole::Custom;
        }
        out.push(ServerFolder {
            name: d.name.to_string(),
            delimiter: d.delimiter.as_ref().map(|x| x.to_string()),
            role,
            selectable,
        });
    })
    .await?;
    if !out.iter().any(|f| f.role == FolderRole::Inbox) {
        out.push(ServerFolder { name: "INBOX".into(), delimiter: None, role: FolderRole::Inbox, selectable: true });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------
// Strict command execution.
//
// async-imap's response streams end quietly on EOF and don't check the tagged status, so
// a dropped connection during UID SEARCH looks like "the mailbox is empty" and a failed
// STORE looks like success. Everything the sync engine relies on goes through `exec`,
// which only succeeds on a tagged OK and treats EOF / BYE as a connection error.

pub fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

pub async fn exec<F>(session: &mut Session, cmd: &str, on_response: F) -> Result<()>
where
    F: FnMut(&Response<'_>),
{
    let tag = timeout(COMMAND_TIMEOUT, session.run_command(cmd)).await??;
    finish(session, tag, on_response).await
}

/// A piece of a command: plain text, or a string sent as an IMAP literal (`{n}` + bytes), which
/// is how non-ASCII search terms have to travel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Seg {
    Text(String),
    Lit(String),
}

/// Like `exec`, but sends literals the synchronizing way: each `{n}` waits for the server's
/// "+" before the bytes go out. Works on every server (no LITERAL+ needed).
pub async fn exec_segments<F>(session: &mut Session, segs: &[Seg], on_response: F) -> Result<()>
where
    F: FnMut(&Response<'_>),
{
    let mut line = String::new();
    let mut tag = None;
    for seg in segs {
        match seg {
            Seg::Text(t) => line.push_str(t),
            Seg::Lit(l) => {
                line.push_str(&format!("{{{}}}", l.len()));
                match &tag {
                    None => tag = Some(timeout(COMMAND_TIMEOUT, session.run_command(&line)).await??),
                    Some(_) => timeout(COMMAND_TIMEOUT, session.run_command_untagged(&line)).await??,
                }
                wait_continue(session).await?;
                line = l.clone();
            }
        }
    }
    let tag = match tag {
        None => timeout(COMMAND_TIMEOUT, session.run_command(&line)).await??,
        Some(t) => {
            timeout(COMMAND_TIMEOUT, session.run_command_untagged(&line)).await??;
            t
        }
    };
    finish(session, tag, on_response).await
}

async fn wait_continue(session: &mut Session) -> Result<()> {
    loop {
        let next = timeout(COMMAND_TIMEOUT, session.read_response())
            .await?
            .map_err(|e| CoreError::network(e.to_string()))?;
        let Some(resp) = next else {
            return Err(CoreError::network(t!("The connection to the server was lost", "与服务器的连接已断开")));
        };
        match resp.parsed() {
            Response::Continue { .. } => return Ok(()),
            Response::Done { status, outcome, .. } if *status != Status::Ok => {
                return Err(CoreError::server(outcome.information.as_deref().map(str::to_string).unwrap_or_else(|| t!("The server rejected the request", "服务器拒绝了请求"))));
            }
            _ => {}
        }
    }
}

async fn finish<F>(session: &mut Session, tag: async_imap::imap_proto::RequestId, mut on_response: F) -> Result<()>
where
    F: FnMut(&Response<'_>),
{
    loop {
        let next = timeout(COMMAND_TIMEOUT, session.read_response())
            .await?
            .map_err(|e| CoreError::network(e.to_string()))?;
        let Some(resp) = next else {
            return Err(CoreError::network(t!("The connection to the server was lost", "与服务器的连接已断开")));
        };
        match resp.parsed() {
            Response::Done { tag: t, status, outcome } if *t == tag => {
                return match status {
                    Status::Ok => Ok(()),
                    _ => Err(CoreError::server(outcome.information.as_deref().map(str::to_string).unwrap_or_else(|| t!("The server rejected the request", "服务器拒绝了请求")))),
                };
            }
            Response::Data { status: Status::Bye, outcome } => {
                return Err(CoreError::network(t!("The server closed the connection: {}", "服务器断开了连接：{}", outcome.information.as_deref().unwrap_or_default())));
            }
            other => on_response(other),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SelectInfo {
    pub exists: u32,
    pub uidvalidity: Option<u32>,
    pub highest_modseq: Option<u64>,
}

pub async fn select(session: &mut Session, name: &str, condstore: bool) -> Result<SelectInfo> {
    let cmd = if condstore { format!("SELECT {} (CONDSTORE)", quote(name)) } else { format!("SELECT {}", quote(name)) };
    let mut info = SelectInfo::default();
    exec(session, &cmd, |r| match r {
        Response::MailboxData(MailboxDatum::Exists(n)) => info.exists = *n,
        Response::Data { outcome, .. } => match outcome.code {
            Some(ResponseCode::UidValidity(v)) => info.uidvalidity = Some(v),
            Some(ResponseCode::HighestModSeq(m)) => info.highest_modseq = Some(m),
            _ => {}
        },
        _ => {}
    })
    .await?;
    Ok(info)
}

pub async fn uid_search(session: &mut Session, query: &str) -> Result<std::collections::HashSet<u32>> {
    let mut out = std::collections::HashSet::new();
    exec(session, &format!("UID SEARCH {query}"), |r| {
        if let Response::MailboxData(MailboxDatum::Search(ids)) = r {
            out.extend(ids.iter().copied());
        }
    })
    .await?;
    Ok(out)
}

#[derive(Debug, Clone, Default)]
pub struct FetchItem {
    pub uid: Option<u32>,
    pub flags: Option<Flags>,
    pub internal_date: Option<i64>,
    pub size: Option<u32>,
    pub body: Option<Vec<u8>>,
    /// `BODY.PEEK[HEADER.FIELDS (...)]`
    pub header: Option<Vec<u8>>,
}

pub fn parse_internal_date(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_str(s.trim(), "%d-%b-%Y %H:%M:%S %z").ok().map(|d| d.timestamp())
}

fn fetch_item(attrs: &[AttributeValue<'_>]) -> FetchItem {
    let mut f = FetchItem::default();
    for a in attrs {
        match a {
            AttributeValue::Uid(u) => f.uid = Some(*u),
            AttributeValue::Flags(list) => f.flags = Some(flags_from(list.iter().map(|c| c.as_ref()))),
            AttributeValue::InternalDate(d) => f.internal_date = parse_internal_date(d),
            AttributeValue::Rfc822Size(n) => f.size = Some(*n),
            AttributeValue::BodySection { section: None, data: Some(d), .. } => f.body = Some(d.to_vec()),
            AttributeValue::BodySection { section: Some(_), data: Some(d), .. } => f.header = Some(d.to_vec()),
            AttributeValue::Rfc822(Some(d)) => f.body = Some(d.to_vec()),
            _ => {}
        }
    }
    f
}

pub async fn uid_fetch(session: &mut Session, set: &str, items: &str) -> Result<Vec<FetchItem>> {
    let mut out = Vec::new();
    exec(session, &format!("UID FETCH {set} {items}"), |r| {
        if let Response::Fetch(_, attrs) = r {
            out.push(fetch_item(attrs));
        }
    })
    .await?;
    Ok(out)
}

pub async fn simple(session: &mut Session, cmd: &str) -> Result<()> {
    exec(session, cmd, |_| {}).await
}

pub fn flags_from<'a>(it: impl Iterator<Item = &'a str>) -> Flags {
    let mut f = Flags::default();
    for flag in it {
        match flag.to_ascii_lowercase().as_str() {
            "\\seen" => f.seen = true,
            "\\flagged" => f.flagged = true,
            "\\answered" => f.answered = true,
            _ => {}
        }
    }
    f
}

/// Formats a date for SEARCH SINCE / BEFORE ("18-Oct-2026").
pub fn imap_date(ts: i64) -> String {
    let d = chrono::DateTime::from_timestamp(ts, 0).unwrap_or_default();
    d.format("%-d-%b-%Y").to_string()
}

/// Compresses sorted uids into an IMAP sequence set ("1:5,8,10:12").
pub fn uid_set(uids: &[u32]) -> String {
    let mut v = uids.to_vec();
    v.sort_unstable();
    v.dedup();
    let mut out = String::new();
    let mut i = 0;
    while i < v.len() {
        let start = v[i];
        let mut end = start;
        while i + 1 < v.len() && v[i + 1] == end + 1 {
            i += 1;
            end = v[i];
        }
        if !out.is_empty() {
            out.push(',');
        }
        if start == end {
            out.push_str(&start.to_string());
        } else {
            out.push_str(&format!("{start}:{end}"));
        }
        i += 1;
    }
    out
}

/// Splits uids into chunks whose sequence-set strings stay short (servers limit line length).
pub fn uid_chunks(uids: &[u32], max: usize) -> Vec<Vec<u32>> {
    let mut v = uids.to_vec();
    v.sort_unstable();
    v.chunks(max).map(|c| c.to_vec()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uid_sets() {
        assert_eq!(uid_set(&[5, 1, 2, 3, 8, 10, 11, 12, 3]), "1:3,5,8,10:12");
        assert_eq!(uid_set(&[42]), "42");
        assert_eq!(uid_set(&[]), "");
    }

    #[test]
    fn quoting_and_internal_dates() {
        assert_eq!(quote("INBOX"), "\"INBOX\"");
        assert_eq!(quote("a\"b\\c"), "\"a\\\"b\\\\c\"");
        assert_eq!(parse_internal_date(" 7-Oct-2026 09:41:00 +0800"), Some(1791337260));
        assert_eq!(parse_internal_date("18-Oct-2026 01:41:00 +0000"), Some(1792287660));
        assert_eq!(flags_from(["\\Seen", "\\FLAGGED", "$Junk"].into_iter()), Flags { seen: true, flagged: true, answered: false });
    }

    #[test]
    fn dates() {
        assert_eq!(imap_date(1792287660), "18-Oct-2026");
        assert_eq!(imap_date(0), "1-Jan-1970");
    }

    #[test]
    fn apple_notes_folder_is_not_mail() {
        assert!(is_notes_store("Notes"));
        assert!(is_notes_store("notes"));
        assert!(!is_notes_store("INBOX/Notes"));
        assert!(!is_notes_store("Notes 2026"));
    }

    #[test]
    fn roles_by_attribute_and_name() {
        assert_eq!(role_from("Sent Messages", &[]), FolderRole::Sent);
        assert_eq!(role_from("[Gmail]/Sent Mail", &[NameAttribute::Sent]), FolderRole::Sent);
        assert_eq!(role_from("[Gmail]/All Mail", &[NameAttribute::All]), FolderRole::All);
        assert_eq!(role_from("inbox", &[]), FolderRole::Inbox);
        assert_eq!(role_from("Deleted Messages", &[]), FolderRole::Trash);
        assert_eq!(role_from("&V4NXPpCuTvY-", &[]), FolderRole::Junk);
        assert_eq!(role_from("INBOX.Archive", &[]), FolderRole::Archive);
        assert_eq!(role_from("Projects", &[]), FolderRole::Custom);
    }

    #[test]
    fn temporary_login_failures() {
        assert!(is_temporary_login_failure("[UNAVAILABLE] Internal error occurred."));
        assert!(is_temporary_login_failure("[ALERT] Too many simultaneous connections. (Failure)"));
        assert!(!is_temporary_login_failure("[AUTHENTICATIONFAILED] Authentication failed."));
        assert!(!is_temporary_login_failure("[ALERT] Application-specific password required"));
    }

    #[test]
    fn plaintext_rejected_for_remote_hosts() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let err = rt
            .block_on(connect(&ServerConfig { host: "imap.example.com".into(), port: 143, security: Security::None }, false))
            .unwrap_err();
        assert!(matches!(err, CoreError::InvalidInput { .. }));
    }
}
