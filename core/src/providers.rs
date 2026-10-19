//! Server presets for the providers people actually use.

use crate::types::{ProviderKind, ProviderPreset, Security, ServerConfig};

fn server(host: &str, port: u16, security: Security) -> ServerConfig {
    ServerConfig { host: host.into(), port, security }
}

pub fn icloud() -> ProviderPreset {
    ProviderPreset {
        kind: ProviderKind::ICloud,
        name: "iCloud".into(),
        imap: server("imap.mail.me.com", 993, Security::Tls),
        smtp: server("smtp.mail.me.com", 587, Security::StartTls),
        server_saves_sent: false,
        password_help_url: "https://account.apple.com/account/manage".into(),
        password_help_text: t!("iCloud Mail needs an app-specific password: sign in at account.apple.com → Sign-In and Security → App-Specific Passwords → Generate, then paste the 16-character password here.", "iCloud 邮件需要「App 专用密码」：登录 account.apple.com → 登录与安全 → App 专用密码 → 生成，然后把 16 位密码粘贴到这里。"),
        domains: vec!["icloud.com".into(), "me.com".into(), "mac.com".into()],
    }
}

pub fn gmail() -> ProviderPreset {
    ProviderPreset {
        kind: ProviderKind::Gmail,
        name: "Gmail".into(),
        imap: server("imap.gmail.com", 993, Security::Tls),
        smtp: server("smtp.gmail.com", 465, Security::Tls),
        server_saves_sent: true,
        password_help_url: "https://myaccount.google.com/apppasswords".into(),
        password_help_text: t!("Gmail needs 2-Step Verification turned on. Then create an app password at myaccount.google.com/apppasswords and paste the 16-character password here.", "Gmail 需要先开启两步验证，再到 myaccount.google.com/apppasswords 生成「应用专用密码」，把 16 位密码粘贴到这里。"),
        domains: vec!["gmail.com".into(), "googlemail.com".into()],
    }
}

fn other(name: &str, imap: ServerConfig, smtp: ServerConfig, saves_sent: bool, help: String, domains: &[&str]) -> ProviderPreset {
    ProviderPreset {
        kind: ProviderKind::Other,
        name: name.into(),
        imap,
        smtp,
        server_saves_sent: saves_sent,
        password_help_url: String::new(),
        password_help_text: help,
        domains: domains.iter().map(|d| d.to_string()).collect(),
    }
}

/// Well-known servers for "Other" accounts, matched by domain.
fn known() -> Vec<ProviderPreset> {
    vec![
        other(
            &t!("QQ Mail", "QQ 邮箱"),
            server("imap.qq.com", 993, Security::Tls),
            server("smtp.qq.com", 465, Security::Tls),
            true,
            t!("Turn on IMAP/SMTP in QQ Mail's web settings (Settings → Account) and use the authorization code it gives you as the password.", "QQ 邮箱需要在网页版「设置 → 账户」开启 IMAP/SMTP 服务，并使用生成的授权码作为密码。"),
            &["qq.com", "foxmail.com", "vip.qq.com"],
        ),
        other(
            &t!("NetEase 163", "网易 163"),
            server("imap.163.com", 993, Security::Tls),
            server("smtp.163.com", 465, Security::Tls),
            true,
            t!("Turn on IMAP in NetEase Mail's web settings (Settings → POP3/SMTP/IMAP) and use the client authorization code as the password.", "网易邮箱需要在网页版「设置 → POP3/SMTP/IMAP」开启 IMAP，并使用客户端授权码作为密码。"),
            &["163.com"],
        ),
        other(
            &t!("NetEase 126", "网易 126"),
            server("imap.126.com", 993, Security::Tls),
            server("smtp.126.com", 465, Security::Tls),
            true,
            t!("Turn on IMAP in NetEase Mail's web settings (Settings → POP3/SMTP/IMAP) and use the client authorization code as the password.", "网易邮箱需要在网页版「设置 → POP3/SMTP/IMAP」开启 IMAP，并使用客户端授权码作为密码。"),
            &["126.com"],
        ),
        other(
            "Yahoo",
            server("imap.mail.yahoo.com", 993, Security::Tls),
            server("smtp.mail.yahoo.com", 465, Security::Tls),
            true,
            t!("Yahoo needs an app password from your account security settings.", "Yahoo 需要在账户安全设置里生成「应用密码」。"),
            &["yahoo.com", "ymail.com"],
        ),
        other(
            "Fastmail",
            server("imap.fastmail.com", 993, Security::Tls),
            server("smtp.fastmail.com", 465, Security::Tls),
            true,
            t!("Create an app password in Fastmail under Settings → Privacy & Security → App passwords.", "Fastmail 需要在「设置 → 隐私与安全 → 应用密码」生成专用密码。"),
            &["fastmail.com", "fastmail.fm"],
        ),
        other(
            "Zoho",
            server("imap.zoho.com", 993, Security::Tls),
            server("smtp.zoho.com", 465, Security::Tls),
            true,
            t!("With two-factor authentication on, Zoho needs an application-specific password.", "Zoho 开启两步验证后需要使用应用专用密码。"),
            &["zoho.com", "zohomail.com"],
        ),
    ]
}

pub fn presets() -> Vec<ProviderPreset> {
    vec![icloud(), gmail()]
}

/// Suggests settings for an address: exact provider match first, otherwise the
/// conventional imap./smtp. hostnames for the domain.
pub fn suggest(email: &str) -> ProviderPreset {
    let domain = email.rsplit('@').next().unwrap_or("").trim().to_ascii_lowercase();
    for p in presets().into_iter().chain(known()) {
        if p.domains.iter().any(|d| *d == domain) {
            return p;
        }
    }
    other(
        &domain,
        server(&format!("imap.{domain}"), 993, Security::Tls),
        server(&format!("smtp.{domain}"), 465, Security::Tls),
        false,
        t!("Use the IMAP/SMTP settings from your provider. Many need an app password or authorization code.", "使用邮箱服务商提供的 IMAP/SMTP 设置。很多服务商需要专用密码或授权码。"),
        &[],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggests_by_domain() {
        assert_eq!(suggest("someone@me.com").kind, ProviderKind::ICloud);
        assert_eq!(suggest("someone@Gmail.com").kind, ProviderKind::Gmail);
        assert_eq!(suggest("a@qq.com").imap.host, "imap.qq.com");
        let s = suggest("me@example.org");
        assert_eq!(s.imap.host, "imap.example.org");
        assert_eq!(s.smtp.port, 465);
    }
}
