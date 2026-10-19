//! The reading pane document: a native-looking header (account kicker, serif subject,
//! sender, recipients, attachments, remote-content notice) followed by the message body.
//!
//! JavaScript stays off. Interaction uses `litemail://` links that the app intercepts:
//!   litemail://attachment/<index>   open (⌥-click: save as)
//!   litemail://load-remote          allow remote images for this message
//!   litemail://unsubscribe          follow List-Unsubscribe

use chrono::{Datelike, Local, TimeZone};

use crate::mime::{self, Parsed, escape_html};
use crate::types::{AttachmentInfo, EmailAddress};

pub struct HeaderInfo<'a> {
    /// e.g. "iCloud · 收件箱"
    pub kicker: String,
    pub account_email: &'a str,
    pub date: i64,
    pub allow_remote: bool,
}

/// Stable hue (0..360) for avatars and account dots. The Swift UI uses the same function.
pub fn hue(key: &str) -> u32 {
    let mut h: u32 = 0x811c9dc5;
    for b in key.trim().to_lowercase().bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x01000193);
    }
    h % 360
}

pub fn initials(name: &str, address: &str) -> String {
    let name = name.trim();
    if let Some(c) = name.chars().next() {
        if !c.is_ascii() {
            return c.to_string();
        }
        let letters: String = name
            .split_whitespace()
            .filter_map(|w| w.chars().find(|c| c.is_alphanumeric()))
            .take(2)
            .collect::<String>()
            .to_uppercase();
        if !letters.is_empty() {
            return letters;
        }
    }
    address.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "?".into())
}

const WEEKDAYS: [&str; 7] = ["周一", "周二", "周三", "周四", "周五", "周六", "周日"];

pub fn long_date(ts: i64) -> String {
    let Some(d) = Local.timestamp_opt(ts, 0).single() else { return String::new() };
    let now = Local::now();
    let days = (now.date_naive() - d.date_naive()).num_days();
    if crate::i18n::zh() {
        let time = d.format("%H:%M");
        return match days {
            0 => format!("今天 {time}"),
            1 => format!("昨天 {time}"),
            _ if d.year() == now.year() => {
                format!("{}月{}日 {} {time}", d.month(), d.day(), WEEKDAYS[d.weekday().num_days_from_monday() as usize])
            }
            _ => format!("{}年{}月{}日 {time}", d.year(), d.month(), d.day()),
        };
    }
    let time = d.format(if crate::i18n::hour24() { "%H:%M" } else { "%-I:%M %p" });
    match days {
        0 => format!("Today {time}"),
        1 => format!("Yesterday {time}"),
        _ if d.year() == now.year() => format!("{} at {time}", d.format("%a, %b %-d")),
        _ => format!("{} at {time}", d.format("%b %-d, %Y")),
    }
}

pub fn human_size(bytes: u64) -> String {
    match bytes {
        b if b >= 1024 * 1024 * 1024 => format!("{:.1} GB", b as f64 / 1073741824.0),
        b if b >= 1024 * 1024 => format!("{:.1} MB", b as f64 / 1048576.0),
        b if b >= 1024 => format!("{} KB", (b as f64 / 1024.0).round() as u64),
        b => format!("{b} B"),
    }
}

/// Short file-type badge and its hue.
fn badge(a: &AttachmentInfo) -> (&'static str, u32) {
    let ext = a.filename.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    let m = a.mime_type.as_str();
    match ext.as_str() {
        "pdf" => ("PDF", 25),
        "doc" | "docx" | "pages" | "rtf" => ("DOC", 252),
        "xls" | "xlsx" | "numbers" | "csv" => ("XLS", 150),
        "ppt" | "pptx" | "key" => ("PPT", 50),
        "zip" | "rar" | "7z" | "gz" | "tar" => ("ZIP", 290),
        "eml" => ("EML", 200),
        "ics" => ("ICS", 340),
        "txt" | "md" => ("TXT", 220),
        _ if m.starts_with("image/") => ("IMG", 150),
        _ if m.starts_with("video/") => ("VID", 320),
        _ if m.starts_with("audio/") => ("AUD", 180),
        _ => ("FILE", 260),
    }
}

fn person(a: &EmailAddress, me: &str) -> String {
    if !me.is_empty() && a.address.eq_ignore_ascii_case(me) {
        t!("me", "我")
    } else if !a.name.is_empty() {
        a.name.clone()
    } else {
        a.address.split('@').next().unwrap_or(&a.address).to_string()
    }
}

fn full_list(v: &[EmailAddress]) -> String {
    v.iter()
        .map(|a| {
            if a.name.is_empty() {
                escape_html(&a.address)
            } else {
                format!("{} <span>&lt;{}&gt;</span>", escape_html(&a.name), escape_html(&a.address))
            }
        })
        .collect::<Vec<_>>()
        .join(if crate::i18n::zh() { "、" } else { ", " })
}

fn recipients_summary(p: &Parsed, me: &str) -> String {
    let all: Vec<String> = p.to.iter().chain(p.cc.iter()).map(|a| person(a, me)).collect();
    if all.is_empty() {
        return t!("No recipients", "无收件人");
    }
    if all.len() <= 3 {
        t!("to {}", "发给 {}", all.join(if crate::i18n::zh() { "、" } else { ", " }))
    } else {
        t!("to {} and {} others", "发给 {} 等 {} 人", all[..2].join(if crate::i18n::zh() { "、" } else { ", " }), if crate::i18n::zh() { all.len() } else { all.len() - 2 })
    }
}

const READER_CSS: &str = r#"
@font-face { font-family: "LM Serif"; font-style: normal; font-weight: 200 900;
  src: url("litemail-asset://fonts/SourceSerif4.ttf") format("truetype"); }
@font-face { font-family: "LM Serif"; font-style: italic; font-weight: 200 900;
  src: url("litemail-asset://fonts/SourceSerif4-Italic.ttf") format("truetype"); }
:root { color-scheme: light dark; --lm-bg: #fffefb; --lm-ink: #141413; --lm-ink2: #5e5d59; --lm-ink3: #9c9a92;
  --lm-line: #e8e6dc; --lm-line2: #dad7cc; --lm-field: #f5f4ed; --lm-accent: #b85a3c;
  --lm-serif: "LM Serif", ui-serif, "New York", Georgia, "Songti SC", serif;
  --lm-sans: -apple-system, BlinkMacSystemFont, "PingFang SC", "Hiragino Sans GB", sans-serif; }
@media (prefers-color-scheme: dark) { :root { --lm-bg: #2b2a27; --lm-ink: #f5f4ef; --lm-ink2: #b7b5a9; --lm-ink3: #85837a;
  --lm-line: rgba(255,255,255,.07); --lm-line2: rgba(255,255,255,.13); --lm-field: #34332f; --lm-accent: #e8957a; } }
html, body { margin: 0 !important; padding: 0 !important; background: var(--lm-bg) !important; }
body { font: 15px/1.7 var(--lm-sans); color: var(--lm-ink); -webkit-font-smoothing: antialiased; overflow-wrap: break-word; -webkit-text-size-adjust: 100%; }
#lm-h { display: block !important; padding: 24px 52px 0 !important; margin: 0 !important; background: var(--lm-bg) !important;
  color: var(--lm-ink) !important; font: 13px/1.45 -apple-system, BlinkMacSystemFont, "PingFang SC", sans-serif !important;
  text-align: left !important; -webkit-user-select: text; }
#lm-h * { font-family: inherit; box-sizing: border-box; letter-spacing: normal; text-transform: none; }
#lm-h .k { display: flex !important; align-items: center; gap: 6px; font-size: 11px !important; font-weight: 600 !important;
  letter-spacing: .05em !important; color: var(--lm-ink3) !important; margin: 0 !important; }
#lm-h .k i { width: 7px; height: 7px; border-radius: 50%; display: inline-block; background: var(--dot); }
#lm-h h1 { font: 500 29px/1.2 var(--lm-serif) !important; letter-spacing: -.018em !important;
  margin: 8px 0 18px !important; padding: 0 !important; color: var(--lm-ink) !important; text-wrap: pretty; }
#lm-h h1.none { color: var(--lm-ink3) !important; font-style: italic !important; }
#lm-h .f { display: grid !important; grid-template-columns: 40px minmax(0, 1fr) auto; gap: 0 12px; align-items: start;
  padding: 0 0 16px !important; border-bottom: .5px solid var(--lm-line) !important; margin: 0 !important; }
#lm-h .av { width: 40px; height: 40px; border-radius: 50%; display: flex !important; align-items: center; justify-content: center;
  font-size: 14px !important; font-weight: 600 !important; color: #3d2a1f !important; background: var(--l); margin-top: 1px; }
#lm-h .who { min-width: 0; padding-top: 1px; }
#lm-h .who b { font-weight: 600 !important; font-size: 14px !important; color: var(--lm-ink) !important; }
#lm-h .who b span { font-weight: 400 !important; color: var(--lm-ink2) !important; margin-left: 4px; }
#lm-h details { font-size: 12.5px !important; color: var(--lm-ink2) !important; margin: 2px 0 0 !important; }
#lm-h summary { list-style: none; cursor: default; display: inline-flex; align-items: center; gap: 4px; outline: none; }
#lm-h summary::-webkit-details-marker { display: none; }
#lm-h summary::after { content: "›"; display: inline-block; color: var(--lm-ink3); transition: transform .15s ease; }
#lm-h details[open] summary::after { transform: rotate(90deg); }
#lm-h .full { margin-top: 6px; line-height: 1.75; display: grid; grid-template-columns: auto 1fr; gap: 0 10px; }
#lm-h .full em { font-style: normal; color: var(--lm-ink3); }
#lm-h .full span span, #lm-h .full span { color: var(--lm-ink2); }
#lm-h .t { font-size: 12.5px !important; color: var(--lm-ink2) !important; text-align: right; white-space: nowrap; padding-top: 2px; }
#lm-h .t a { display: block; margin-top: 2px; }
#lm-h a { color: var(--lm-accent) !important; text-decoration: none !important; cursor: default; }
#lm-h a:hover { text-decoration: underline !important; }
#lm-h .sh { display: flex !important; align-items: center; gap: 8px; margin: 16px 0 0 !important; padding: 9px 12px !important;
  border-radius: 10px; background: var(--lm-field) !important; font-size: 12.5px !important; color: var(--lm-ink2) !important; }
#lm-h .sh svg { flex: none; }
#lm-h .sh a { margin-left: auto; font-weight: 600 !important; }
#lm-h .ch { display: flex !important; flex-wrap: wrap; gap: 10px; margin: 16px 0 0 !important; padding: 0 !important; }
#lm-h a.c { display: flex !important; align-items: center; gap: 10px; height: 50px; padding: 0 14px 0 9px !important;
  border-radius: 12px; background: var(--lm-field) !important; box-shadow: inset 0 0 0 .5px var(--lm-line2);
  color: var(--lm-ink) !important; max-width: 300px; text-decoration: none !important; }
#lm-h a.c:hover { box-shadow: inset 0 0 0 1.5px var(--lm-accent); text-decoration: none !important; }
#lm-h a.c .b { flex: none; width: 32px; height: 32px; border-radius: 8px; display: flex !important; align-items: center;
  justify-content: center; font: 700 9px/1 ui-monospace, "SF Mono", Menlo, monospace !important; letter-spacing: .03em !important;
  color: var(--bi); background: var(--bb); }
#lm-h a.c .n { min-width: 0; display: block; }
#lm-h a.c .n b { display: block; font-weight: 500 !important; font-size: 13px !important; white-space: nowrap; overflow: hidden;
  text-overflow: ellipsis; color: var(--lm-ink) !important; }
#lm-h a.c .n small { display: block; font-size: 11.5px !important; color: var(--lm-ink2) !important; }
@media (prefers-color-scheme: dark) {
  #lm-h .av { color: #f1f1f5 !important; background: var(--d); }
  #lm-h a.c .b { color: var(--bid); background: var(--bbd); }
}
#lm-m { display: block; padding: 24px 52px 60px; }
#lm-m.plain, #lm-m.simple { color: var(--lm-ink); font: 15.5px/1.72 var(--lm-serif); }
#lm-m.plain a, #lm-m.simple a { color: var(--lm-accent); text-decoration: none; }
#lm-m.plain a:hover, #lm-m.simple a:hover { text-decoration: underline; }
#lm-m .text { white-space: pre-wrap; max-width: 680px; }
#lm-m .text blockquote { margin: 4px 0; padding: 0 0 0 12px; border-left: 2px solid var(--lm-line2); color: var(--lm-ink2); white-space: pre-wrap; }
#lm-m .text .sig { color: var(--lm-ink3); }
#lm-m img { max-width: 100%; height: auto; }
#lm-m table { max-width: 100%; }
#lm-m pre, #lm-m code { white-space: pre-wrap; font-family: ui-monospace, "SF Mono", Menlo, monospace; font-size: 12.5px; }
#lm-m.designed { color: #000; }
#lm-m.designed > .card { background: var(--card, #ffffff); color: #000; overflow-x: auto; border-radius: 12px; padding: 16px 0;
  box-shadow: 0 0 0 .5px rgba(0,0,0,.06); }
@media (prefers-color-scheme: dark) {
  #lm-m.designed > .card { box-shadow: 0 0 0 .5px rgba(255,255,255,.1); }
}
#lm-m img[data-lm-blocked="hide"] { display: none !important; }
#lm-m img[data-lm-blocked="box"] { background: rgba(120,120,120,.12); border-radius: 6px; }
"#;

const SHIELD_SVG: &str = "<svg width=\"16\" height=\"16\" viewBox=\"0 0 16 16\" aria-hidden=\"true\"><path d=\"M8 1.5 2.5 3.6v4.1c0 3.3 2.3 5.9 5.5 6.8 3.2-.9 5.5-3.5 5.5-6.8V3.6L8 1.5Z\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.3\" stroke-linejoin=\"round\"/></svg>";

pub fn document(raw: &[u8], p: &Parsed, attachments: &[AttachmentInfo], info: &HeaderInfo<'_>) -> (String, bool) {
    let content = mime::content(raw);
    let me = info.account_email;
    let mut h = String::with_capacity(4096);

    // kicker + subject
    h.push_str(&format!(
        "<div class=\"k\"><i style=\"--dot: oklch(0.62 0.16 {})\"></i>{}</div>",
        hue(me),
        escape_html(&info.kicker.to_uppercase())
    ));
    if p.subject.trim().is_empty() {
        h.push_str(&format!("<h1 class=\"none\">{}</h1>", t!("(No subject)", "（无主题）")));
    } else {
        h.push_str(&format!("<h1>{}</h1>", escape_html(&p.subject)));
    }

    // sender block
    let fh = hue(&p.from.address);
    let from_name = if p.from.name.is_empty() { p.from.address.clone() } else { p.from.name.clone() };
    let addr = if p.from.name.is_empty() { String::new() } else { format!("<span>{}</span>", escape_html(&p.from.address)) };
    let mut full = String::new();
    let mut row = |label: &str, v: &[EmailAddress]| {
        if !v.is_empty() {
            full.push_str(&format!("<em>{label}</em><span>{}</span>", full_list(v)));
        }
    };
    row(&t!("To", "收件人"), &p.to);
    row(&t!("Cc", "抄送"), &p.cc);
    if !p.reply_to.is_empty() && p.reply_to.first().map(|a| &a.address) != Some(&p.from.address) {
        row(&t!("Reply-To", "回复至"), &p.reply_to);
    }
    let unsubscribe = if p.list_unsubscribe.is_some() {
        format!("<a href=\"litemail://unsubscribe\">{}</a>", t!("Unsubscribe", "退订"))
    } else {
        String::new()
    };
    h.push_str(&format!(
        "<div class=\"f\"><div class=\"av\" style=\"--l: oklch(0.9 0.045 {fh}); --d: oklch(0.42 0.06 {fh})\">{}</div>\
         <div class=\"who\"><b>{}{}</b><details><summary>{}</summary><div class=\"full\">{}</div></details></div>\
         <div class=\"t\">{}{}</div></div>",
        escape_html(&initials(&p.from.name, &p.from.address)),
        escape_html(&from_name),
        addr,
        escape_html(&recipients_summary(p, me)),
        full,
        escape_html(&long_date(info.date)),
        unsubscribe
    ));

    if content.has_remote && !info.allow_remote {
        h.push_str(&format!(
            "<div class=\"sh\">{SHIELD_SVG}{}<a href=\"litemail://load-remote\">{}</a></div>",
            t!("Remote images blocked — the sender can’t tell you opened this.", "已拦截远程图片，发件人不会知道你打开了这封邮件。"),
            t!("Load images", "加载图片")
        ));
    }

    if !attachments.is_empty() {
        h.push_str("<div class=\"ch\">");
        for a in attachments {
            let (label, bh) = badge(a);
            h.push_str(&format!(
                "<a class=\"c\" href=\"litemail://attachment/{}\" title=\"{}\" \
                 style=\"--bi: oklch(0.52 0.15 {bh}); --bb: oklch(0.62 0.15 {bh} / .14); --bid: oklch(0.8 0.12 {bh}); --bbd: oklch(0.62 0.15 {bh} / .22)\">\
                 <span class=\"b\">{label}</span><span class=\"n\"><b>{}</b><small>{}</small></span></a>",
                a.index,
                t!("Click to open · Option-click to save", "点按打开 · 按住 ⌥ 点按可另存为"),
                escape_html(&a.filename),
                human_size(a.size)
            ));
        }
        h.push_str("</div>");
    }

    let html_body = if content.has_remote && !info.allow_remote {
        mime::placeholder_remote_images(&content.html)
    } else {
        content.html
    };
    let body = if content.class == "designed" {
        // The card takes the email's own outer colour, so it reads as one sheet, not a frame in a frame.
        let bg = mime::outer_background(&html_body).map(|c| format!(" style=\"--card: {c}\"")).unwrap_or_default();
        format!("<div class=\"card\"{bg}>{html_body}</div>")
    } else {
        html_body
    };
    let html = format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\">\
         <meta http-equiv=\"Content-Security-Policy\" content=\"{}\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <meta name=\"referrer\" content=\"no-referrer\">\
         <style>{READER_CSS}</style></head><body><header id=\"lm-h\">{h}</header>\
         <main id=\"lm-m\" class=\"{}\">{body}</main></body></html>",
        mime::csp(info.allow_remote),
        content.class
    );
    (html, content.has_remote)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(allow_remote: bool) -> HeaderInfo<'static> {
        HeaderInfo { kicker: "iCloud · 收件箱".into(), account_email: "me@icloud.com", date: 1792287660, allow_remote }
    }

    #[test]
    fn hue_is_stable() {
        // Swift's Avatar.hue(for:) must return the same values.
        assert_eq!(hue("zhang@corp.example"), hue(" ZHANG@corp.example "));
        assert_eq!(hue("zhang@corp.example"), 339);
        assert_eq!(hue("me@icloud.com"), 98);
        assert_eq!(hue(""), 2166136261 % 360);
    }

    #[test]
    fn initials_for_names() {
        assert_eq!(initials("张三", "z@x"), "张");
        assert_eq!(initials("Tim Cook", "t@x"), "TC");
        assert_eq!(initials("", "bob@x"), "B");
        assert_eq!(initials("\"  \"", ""), "?");
    }

    #[test]
    fn sizes() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(820 * 1024), "820 KB");
        assert_eq!(human_size(1_258_291), "1.2 MB");
    }

    #[test]
    fn header_is_escaped_and_complete() {
        let raw = b"From: \"<b>Evil</b>\" <evil@x.com>\r\nTo: me@icloud.com, A <a@x.com>, B <b@x.com>, C <c@x.com>\r\n\
Subject: <script>alert(1)</script> hi\r\nList-Unsubscribe: <https://x.com/u>\r\nContent-Type: text/html\r\n\r\n\
<p style=\"color:#333\">Body</p><img src=\"https://t.example/p.gif\">";
        let p = mime::parse(raw);
        let atts = vec![AttachmentInfo { index: 0, filename: "a<b>.pdf".into(), mime_type: "application/pdf".into(), size: 2048 }];
        let (html, remote) = document(raw, &p, &atts, &info(false));
        assert!(remote);
        assert!(!html.contains("<script>"), "subject escaped");
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt; hi"));
        assert!(html.contains("&lt;b&gt;Evil&lt;/b&gt;"));
        assert!(html.contains("ICLOUD · 收件箱"), "kicker is passed in");
        assert!(html.contains("to me, A and 2 others"), "{html}");
        let (zh, _) = crate::i18n::with_language(true, || document(raw, &p, &atts, &info(false)));
        assert!(zh.contains("发给 我、A 等 4 人"), "{zh}");
        assert!(zh.contains("加载图片"));
        assert!(html.contains("href=\"litemail://attachment/0\""));
        assert!(html.contains("a&lt;b&gt;.pdf"));
        assert!(html.contains(">PDF<"));
        assert!(html.contains("litemail://load-remote"));
        assert!(html.contains("litemail://unsubscribe"));
        assert!(html.contains("class=\"designed\""));
        assert!(html.contains("<div class=\"card\">"));
        assert!(html.contains("img-src data:;"));
        let (html, _) = document(raw, &p, &atts, &info(true));
        assert!(!html.contains("litemail://load-remote"), "no banner once allowed");
        assert!(html.contains("img-src data: https: http:;"));
    }

    #[test]
    fn plain_text_message_has_no_banner_or_card() {
        let raw = b"From: a@x.com\r\nTo: me@icloud.com\r\nSubject: \r\n\r\nhello https://x.com\r\n";
        let p = mime::parse(raw);
        let (html, remote) = document(raw, &p, &[], &info(false));
        assert!(!remote);
        assert!(html.contains("(No subject)"));
        assert!(html.contains("to me"));
        assert!(!html.contains("class=\"sh\""));
        assert!(!html.contains("class=\"ch\""));
        assert!(html.contains("<main id=\"lm-m\" class=\"plain\">"));
    }
}
