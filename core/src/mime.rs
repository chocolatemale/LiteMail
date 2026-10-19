//! Message parsing and rendering. Parsing is delegated to `mail-parser`, which handles
//! the long tail of charsets and malformed MIME. This module turns a parsed message into
//! what the UI needs: header fields, a preview, searchable text and a locked-down HTML page.

use std::collections::HashMap;

use base64::Engine as _;
use mail_parser::{Address, HeaderValue, Message, MessageParser, MimeHeaders, PartType};

use crate::types::{AttachmentInfo, EmailAddress};

const SNIPPET_CHARS: usize = 180;
const SEARCH_TEXT_CHARS: usize = 20_000;

#[derive(Debug, Clone, Default)]
pub struct Parsed {
    pub subject: String,
    pub from: EmailAddress,
    pub to: Vec<EmailAddress>,
    pub cc: Vec<EmailAddress>,
    pub reply_to: Vec<EmailAddress>,
    pub date: Option<i64>,
    pub message_id: Option<String>,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    pub snippet: String,
    pub search_text: String,
    pub has_attachments: bool,
    pub list_unsubscribe: Option<String>,
}

pub fn parse_message(raw: &[u8]) -> Option<Message<'_>> {
    MessageParser::default().parse(raw)
}

pub fn parse(raw: &[u8]) -> Parsed {
    let Some(msg) = parse_message(raw) else {
        return Parsed { subject: String::new(), ..Default::default() };
    };
    let text = plain_text(&msg);
    Parsed {
        subject: clean_header(msg.subject().unwrap_or_default()),
        from: msg.from().and_then(|a| addresses(Some(a)).into_iter().next()).unwrap_or_default_addr(),
        to: addresses(msg.to()),
        cc: addresses(msg.cc()),
        reply_to: addresses(msg.reply_to()),
        date: msg.date().map(|d| d.to_timestamp()),
        message_id: msg.message_id().map(|s| s.to_string()),
        in_reply_to: header_ids(msg.in_reply_to()).into_iter().next(),
        references: header_ids(msg.references()),
        snippet: snippet(&text),
        search_text: truncate_chars(&text, SEARCH_TEXT_CHARS),
        has_attachments: !visible_attachments(&msg).is_empty(),
        list_unsubscribe: list_unsubscribe(&msg),
    }
}

trait OrDefaultAddr {
    fn unwrap_or_default_addr(self) -> EmailAddress;
}

impl OrDefaultAddr for Option<EmailAddress> {
    fn unwrap_or_default_addr(self) -> EmailAddress {
        self.unwrap_or(EmailAddress { name: String::new(), address: String::new() })
    }
}

impl Default for EmailAddress {
    fn default() -> Self {
        EmailAddress { name: String::new(), address: String::new() }
    }
}

fn clean_header(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn addresses(addr: Option<&Address<'_>>) -> Vec<EmailAddress> {
    let Some(addr) = addr else { return Vec::new() };
    addr.iter()
        .filter_map(|a| {
            let address = a.address().unwrap_or_default().trim().to_string();
            let name = clean_header(a.name().unwrap_or_default()).trim_matches('"').to_string();
            if address.is_empty() && name.is_empty() {
                None
            } else {
                Some(EmailAddress { name, address })
            }
        })
        .collect()
}

fn header_ids(v: &HeaderValue<'_>) -> Vec<String> {
    match v {
        HeaderValue::Text(t) => vec![t.trim_matches(['<', '>']).to_string()],
        HeaderValue::TextList(list) => list.iter().map(|t| t.trim_matches(['<', '>']).to_string()).collect(),
        _ => Vec::new(),
    }
}

fn list_unsubscribe(msg: &Message<'_>) -> Option<String> {
    let raw = msg.header_raw("List-Unsubscribe")?;
    let mut fallback = None;
    for part in raw.split(',') {
        let url = part.trim().trim_matches(['<', '>']).trim();
        if url.starts_with("https://") {
            return Some(url.to_string());
        }
        if url.starts_with("mailto:") && fallback.is_none() {
            fallback = Some(url.to_string());
        }
    }
    fallback
}

/// Plain text of every text body (HTML-only messages are converted).
pub fn plain_text(msg: &Message<'_>) -> String {
    let mut out = String::new();
    for i in 0..msg.text_body_count() {
        if let Some(t) = msg.body_text(i) {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&t);
        }
    }
    if out.trim().is_empty() {
        for i in 0..msg.html_body_count() {
            if let Some(h) = msg.body_html(i) {
                out.push_str(&mail_parser::decoders::html::html_to_text(&h));
            }
        }
    }
    out.replace("\r\n", "\n")
}

fn truncate_chars(s: &str, n: usize) -> String {
    match s.char_indices().nth(n) {
        Some((i, _)) => s[..i].to_string(),
        None => s.to_string(),
    }
}

/// One-line preview: skips quoted text, reply headers and signatures.
pub fn snippet(text: &str) -> String {
    let mut words = String::new();
    for line in text.lines() {
        let t = line.trim();
        if t == "--" || t == "-- " || t.starts_with("-----") || t.starts_with("________") {
            break;
        }
        if t.starts_with('>') || t.is_empty() {
            continue;
        }
        if is_reply_header(t) {
            break;
        }
        if !words.is_empty() {
            words.push(' ');
        }
        words.push_str(t);
        if words.chars().count() > SNIPPET_CHARS * 2 {
            break;
        }
    }
    let collapsed = words.split_whitespace().collect::<Vec<_>>().join(" ");
    let cut = truncate_chars(&collapsed, SNIPPET_CHARS);
    if cut.len() < collapsed.len() { format!("{cut}…") } else { cut }
}

fn is_reply_header(line: &str) -> bool {
    (line.starts_with("On ") && line.ends_with("wrote:"))
        || (line.starts_with('在') && (line.ends_with("写道：") || line.ends_with("写道:")))
        || line.starts_with("发件人：")
        || line.starts_with("From: ")
}

fn content_id(part: &mail_parser::MessagePart<'_>) -> Option<String> {
    part.content_id().map(|c| c.trim().trim_matches(['<', '>']).to_string()).filter(|c| !c.is_empty())
}

fn mime_type(part: &mail_parser::MessagePart<'_>) -> String {
    match part.content_type() {
        Some(ct) => match ct.subtype() {
            Some(sub) => format!("{}/{}", ct.ctype(), sub).to_ascii_lowercase(),
            None => ct.ctype().to_ascii_lowercase(),
        },
        None if part.is_message() => "message/rfc822".into(),
        None => "application/octet-stream".into(),
    }
}

fn is_attachment_disposition(part: &mail_parser::MessagePart<'_>) -> bool {
    part.content_disposition().is_some_and(|cd| cd.ctype().eq_ignore_ascii_case("attachment"))
}

/// Attachments the user should see: everything except inline images that the HTML
/// body shows through `cid:` references.
fn visible_attachments<'a>(msg: &'a Message<'_>) -> Vec<(u32, &'a mail_parser::MessagePart<'a>)> {
    let html = html_source(msg).unwrap_or_default().to_ascii_lowercase();
    msg.attachments()
        .enumerate()
        .filter(|(_, part)| {
            if is_attachment_disposition(part) {
                return true;
            }
            match content_id(part) {
                Some(cid) => !html.contains(&format!("cid:{}", cid.to_ascii_lowercase())),
                None => true,
            }
        })
        .map(|(i, part)| (i as u32, part))
        .collect()
}

fn attachment_name(part: &mail_parser::MessagePart<'_>, index: u32) -> String {
    if let Some(name) = part.attachment_name() {
        let name = name.trim().replace(['/', '\\', ':'], "_");
        if !name.is_empty() {
            return name;
        }
    }
    if let Some(inner) = part.message() {
        let subject = inner.subject().unwrap_or("message").trim();
        let subject: String = subject.chars().filter(|c| !"/\\:".contains(*c)).take(60).collect();
        return format!("{}.eml", if subject.is_empty() { "message".into() } else { subject });
    }
    let mime = mime_type(part);
    let ext = match mime.as_str() {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "application/pdf" => "pdf",
        "text/calendar" => "ics",
        "text/plain" => "txt",
        _ => "bin",
    };
    t!("Attachment {}.{}", "附件 {}.{}", index + 1, ext)
}

pub fn attachments(raw: &[u8]) -> Vec<AttachmentInfo> {
    let Some(msg) = parse_message(raw) else { return Vec::new() };
    visible_attachments(&msg)
        .into_iter()
        .map(|(index, part)| AttachmentInfo {
            index,
            filename: attachment_name(part, index),
            mime_type: mime_type(part),
            size: part.len() as u64,
        })
        .collect()
}

/// Returns (filename, mime type, bytes) for the attachment at `index`
/// (position in the message's attachment list).
pub fn attachment(raw: &[u8], index: u32) -> Option<(String, String, Vec<u8>)> {
    let msg = parse_message(raw)?;
    let part = msg.attachments().nth(index as usize)?;
    Some((attachment_name(part, index), mime_type(part), part.contents().to_vec()))
}

/// All visible attachments, used when forwarding.
pub fn all_attachments(raw: &[u8]) -> Vec<(String, String, Vec<u8>)> {
    let Some(msg) = parse_message(raw) else { return Vec::new() };
    visible_attachments(&msg)
        .into_iter()
        .map(|(i, part)| (attachment_name(part, i), mime_type(part), part.contents().to_vec()))
        .collect()
}

fn html_source(msg: &Message<'_>) -> Option<String> {
    let mut html = String::new();
    let mut has_html = false;
    for part in msg.html_bodies() {
        match &part.body {
            PartType::Html(h) => {
                has_html = true;
                html.push_str(h);
            }
            PartType::Text(t) => html.push_str(&text_to_html(t)),
            _ => {}
        }
    }
    has_html.then_some(html)
}

// ---------------------------------------------------------------------------------------
// Rendering

pub struct Rendered {
    pub html: String,
    pub has_remote_content: bool,
}

const BASE_CSS: &str = r#"
:root { color-scheme: light dark; }
html, body { margin: 0; padding: 0; }
body { font: 14px/1.6 -apple-system, BlinkMacSystemFont, "PingFang SC", "Hiragino Sans GB", sans-serif;
  color: #1d1d1f; background: transparent; -webkit-font-smoothing: antialiased;
  overflow-wrap: break-word; word-wrap: break-word; -webkit-text-size-adjust: 100%; }
body.plain { padding: 2px 0 24px; }
.text { white-space: pre-wrap; }
.text blockquote { margin: 4px 0; padding: 0 0 0 12px; border-left: 2px solid rgba(60,60,67,.22); color: #6e6e73; white-space: pre-wrap; }
.text .sig { color: #8a8a8e; }
a { color: #0066d6; text-decoration: none; }
a:hover { text-decoration: underline; }
img { max-width: 100%; height: auto; }
table { max-width: 100%; }
pre, code { white-space: pre-wrap; font-family: ui-monospace, "SF Mono", Menlo, monospace; font-size: 12.5px; }
body.designed { background: #ffffff; color: #000000; padding: 20px; border-radius: 12px; overflow-x: auto; }
body.simple { padding: 2px 0 24px; overflow-x: auto; }
@media (prefers-color-scheme: dark) {
  body.plain, body.simple { color: #f2f2f7; }
  body.plain a, body.simple a { color: #4da3ff; }
  .text blockquote { border-left-color: rgba(235,235,245,.24); color: #a1a1a6; }
  .text .sig { color: #8d8d93; }
}
"#;

/// The message body as HTML, ready to embed: cid images inlined, plain text converted.
pub struct Content {
    pub html: String,
    /// "designed" (sets its own colours), "simple" (plain HTML) or "plain" (text).
    pub class: &'static str,
    pub has_remote: bool,
}

pub fn content(raw: &[u8]) -> Content {
    let Some(msg) = parse_message(raw) else {
        return Content {
            html: format!("<div class=\"text\">{}</div>", text_to_html(&String::from_utf8_lossy(raw))),
            class: "plain",
            has_remote: false,
        };
    };
    match html_source(&msg) {
        Some(html) => {
            let html = inline_cid_images(&msg, &html);
            let has_remote = has_remote_resources(&html);
            let class = if is_designed(&html) { "designed" } else { "simple" };
            Content { html, class, has_remote }
        }
        None => Content {
            html: format!("<div class=\"text\">{}</div>", text_to_html(&plain_text(&msg))),
            class: "plain",
            has_remote: false,
        },
    }
}

pub fn csp(allow_remote: bool) -> String {
    let remote = if allow_remote { " https: http:" } else { "" };
    format!("default-src 'none'; img-src data:{remote}; style-src 'unsafe-inline'{remote}; font-src data: litemail-asset:{remote}; media-src data:{remote}")
}

pub fn render(raw: &[u8], allow_remote: bool) -> Rendered {
    let c = content(raw);
    wrap(&c.html, c.class, c.has_remote, allow_remote)
}

fn wrap(content: &str, class: &str, has_remote: bool, allow_remote: bool) -> Rendered {
    let csp = csp(allow_remote);
    let html = format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\">\
         <meta http-equiv=\"Content-Security-Policy\" content=\"{csp}\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <meta name=\"referrer\" content=\"no-referrer\">\
         <style>{BASE_CSS}</style></head><body class=\"{class}\">{content}</body></html>"
    );
    Rendered { html, has_remote_content: has_remote }
}

/// Emails that set their own colours are shown on a white card even in dark mode,
/// like Apple Mail does; plain HTML without colours adapts to the system appearance.
pub fn is_designed(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    ["bgcolor", "background", "color:", "color =", "color=", "<font", "<table"]
        .iter()
        .any(|p| lower.contains(p))
}

fn inline_cid_images(msg: &Message<'_>, html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    if !lower.contains("cid:") {
        return html.to_string();
    }
    let mut map: HashMap<String, String> = HashMap::new();
    for part in msg.parts.iter() {
        let Some(cid) = content_id(part) else { continue };
        if part.is_multipart() || part.is_text() {
            continue;
        }
        let data = base64::engine::general_purpose::STANDARD.encode(part.contents());
        map.insert(cid.to_ascii_lowercase(), format!("data:{};base64,{}", mime_type(part), data));
    }
    if map.is_empty() {
        return html.to_string();
    }
    let mut out = String::with_capacity(html.len());
    let mut i = 0;
    while let Some(pos) = lower[i..].find("cid:") {
        let start = i + pos;
        out.push_str(&html[i..start]);
        let rest = &html[start + 4..];
        let end = rest.find(|c: char| c == '"' || c == '\'' || c == ')' || c == '>' || c.is_whitespace()).unwrap_or(rest.len());
        let id = &rest[..end];
        match map.get(&percent_decode(id).to_ascii_lowercase()) {
            Some(uri) => out.push_str(uri),
            None => out.push_str(&html[start..start + 4 + end]),
        }
        i = start + 4 + end;
    }
    out.push_str(&html[i..]);
    out
}

fn percent_decode(s: &str) -> String {
    if !s.contains('%') {
        return s.to_string();
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Does the HTML load anything from the network (tracking pixels, remote images, fonts)?
pub fn has_remote_resources(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    let b = lower.as_bytes();
    for attr in ["src", "srcset", "background", "poster", "href"] {
        let mut from = 0;
        while let Some(pos) = lower[from..].find(attr) {
            let start = from + pos;
            from = start + attr.len();
            // must be an attribute name: preceded by whitespace or quote
            if start == 0 || !(b[start - 1].is_ascii_whitespace() || b[start - 1] == b'"' || b[start - 1] == b'\'') {
                continue;
            }
            let mut j = from;
            while j < b.len() && b[j].is_ascii_whitespace() {
                j += 1;
            }
            if j >= b.len() || b[j] != b'=' {
                continue;
            }
            j += 1;
            while j < b.len() && (b[j].is_ascii_whitespace() || b[j] == b'"' || b[j] == b'\'') {
                j += 1;
            }
            let value = &lower[j..];
            if value.starts_with("http:") || value.starts_with("https:") || value.starts_with("//") {
                if attr == "href" {
                    // only stylesheets load from href
                    let tag_start = lower[..start].rfind('<').unwrap_or(0);
                    if lower[tag_start..start].starts_with("<link") {
                        return true;
                    }
                    continue;
                }
                return true;
            }
        }
    }
    let mut from = 0;
    while let Some(pos) = lower[from..].find("url(") {
        let mut j = from + pos + 4;
        while j < b.len() && (b[j].is_ascii_whitespace() || b[j] == b'"' || b[j] == b'\'') {
            j += 1;
        }
        let value = &lower[j..];
        if value.starts_with("http:") || value.starts_with("https:") || value.starts_with("//") {
            return true;
        }
        from = j;
    }
    lower.contains("@import")
}

const BLANK_GIF: &str = "data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7";

/// One attribute of a start tag; spans are byte offsets into the tag text.
struct TagAttr {
    name: String,
    name_span: (usize, usize),
    value: Option<(usize, usize)>,
}

/// Parses the attributes of a start tag such as `<img src="a" width=10>`.
fn tag_attrs(tag: &str) -> Vec<TagAttr> {
    let b = tag.as_bytes();
    let mut i = tag.find(|c: char| c.is_ascii_whitespace() || c == '/' || c == '>').unwrap_or(b.len());
    let mut out = Vec::new();
    loop {
        while i < b.len() && (b[i].is_ascii_whitespace() || b[i] == b'/') {
            i += 1;
        }
        if i >= b.len() || b[i] == b'>' {
            return out;
        }
        let start = i;
        while i < b.len() && !b[i].is_ascii_whitespace() && !matches!(b[i], b'=' | b'>' | b'/') {
            i += 1;
        }
        let name_span = (start, i);
        let mut j = i;
        while j < b.len() && b[j].is_ascii_whitespace() {
            j += 1;
        }
        let mut value = None;
        if j < b.len() && b[j] == b'=' {
            j += 1;
            while j < b.len() && b[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < b.len() && (b[j] == b'"' || b[j] == b'\'') {
                let q = b[j];
                let v = j + 1;
                let end = tag[v..].find(q as char).map(|e| v + e).unwrap_or(b.len());
                value = Some((v, end));
                i = (end + 1).min(b.len());
            } else {
                let v = j;
                while j < b.len() && !b[j].is_ascii_whitespace() && b[j] != b'>' {
                    j += 1;
                }
                value = Some((v, j));
                i = j;
            }
        }
        out.push(TagAttr { name: tag[name_span.0..name_span.1].to_ascii_lowercase(), name_span, value });
    }
}

fn is_remote_url(v: &str) -> bool {
    let v = v.trim_start().to_ascii_lowercase();
    v.starts_with("http:") || v.starts_with("https:") || v.starts_with("//")
}

/// With remote content blocked, a remote `<img>` shows WebKit's broken-image frame and its alt
/// text. Swap it for a transparent pixel: an image with a known size keeps that size as a soft
/// placeholder (`data-lm-blocked="box"`); tracking pixels and unsized images are hidden.
pub fn placeholder_remote_images(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len());
    let mut i = 0;
    while let Some(pos) = lower[i..].find("<img") {
        let start = i + pos;
        let after = lower.as_bytes().get(start + 4).copied().unwrap_or(b'>');
        let Some(len) = lower[start..].find('>') else { break };
        let end = start + len + 1;
        out.push_str(&html[i..start]);
        i = end;
        let tag = &html[start..end];
        if !(after.is_ascii_whitespace() || after == b'/' || after == b'>') {
            out.push_str(tag);
            continue;
        }
        let attrs = tag_attrs(tag);
        let val = |a: &TagAttr| a.value.map(|(s, e)| &tag[s..e]).unwrap_or("");
        let src = attrs.iter().find(|a| a.name == "src");
        let srcset = attrs.iter().find(|a| a.name == "srcset");
        let remote = src.is_some_and(|a| is_remote_url(val(a))) || srcset.is_some_and(|a| val(a).contains("http"));
        if !remote {
            out.push_str(tag);
            continue;
        }
        let px = |name: &str| -> Option<u32> {
            let v = val(attrs.iter().find(|a| a.name == name)?).trim();
            v.strip_suffix("px").unwrap_or(v).trim().parse().ok()
        };
        let size = match (px("width"), px("height")) {
            (Some(w), Some(h)) if w > 2 && h > 2 => Some((w, h)),
            _ => None,
        };
        // Edits as (start, end, replacement), applied back to front.
        let mut edits: Vec<(usize, usize, String)> = Vec::new();
        let mut head = format!(" data-lm-blocked=\"{}\"", if size.is_some() { "box" } else { "hide" });
        match src.and_then(|a| a.value) {
            Some((s, e)) => edits.push((s, e, BLANK_GIF.to_string())),
            None => head.push_str(&format!(" src=\"{BLANK_GIF}\"")),
        }
        if let Some(a) = srcset {
            edits.push((a.name_span.0, a.name_span.1, "data-lm-srcset".into()));
        }
        if let Some((w, h)) = size {
            let dims = format!("width:{w}px;height:{h}px");
            match attrs.iter().find(|a| a.name == "style").and_then(|a| a.value) {
                Some((_, e)) => edits.push((e, e, format!(";{dims}"))),
                None => head.push_str(&format!(" style=\"{dims}\"")),
            }
        }
        edits.sort_by(|a, b| b.0.cmp(&a.0));
        let mut t = tag.to_string();
        for (s, e, r) in edits {
            t.replace_range(s..e, &r);
        }
        t.insert_str(4, &head);
        out.push_str(&t);
    }
    out.push_str(&html[i..]);
    out
}

/// The colour an email paints behind everything (body or outermost table), if it sets one early.
/// Used to give the reading-pane card the same colour, so the email doesn't sit in a second frame.
pub fn outer_background(html: &str) -> Option<String> {
    let head: String = html.chars().take(6000).collect();
    let lower = head.to_ascii_lowercase();
    let mut best: Option<(usize, String)> = None;
    for key in ["bgcolor", "background-color", "background"] {
        let mut from = 0;
        while let Some(pos) = lower[from..].find(key) {
            let at = from + pos;
            from = at + key.len();
            if best.as_ref().is_some_and(|(b, _)| *b < at) {
                break;
            }
            let rest = lower[from..].trim_start();
            let Some(rest) = rest.strip_prefix('=').or_else(|| rest.strip_prefix(':')) else { continue };
            let rest = rest.trim_start().trim_start_matches(['"', '\'']).trim_start();
            let value: String = if rest.starts_with("rgb") {
                rest.chars().take_while(|&c| c != ')').chain(std::iter::once(')')).collect()
            } else {
                rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '#').collect()
            };
            let ok = (value.starts_with('#') && matches!(value.len(), 4 | 7) && value[1..].chars().all(|c| c.is_ascii_hexdigit()))
                || (value.starts_with("rgb") && value.chars().all(|c| c.is_ascii_alphanumeric() || "(),. %".contains(c)))
                || (value.len() >= 3
                    && value.chars().all(|c| c.is_ascii_alphabetic())
                    && !["none", "transparent", "inherit", "initial", "url", "unset", "image", "repeat", "no"].contains(&value.as_str()));
            if ok {
                best = Some((at, value));
                break;
            }
        }
    }
    best.map(|(_, v)| v)
}

pub fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

fn linkify(line: &str) -> String {
    let mut out = String::new();
    let mut rest = line;
    loop {
        let next = ["https://", "http://", "mailto:"]
            .iter()
            .filter_map(|p| rest.find(p).map(|i| (i, *p)))
            .min_by_key(|(i, _)| *i);
        let Some((i, _)) = next else {
            out.push_str(&escape_html(rest));
            return out;
        };
        out.push_str(&escape_html(&rest[..i]));
        let url_part = &rest[i..];
        let mut end = url_part
            .find(|c: char| c.is_whitespace() || "<>\"'，。、；：）】」》".contains(c))
            .unwrap_or(url_part.len());
        while end > 0 && ".,;:!?)]".contains(&url_part[..end][end - 1..]) {
            // keep balanced parentheses, e.g. wikipedia links
            if url_part[..end].ends_with(')') && url_part[..end].matches('(').count() >= url_part[..end].matches(')').count() {
                break;
            }
            end -= 1;
        }
        let url = &url_part[..end];
        if url.len() <= 8 {
            out.push_str(&escape_html(url));
        } else {
            let e = escape_html(url);
            out.push_str(&format!("<a href=\"{e}\">{e}</a>"));
        }
        rest = &url_part[end..];
    }
}

/// Plain text to HTML: escapes, links URLs, nests quoted (`>`) lines and dims the signature.
pub fn text_to_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 64);
    let mut depth = 0usize;
    let mut in_sig = false;
    let lines: Vec<&str> = text.split('\n').collect();
    for (n, raw_line) in lines.iter().enumerate() {
        let line = raw_line.trim_end_matches('\r');
        let mut level = 0;
        let mut body = line;
        loop {
            let t = body.trim_start_matches(' ');
            if let Some(stripped) = t.strip_prefix('>') {
                level += 1;
                body = stripped.strip_prefix(' ').unwrap_or(stripped);
            } else {
                break;
            }
        }
        if level == 0 {
            body = line;
        }
        while depth < level {
            out.push_str("<blockquote>");
            depth += 1;
        }
        while depth > level {
            out.push_str("</blockquote>");
            depth -= 1;
        }
        if level == 0 && !in_sig && (line == "-- " || line == "--") {
            out.push_str("<div class=\"sig\">");
            in_sig = true;
        }
        out.push_str(&linkify(body));
        if n + 1 < lines.len() {
            out.push('\n');
        }
    }
    while depth > 0 {
        out.push_str("</blockquote>");
        depth -= 1;
    }
    if in_sig {
        out.push_str("</div>");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocked_remote_images_become_placeholders() {
        let html = concat!(
            "<p><img alt=\"Clutch\" src=\"https://cdn.example/logo.png\" width=\"160\" height=\"72\" style=\"display:block\"></p>",
            "<img src=\"https://t.example/open.gif\" width=\"1\" height=\"1\" alt=\"\"><IMG SRC='http://x/y.jpg' alt=Hero>",
            "<img src=\"data:image/png;base64,AAA\" width=\"10\" height=\"10\">",
            "<img srcset=\"https://x/a.png 2x\" width=40 height=40><imgx src=\"https://no\">",
        );
        let out = placeholder_remote_images(html);
        for gone in ["cdn.example", "t.example", "x/y.jpg", " srcset="] {
            assert!(!out.contains(gone), "{gone} still in {out}");
        }
        assert!(out.contains("<img data-lm-blocked=\"box\" alt=\"Clutch\" src=\"data:image/gif"), "{out}");
        assert!(out.contains("style=\"display:block;width:160px;height:72px\""), "{out}");
        assert_eq!(out.matches("data-lm-blocked=\"hide\"").count(), 2, "tracking pixel and unsized image are hidden: {out}");
        assert!(out.contains("<img src=\"data:image/png;base64,AAA\" width=\"10\" height=\"10\">"), "inline images untouched");
        assert!(out.contains("data-lm-srcset=") && out.contains("style=\"width:40px;height:40px\""), "{out}");
        assert!(out.ends_with("<imgx src=\"https://no\">"));
    }

    #[test]
    fn finds_the_outer_background() {
        let bg = |h: &str| outer_background(h);
        assert_eq!(bg("<body style=\"margin:0;background-color:#F4F4F4\"><table bgcolor=\"#ffffff\">").as_deref(), Some("#f4f4f4"));
        assert_eq!(bg("<table width=\"100%\" bgcolor=\"#f3efe8\"><tr><td style=\"background:#fff\">").as_deref(), Some("#f3efe8"));
        assert_eq!(bg("<div style=\"background: url(https://x/bg.png) no-repeat\"><td bgcolor=white>").as_deref(), Some("white"));
        assert_eq!(bg("<div style=\"background-color: rgb(250, 250, 250)\">").as_deref(), Some("rgb(250, 250, 250)"));
        assert_eq!(bg("<p>Hello</p>"), None);
        assert_eq!(bg("<td style=\"background:transparent\">"), None);
    }

    const GBK_MAIL: &[u8] = b"From: =?GBK?B?1cXI/Q==?= <zhangsan@example.cn>\r\n\
To: \"Li Si\" <lisi@example.com>, wangwu@example.com\r\n\
Subject: =?GBK?B?1ty74bzN0qo=?=\r\n\
Date: Sun, 18 Oct 2026 09:41:00 +0800\r\n\
Message-ID: <abc123@example.cn>\r\n\
MIME-Version: 1.0\r\n\
Content-Type: text/plain; charset=GBK\r\n\
Content-Transfer-Encoding: base64\r\n\
\r\n\
tPO80rrDo6zD98zsz8LO58j9tePKx9bcu+Ghow==\r\n";

    #[test]
    fn parses_gbk_headers_and_body() {
        let p = parse(GBK_MAIL);
        assert_eq!(p.subject, "周会纪要");
        assert_eq!(p.from.name, "张三");
        assert_eq!(p.from.address, "zhangsan@example.cn");
        assert_eq!(p.to.len(), 2);
        assert_eq!(p.to[0].name, "Li Si");
        assert_eq!(p.to[1].address, "wangwu@example.com");
        assert_eq!(p.message_id.as_deref(), Some("abc123@example.cn"));
        assert_eq!(p.date, Some(1792287660));
        assert!(p.snippet.starts_with("大家好，明天下午三点"), "{}", p.snippet);
        assert!(!p.has_attachments);
    }

    #[test]
    fn decodes_adjacent_encoded_words_and_qp() {
        let raw = b"Subject: =?UTF-8?Q?=E4=BD=A0=E5=A5=BD?= =?UTF-8?B?5LiW55WM?=\r\n\
From: a@b.c\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\n\
caf=C3=A9 =E2=80=94 long line that is soft=\r\n wrapped\r\n";
        let p = parse(raw);
        assert_eq!(p.subject, "你好世界");
        assert_eq!(p.snippet, "café — long line that is soft wrapped");
    }

    fn multipart() -> Vec<u8> {
        b"From: News <news@shop.example>\r\n\
To: me@example.com\r\n\
Subject: Weekly\r\n\
Message-ID: <m1@shop>\r\n\
In-Reply-To: <parent@shop>\r\n\
References: <root@shop> <parent@shop>\r\n\
List-Unsubscribe: <mailto:unsub@shop.example>, <https://shop.example/unsub?id=1>\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"outer\"\r\n\
\r\n\
--outer\r\n\
Content-Type: multipart/related; boundary=\"rel\"\r\n\
\r\n\
--rel\r\n\
Content-Type: multipart/alternative; boundary=\"alt\"\r\n\
\r\n\
--alt\r\n\
Content-Type: text/plain; charset=utf-8\r\n\
\r\n\
Plain version\r\n\
--alt\r\n\
Content-Type: text/html; charset=utf-8\r\n\
\r\n\
<html><body><p>Hello <b>there</b></p><img src=\"cid:logo@shop\"><img src=\"https://track.example/p.gif\"></body></html>\r\n\
--alt--\r\n\
--rel\r\n\
Content-Type: image/png\r\n\
Content-ID: <logo@shop>\r\n\
Content-Transfer-Encoding: base64\r\n\
\r\n\
iVBORw0KGgo=\r\n\
--rel--\r\n\
--outer\r\n\
Content-Type: application/pdf; name=\"=?UTF-8?B?5Y+R56WoLnBkZg==?=\"\r\n\
Content-Disposition: attachment; filename*=UTF-8''%E5%8F%91%E7%A5%A8.pdf\r\n\
Content-Transfer-Encoding: base64\r\n\
\r\n\
JVBERi0xLjQK\r\n\
--outer--\r\n"
            .to_vec()
    }

    #[test]
    fn multipart_structure() {
        let raw = multipart();
        let p = parse(&raw);
        assert_eq!(p.snippet, "Plain version");
        assert!(p.has_attachments);
        assert_eq!(p.in_reply_to.as_deref(), Some("parent@shop"));
        assert_eq!(p.references, vec!["root@shop", "parent@shop"]);
        assert_eq!(p.list_unsubscribe.as_deref(), Some("https://shop.example/unsub?id=1"));

        let atts = attachments(&raw);
        assert_eq!(atts.len(), 1, "inline cid image must not be listed: {atts:?}");
        assert_eq!(atts[0].filename, "发票.pdf");
        assert_eq!(atts[0].mime_type, "application/pdf");
        let (name, _, bytes) = attachment(&raw, atts[0].index).unwrap();
        assert_eq!(name, "发票.pdf");
        assert_eq!(bytes, b"%PDF-1.4\n");
    }

    #[test]
    fn renders_html_with_cid_inlined_and_remote_blocked() {
        let raw = multipart();
        let r = render(&raw, false);
        assert!(r.has_remote_content);
        assert!(r.html.contains("data:image/png;base64,iVBORw0KGgo="), "cid image inlined");
        assert!(!r.html.contains("cid:logo"));
        assert!(r.html.contains("img-src data:;"), "remote images blocked by CSP");
        assert!(!r.html.contains("script-src"));
        let r = render(&raw, true);
        assert!(r.html.contains("img-src data: https: http:;"));
    }

    #[test]
    fn plain_text_rendering() {
        let html = text_to_html("Hi <b>&\nsee https://example.com/a_(b), ok.\n> quoted\n> > deeper\nback\n-- \nsig line");
        assert!(html.contains("Hi &lt;b&gt;&amp;"));
        assert!(html.contains("<a href=\"https://example.com/a_(b)\">https://example.com/a_(b)</a>, ok."), "{html}");
        assert!(html.contains("<blockquote>quoted\n<blockquote>deeper\n</blockquote></blockquote>back"), "{html}");
        assert!(html.contains("<div class=\"sig\">-- \nsig line</div>"), "{html}");
    }

    #[test]
    fn remote_detection() {
        assert!(has_remote_resources("<img src=\"https://x/y.png\">"));
        assert!(has_remote_resources("<td background='http://x/bg.jpg'>"));
        assert!(has_remote_resources("<div style=\"background:url( 'https://x' )\">"));
        assert!(has_remote_resources("<link rel=stylesheet href=\"https://x/a.css\">"));
        assert!(!has_remote_resources("<a href=\"https://example.com\">link</a>"));
        assert!(!has_remote_resources("<img src=\"data:image/png;base64,AAA\">"));
        assert!(!has_remote_resources("<p>see https://example.com</p>"));
    }

    #[test]
    fn snippet_skips_quotes_and_reply_headers() {
        let s = snippet("Sounds good!\n\nOn Mon, Oct 13, 2026 at 9:00 Bob wrote:\n> earlier text");
        assert_eq!(s, "Sounds good!");
        let s = snippet("好的\n在 2026年10月13日 09:00，张三 写道：\n> 之前");
        assert_eq!(s, "好的");
        let long = "字".repeat(400);
        assert_eq!(snippet(&long).chars().count(), SNIPPET_CHARS + 1);
    }

    #[test]
    fn designed_detection() {
        assert!(is_designed("<table bgcolor=\"#fff\">"));
        assert!(is_designed("<p style=\"color: red\">"));
        assert!(!is_designed("<div>Hello<br>world</div>"));
    }

    #[test]
    fn survives_garbage() {
        let p = parse(b"\x00\x01\x02 not a mail");
        let _ = render(b"\xff\xfe", false);
        let _ = p.snippet;
    }
}
