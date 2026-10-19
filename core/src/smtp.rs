//! Sending through SMTP (implicit TLS, STARTTLS, or plain for localhost).

use std::time::Duration;

use lettre::address::Envelope;
use lettre::transport::smtp::authentication::{Credentials, Mechanism};
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};

use crate::error::{CoreError, Result};
use crate::imap::is_local_host;
use crate::types::{Security, ServerConfig};

fn transport(cfg: &ServerConfig, allow_invalid: bool, user: &str, password: &str) -> Result<AsyncSmtpTransport<Tokio1Executor>> {
    let host = cfg.host.trim().to_string();
    let params = || {
        let lax = allow_invalid && is_local_host(&host);
        TlsParameters::builder(host.clone())
            .dangerous_accept_invalid_certs(lax)
            .dangerous_accept_invalid_hostnames(lax)
            .build()
            .map_err(|e| CoreError::network(t!("TLS setup failed: {e}", "TLS 设置失败：{e}")))
    };
    let tls = match cfg.security {
        Security::Tls => Tls::Wrapper(params()?),
        Security::StartTls => Tls::Required(params()?),
        Security::None => {
            if !is_local_host(&host) {
                return Err(CoreError::invalid(t!("Unencrypted connections are only allowed to this Mac", "不加密的连接只允许用于本机测试服务器")));
            }
            Tls::None
        }
    };
    Ok(AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(host)
        .port(cfg.port)
        .tls(tls)
        .credentials(Credentials::new(user.trim().to_string(), password.to_string()))
        .authentication(vec![Mechanism::Plain, Mechanism::Login])
        .timeout(Some(Duration::from_secs(60)))
        .build())
}

fn map_err(e: lettre::transport::smtp::Error) -> CoreError {
    if e.is_permanent() {
        let msg = e.to_string();
        if msg.contains("535") || msg.contains("534") || msg.to_lowercase().contains("auth") {
            return CoreError::Auth { message: t!("The outgoing server refused the sign-in: {msg}", "发信服务器拒绝登录：{msg}") };
        }
        return CoreError::server(t!("The outgoing server refused this message: {msg}", "发信服务器拒绝了这封邮件：{msg}"));
    }
    if e.is_timeout() {
        return CoreError::network(t!("The outgoing server timed out", "连接发信服务器超时"));
    }
    let msg = e.to_string();
    if msg.contains("535") || msg.to_lowercase().contains("authentication") {
        return CoreError::Auth { message: t!("The outgoing server refused the sign-in: {msg}", "发信服务器拒绝登录：{msg}") };
    }
    CoreError::network(t!("Sending failed: {msg}", "发信失败：{msg}"))
}

/// Connects and authenticates without sending anything.
pub async fn check(cfg: &ServerConfig, allow_invalid: bool, user: &str, password: &str) -> Result<()> {
    let t = transport(cfg, allow_invalid, user, password)?;
    match t.test_connection().await {
        Ok(true) => Ok(()),
        Ok(false) => Err(CoreError::network(t!("The outgoing server didn't respond", "发信服务器没有响应"))),
        Err(e) => Err(map_err(e)),
    }
}

pub async fn send(cfg: &ServerConfig, allow_invalid: bool, user: &str, password: &str, envelope: &Envelope, raw: &[u8]) -> Result<()> {
    let t = transport(cfg, allow_invalid, user, password)?;
    t.send_raw(envelope, raw).await.map_err(map_err)?;
    Ok(())
}
