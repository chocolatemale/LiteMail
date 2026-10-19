//! Building drafts (new, reply, reply all, forward) and the outgoing MIME message.

use std::sync::atomic::{AtomicU64, Ordering};

use chrono::TimeZone;
use lettre::message::header::ContentType;
use lettre::message::{Attachment, Mailbox, Mailboxes, MultiPart, SinglePart};
use lettre::{Address, Message, address::Envelope};

use crate::error::{CoreError, Result};
use crate::mime::{self, Parsed};
use crate::types::{Account, Draft, EmailAddress};

pub fn signature_block(account: &Account) -> String {
    let sig = account.signature.trim_end();
    if sig.is_empty() { String::new() } else { format!("\n\n-- \n{sig}") }
}

pub fn new_draft(account: &Account) -> Draft {
    Draft {
        account_id: account.id,
        to: String::new(),
        cc: String::new(),
        bcc: String::new(),
        subject: String::new(),
        body: signature_block(account),
        attachments: Vec::new(),
        in_reply_to: None,
        references: Vec::new(),
        reply_to_message: None,
        forward_from: None,
        draft_source: None,
        from: None,
    }
}

/// Continues a draft stored on the server: recipients, subject, text and attachments.
pub fn edit_draft(account: &Account, mine: &[EmailAddress], raw: &[u8], local_id: i64) -> Draft {
    let p = mime::parse(raw);
    let sender = own_address(account, mine, &[&p.from]);
    let text = mime::parse_message(raw).map(|m| mime::plain_text(&m)).unwrap_or_default();
    let bcc = mime::parse_message(raw).map(|m| mime::addresses(m.bcc())).unwrap_or_default();
    let has_attachments = !mime::attachments(raw).is_empty();
    Draft {
        account_id: account.id,
        to: join(&p.to),
        cc: join(&p.cc),
        bcc: join(&bcc),
        subject: p.subject.clone(),
        body: text.trim_end().to_string(),
        attachments: Vec::new(),
        in_reply_to: p.in_reply_to.clone(),
        references: p.references.clone(),
        reply_to_message: None,
        forward_from: has_attachments.then_some(local_id),
        draft_source: Some(local_id),
        from: sender,
    }
}

fn format_addr(a: &EmailAddress) -> String {
    if a.name.is_empty() || a.name == a.address {
        a.address.clone()
    } else if a.name.contains([',', '"', '<', '>', ';', '@']) {
        format!("\"{}\" <{}>", a.name.replace('"', "'"), a.address)
    } else {
        format!("{} <{}>", a.name, a.address)
    }
}

fn join(addrs: &[EmailAddress]) -> String {
    addrs.iter().map(format_addr).collect::<Vec<_>>().join(", ")
}

fn has_prefix(subject: &str, prefixes: &[&str]) -> bool {
    let s = subject.trim_start().to_lowercase();
    prefixes.iter().any(|p| s.starts_with(p))
}

pub fn reply_subject(subject: &str) -> String {
    if has_prefix(subject, &["re:", "re：", "回复:", "回复：", "答复:", "答复："]) {
        subject.trim().to_string()
    } else {
        format!("Re: {}", subject.trim())
    }
}

pub fn forward_subject(subject: &str) -> String {
    if has_prefix(subject, &["fwd:", "fw:", "转发:", "转发："]) {
        subject.trim().to_string()
    } else {
        format!("Fwd: {}", subject.trim())
    }
}

fn format_date(ts: i64) -> String {
    match chrono::Local.timestamp_opt(ts, 0).single() {
        Some(d) if crate::i18n::zh() => d.format("%Y年%-m月%-d日 %H:%M").to_string(),
        Some(d) if crate::i18n::hour24() => d.format("%a, %b %-d, %Y at %H:%M").to_string(),
        Some(d) => d.format("%a, %b %-d, %Y at %-I:%M %p").to_string(),
        None => String::new(),
    }
}

fn quote(text: &str) -> String {
    let trimmed = text.trim_end();
    trimmed
        .lines()
        .map(|l| if l.starts_with('>') { format!(">{l}") } else if l.is_empty() { ">".to_string() } else { format!("> {l}") })
        .collect::<Vec<_>>()
        .join("\n")
}

fn display(a: &EmailAddress) -> String {
    if a.name.is_empty() { a.address.clone() } else { format!("{} <{}>", a.name, a.address) }
}

/// The account's own address among `mine` that a message involved, to answer from it.
/// None when it was the account's main address (the default sender).
fn own_address(account: &Account, mine: &[EmailAddress], candidates: &[&EmailAddress]) -> Option<EmailAddress> {
    let primary = account.email.to_lowercase();
    candidates.iter().find_map(|c| {
        let k = c.address.to_lowercase();
        if k == primary {
            return Some(None);
        }
        mine.iter().find(|m| m.address.to_lowercase() == k).map(|m| Some(m.clone()))
    })?
}

/// `mine`: every address the account sends from (aliases too), so replies come from the address
/// the message was sent to and reply-all doesn't copy the user to themselves.
pub fn reply_draft(account: &Account, mine: &[EmailAddress], raw: &[u8], local_id: i64, date: i64, all: bool) -> Draft {
    let p = mime::parse(raw);
    let text = mime::parse_message(raw).map(|m| mime::plain_text(&m)).unwrap_or_default();
    let me = account.email.to_lowercase();
    let is_mine = |a: &str| { let a = a.to_lowercase(); a == me || mine.iter().any(|m| m.address.to_lowercase() == a) };
    let primary: Vec<EmailAddress> = if !p.reply_to.is_empty() { p.reply_to.clone() } else { vec![p.from.clone()] };
    let from_me = is_mine(&p.from.address);
    let sender = if from_me {
        own_address(account, mine, &[&p.from])
    } else {
        own_address(account, mine, &p.to.iter().chain(p.cc.iter()).collect::<Vec<_>>())
    };
    let mut to: Vec<EmailAddress> = if from_me && !p.to.is_empty() { p.to.clone() } else { primary };
    let mut cc: Vec<EmailAddress> = Vec::new();
    if all {
        let mut seen: Vec<String> = to.iter().map(|a| a.address.to_lowercase()).collect();
        seen.push(me.clone());
        for a in p.to.iter().chain(p.cc.iter()) {
            let k = a.address.to_lowercase();
            if !k.is_empty() && !seen.contains(&k) && !is_mine(&k) {
                seen.push(k);
                cc.push(a.clone());
            }
        }
    }
    to.retain(|a| !a.address.is_empty());
    let header = t!("On {}, {} wrote:", "在 {}，{} 写道：", format_date(p.date.unwrap_or(date)), display(&p.from));
    let body = format!("{}\n\n{}\n{}\n", signature_block(account), header, quote(&text));
    let mut references = p.references.clone();
    if let Some(id) = &p.message_id {
        references.push(id.clone());
    }
    Draft {
        account_id: account.id,
        to: join(&to),
        cc: join(&cc),
        bcc: String::new(),
        subject: reply_subject(&p.subject),
        body,
        attachments: Vec::new(),
        in_reply_to: p.message_id.clone(),
        references,
        reply_to_message: Some(local_id),
        forward_from: None,
        draft_source: None,
        from: sender,
    }
}

pub fn forward_draft(account: &Account, raw: &[u8], local_id: i64, date: i64) -> Draft {
    let p: Parsed = mime::parse(raw);
    let text = mime::parse_message(raw).map(|m| mime::plain_text(&m)).unwrap_or_default();
    let mut header = vec![
        t!("---------- Forwarded message ----------", "---------- 转发的邮件 ----------"),
        t!("From: {}", "发件人：{}", display(&p.from)),
        t!("Date: {}", "日期：{}", format_date(p.date.unwrap_or(date))),
        t!("Subject: {}", "主题：{}", p.subject),
        t!("To: {}", "收件人：{}", p.to.iter().map(display).collect::<Vec<_>>().join(", ")),
    ];
    if !p.cc.is_empty() {
        header.push(t!("Cc: {}", "抄送：{}", p.cc.iter().map(display).collect::<Vec<_>>().join(", ")));
    }
    Draft {
        account_id: account.id,
        to: String::new(),
        cc: String::new(),
        bcc: String::new(),
        subject: forward_subject(&p.subject),
        body: format!("{}\n\n{}\n\n{}\n", signature_block(account), header.join("\n"), text.trim_end()),
        attachments: Vec::new(),
        in_reply_to: None,
        references: Vec::new(),
        reply_to_message: None,
        forward_from: Some(local_id),
        draft_source: None,
        from: None,
    }
}

/// Parses a user-typed address list. Accepts commas, semicolons and full-width commas.
pub fn parse_addresses(input: &str) -> Result<Vec<Mailbox>> {
    let normalized = input.replace(['；', ';', '，', '\n'], ",");
    let mut out = Vec::new();
    for piece in split_outside_quotes(&normalized) {
        let piece = piece.trim();
        if piece.is_empty() {
            continue;
        }
        let parsed: Mailboxes = piece
            .parse()
            .or_else(|_| {
                // "张三 <a@b.c>" with unquoted non-ASCII names
                if let (Some(l), Some(r)) = (piece.rfind('<'), piece.rfind('>')) {
                    let name = piece[..l].trim().trim_matches('"').to_string();
                    let addr: Address = piece[l + 1..r].trim().parse()?;
                    return Ok(Mailboxes::from(Mailbox::new(if name.is_empty() { None } else { Some(name) }, addr)));
                }
                Err(lettre::address::AddressError::MissingParts)
            })
            .map_err(|_| CoreError::invalid(t!("\"{piece}\" isn't a valid email address", "「{piece}」不是有效的邮箱地址")))?;
        out.extend(parsed.into_iter());
    }
    Ok(out)
}

fn split_outside_quotes(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in s.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                cur.push(c);
            }
            ',' if !quoted => out.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    out.push(cur);
    out
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn new_message_id(email: &str) -> String {
    use std::hash::{BuildHasher, Hasher};
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(n);
    h.write_i64(chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default());
    let domain = email.rsplit('@').next().filter(|d| !d.is_empty()).unwrap_or("litemail.local");
    format!("{:x}.{:016x}@{}", chrono::Utc::now().timestamp_millis(), h.finish(), domain)
}

pub struct Outgoing {
    pub raw: Vec<u8>,
    pub envelope: Envelope,
    pub message_id: String,
}

/// Attachment payload: (file name, mime type, bytes).
pub type Payload = (String, String, Vec<u8>);

pub fn build(account: &Account, draft: &Draft, files: Vec<Payload>) -> Result<Outgoing> {
    build_message(account, draft, files, true)
}

/// Like `build`, but recipients may be empty (drafts).
pub fn build_message(account: &Account, draft: &Draft, files: Vec<Payload>, require_recipient: bool) -> Result<Outgoing> {
    let (from_name, from_email) = match &draft.from {
        Some(a) if !a.address.trim().is_empty() => (if a.name.trim().is_empty() { account.display_name.trim() } else { a.name.trim() }, a.address.trim()),
        _ => (account.display_name.trim(), account.email.trim()),
    };
    let from_addr: Address = from_email.parse().map_err(|_| CoreError::invalid(t!("The sender address is invalid", "发件地址无效")))?;
    let from = Mailbox::new(if from_name.is_empty() { None } else { Some(from_name.to_string()) }, from_addr.clone());
    let to = parse_addresses(&draft.to)?;
    let cc = parse_addresses(&draft.cc)?;
    let bcc = parse_addresses(&draft.bcc)?;
    if require_recipient && to.is_empty() && cc.is_empty() && bcc.is_empty() {
        return Err(CoreError::invalid(t!("Add at least one recipient", "请至少填写一个收件人")));
    }
    let no_recipients = to.is_empty() && cc.is_empty() && bcc.is_empty();
    let message_id = new_message_id(&account.email);
    let mut b = Message::builder()
        .from(from)
        .subject(draft.subject.trim())
        .message_id(Some(format!("<{message_id}>")))
        .user_agent(format!("LiteMail/{}", env!("CARGO_PKG_VERSION")))
        .date_now();
    for m in to {
        b = b.to(m);
    }
    for m in cc {
        b = b.cc(m);
    }
    for m in bcc {
        b = b.bcc(m);
    }
    if no_recipients {
        // Drafts are never delivered; lettre still wants an envelope.
        let env = Envelope::new(Some(from_addr.clone()), vec![from_addr]).map_err(|e| CoreError::invalid(e.to_string()))?;
        b = b.envelope(env);
    }
    if let Some(irt) = draft.in_reply_to.as_deref().filter(|s| !s.is_empty()) {
        b = b.in_reply_to(format!("<{irt}>"));
    }
    if !draft.references.is_empty() {
        let refs: Vec<String> = draft.references.iter().filter(|r| !r.is_empty()).map(|r| format!("<{r}>")).collect();
        b = b.references(refs.join(" "));
    }
    let body = normalize_newlines(&draft.body);
    let text = SinglePart::plain(body);
    let msg = if files.is_empty() {
        b.singlepart(text)
    } else {
        let mut mp = MultiPart::mixed().singlepart(text);
        for (name, mime_type, bytes) in files {
            let ct = ContentType::parse(&mime_type).unwrap_or(ContentType::parse("application/octet-stream").unwrap());
            mp = mp.singlepart(Attachment::new(name).body(bytes, ct));
        }
        b.multipart(mp)
    }
    .map_err(|e| CoreError::invalid(t!("Couldn't build the message: {e}", "无法生成邮件：{e}")))?;
    Ok(Outgoing { raw: msg.formatted(), envelope: msg.envelope().clone(), message_id })
}

fn normalize_newlines(s: &str) -> String {
    s.replace("\r\n", "\n")
}

/// Guesses a MIME type from a file extension.
pub fn mime_for(path: &str) -> String {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "heic" => "image/heic",
        "webp" => "image/webp",
        "txt" | "md" => "text/plain",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "zip" => "application/zip",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "key" => "application/x-iwork-keynote-sffkey",
        "pages" => "application/x-iwork-pages-sffpages",
        "numbers" => "application/x-iwork-numbers-sffnumbers",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "ics" => "text/calendar",
        "eml" => "message/rfc822",
        _ => "application/octet-stream",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;

    fn account() -> Account {
        Account {
            id: 1,
            email: "me@example.com".into(),
            display_name: "我".into(),
            provider: ProviderKind::Other,
            imap: ServerConfig { host: "h".into(), port: 993, security: Security::Tls },
            smtp: ServerConfig { host: "h".into(), port: 465, security: Security::Tls },
            username: "me".into(),
            sync_days: 30,
            sync_interval_minutes: 5,
            server_saves_sent: false,
            signature: "Alex\n发自 LiteMail".into(),
            notify: true,
            allow_invalid_certs: false,
        }
    }

    const ORIGINAL: &[u8] = b"From: =?UTF-8?B?5byg5LiJ?= <zhang@corp.cn>\r\n\
Reply-To: team@corp.cn\r\n\
To: me@example.com, Bob <bob@corp.cn>\r\n\
Cc: Carol <carol@corp.cn>, ME <ME@example.com>\r\n\
Subject: =?UTF-8?B?5ZGo5Lya?=\r\n\
Date: Sun, 18 Oct 2026 09:41:00 +0800\r\n\
Message-ID: <orig@corp.cn>\r\n\
References: <root@corp.cn>\r\n\
Content-Type: text/plain; charset=utf-8\r\n\r\n\
line one\r\n> older quote\r\n\r\nline three\r\n";

    #[test]
    fn subjects() {
        assert_eq!(reply_subject("Hello"), "Re: Hello");
        assert_eq!(reply_subject("RE: Hello"), "RE: Hello");
        assert_eq!(reply_subject("回复：你好"), "回复：你好");
        assert_eq!(forward_subject("Hello"), "Fwd: Hello");
        assert_eq!(forward_subject("FW: x"), "FW: x");
    }

    #[test]
    fn reply_uses_reply_to_and_threads() {
        let d = reply_draft(&account(), &[], ORIGINAL, 9, 0, false);
        assert_eq!(d.to, "team@corp.cn");
        assert_eq!(d.cc, "");
        assert_eq!(d.subject, "Re: 周会");
        assert_eq!(d.in_reply_to.as_deref(), Some("orig@corp.cn"));
        assert_eq!(d.references, vec!["root@corp.cn", "orig@corp.cn"]);
        assert_eq!(d.reply_to_message, Some(9));
        assert!(d.body.starts_with("\n\n-- \nAlex\n发自 LiteMail\n\nOn "), "{}", d.body);
        assert!(d.body.contains("张三 <zhang@corp.cn> wrote:\n> line one\n>> older quote\n>\n> line three"), "{}", d.body);
        let zh = crate::i18n::with_language(true, || reply_draft(&account(), &[], ORIGINAL, 9, 0, false));
        assert!(zh.body.contains("张三 <zhang@corp.cn> 写道：\n> line one"), "{}", zh.body);
    }

    #[test]
    fn reply_all_dedupes_and_skips_me() {
        let d = reply_draft(&account(), &[], ORIGINAL, 9, 0, true);
        assert_eq!(d.to, "team@corp.cn");
        assert_eq!(d.cc, "Bob <bob@corp.cn>, Carol <carol@corp.cn>");
    }

    #[test]
    fn replies_come_from_the_alias_the_mail_was_sent_to() {
        let alias = EmailAddress { name: "Alex Work".into(), address: "alex@work.example".into() };
        let raw = b"From: Bob <bob@corp.cn>\r\nTo: alex@WORK.example\r\nCc: me@example.com, Amy <amy@corp.cn>\r\n\
Subject: Hi\r\nMessage-ID: <a@corp.cn>\r\n\r\nhello\r\n";
        let d = reply_draft(&account(), &[alias.clone()], raw, 3, 0, true);
        assert_eq!(d.from, Some(alias.clone()), "answers from the address it was sent to");
        assert_eq!(d.cc, "Amy <amy@corp.cn>", "none of my addresses are copied");
        // Sent to the main address: the default sender (None).
        assert_eq!(reply_draft(&account(), &[alias.clone()], ORIGINAL, 9, 0, false).from, None);
        // The alias ends up in the From header and the envelope.
        let mut out_draft = d.clone();
        out_draft.to = "bob@corp.cn".into();
        let out = build(&account(), &out_draft, Vec::new()).unwrap();
        let text = String::from_utf8_lossy(&out.raw);
        assert!(text.contains("From: \"Alex Work\" <alex@work.example>") || text.contains("From: Alex Work <alex@work.example>"), "{text}");
        assert_eq!(out.envelope.from().map(|a| a.to_string()), Some("alex@work.example".into()));
        // Editing a saved draft keeps its alias.
        let e = edit_draft(&account(), &[alias.clone()], &out.raw, 7);
        assert_eq!(e.from.map(|a| a.address), Some("alex@work.example".into()));
    }

    #[test]
    fn forward_keeps_header_block() {
        let d = forward_draft(&account(), ORIGINAL, 9, 0);
        assert_eq!(d.subject, "Fwd: 周会");
        assert_eq!(d.forward_from, Some(9));
        assert!(d.body.contains("From: 张三 <zhang@corp.cn>"));
        assert!(d.body.contains("line one\n> older quote"));
    }

    #[test]
    fn edit_draft_round_trip() {
        let mut d = new_draft(&account());
        d.to = "张三 <z@c.cn>".into();
        d.bcc = "hidden@x.com".into();
        d.subject = "草稿".into();
        d.body = "还没写完".into();
        let out = build_message(&account(), &d, vec![], true).unwrap();
        let e = edit_draft(&account(), &[], &out.raw, 5);
        assert_eq!(e.to, "张三 <z@c.cn>");
        assert_eq!(e.subject, "草稿");
        assert_eq!(e.body, "还没写完");
        assert_eq!(e.draft_source, Some(5));
        assert_eq!(e.forward_from, None);
        let empty = new_draft(&account());
        assert!(build_message(&account(), &empty, vec![], false).is_ok(), "drafts may have no recipients");
    }

    #[test]
    fn address_parsing() {
        let v = parse_addresses("a@b.com， 张三 <z@c.cn>; \"Doe, John\" <j@d.com>").unwrap();
        assert_eq!(v.len(), 3);
        assert_eq!(v[1].name.as_deref(), Some("张三"));
        assert_eq!(v[2].email.to_string(), "j@d.com");
        assert!(parse_addresses("not an address").is_err());
        assert!(parse_addresses("  ").unwrap().is_empty());
    }

    #[test]
    fn builds_mime_without_bcc_header() {
        let mut d = new_draft(&account());
        d.to = "张三 <z@c.cn>".into();
        d.bcc = "secret@hidden.com".into();
        d.subject = "测试 subject".into();
        d.body = "第一行\n.leading dot\n".repeat(3);
        d.in_reply_to = Some("orig@corp.cn".into());
        d.references = vec!["root@corp.cn".into(), "orig@corp.cn".into()];
        let out = build(&account(), &d, vec![("报告.pdf".into(), "application/pdf".into(), b"%PDF-1.4".to_vec())]).unwrap();
        let raw = String::from_utf8_lossy(&out.raw).to_string();
        assert!(!raw.to_lowercase().contains("bcc:"), "Bcc must not be in headers");
        assert!(out.envelope.to().iter().any(|a| a.to_string() == "secret@hidden.com"), "but in the envelope");
        assert!(raw.contains("In-Reply-To: <orig@corp.cn>"));
        assert!(raw.contains("References: <root@corp.cn> <orig@corp.cn>"));
        assert!(raw.contains(&format!("Message-ID: <{}>", out.message_id)));
        // round-trip through our own parser
        let p = mime::parse(&out.raw);
        assert_eq!(p.subject, "测试 subject");
        assert_eq!(p.to[0].name, "张三");
        assert!(p.has_attachments);
        let atts = mime::attachments(&out.raw);
        assert_eq!(atts[0].filename, "报告.pdf");
        let text = mime::plain_text(&mime::parse_message(&out.raw).unwrap());
        assert!(text.contains("第一行\n.leading dot"), "{text}");
    }

    #[test]
    fn requires_a_recipient() {
        let d = new_draft(&account());
        assert!(matches!(build(&account(), &d, vec![]), Err(CoreError::InvalidInput { .. })));
    }
}
