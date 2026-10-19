#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum CoreError {
    #[error("{message}")]
    Network { message: String },
    #[error("{message}")]
    Auth { message: String },
    #[error("{message}")]
    Server { message: String },
    #[error("{message}")]
    InvalidInput { message: String },
    #[error("{message}")]
    Storage { message: String },
    #[error("{}", t!("Message or account not found", "找不到这封邮件或账户"))]
    NotFound,
}

pub type Result<T> = std::result::Result<T, CoreError>;

impl CoreError {
    pub fn invalid(message: impl Into<String>) -> Self {
        CoreError::InvalidInput { message: message.into() }
    }

    pub fn network(message: impl Into<String>) -> Self {
        CoreError::Network { message: message.into() }
    }

    pub fn server(message: impl Into<String>) -> Self {
        CoreError::Server { message: message.into() }
    }

    /// Errors after which the connection must be dropped and re-established.
    pub fn is_connection_error(&self) -> bool {
        matches!(self, CoreError::Network { .. })
    }
}

impl From<rusqlite::Error> for CoreError {
    fn from(e: rusqlite::Error) -> Self {
        match e {
            rusqlite::Error::QueryReturnedNoRows => CoreError::NotFound,
            e => CoreError::Storage { message: e.to_string() },
        }
    }
}

impl From<std::io::Error> for CoreError {
    fn from(e: std::io::Error) -> Self {
        CoreError::Storage { message: e.to_string() }
    }
}

impl From<async_imap::error::Error> for CoreError {
    fn from(e: async_imap::error::Error) -> Self {
        use async_imap::error::Error as E;
        match e {
            E::No(msg) => CoreError::Server { message: server_text(&msg) },
            E::Bad(msg) => CoreError::Server { message: t!("The server rejected the request: {}", "服务器拒绝了请求：{}", server_text(&msg)) },
            E::Io(io) => CoreError::Network { message: io.to_string() },
            // Only user-typed text (user name, password) can carry characters IMAP can't send.
            E::Validate(_) => CoreError::Auth {
                message: t!(
                    "The user name or password contains a character that can't be sent, such as a line break. Paste it again.",
                    "用户名或密码里有无法发送的字符（例如换行），请重新粘贴。"
                ),
            },
            E::ConnectionLost => CoreError::Network { message: t!("The connection to the server was lost", "与服务器的连接已断开") },
            other => CoreError::Network { message: other.to_string() },
        }
    }
}

impl From<tokio_native_tls::native_tls::Error> for CoreError {
    fn from(e: tokio_native_tls::native_tls::Error) -> Self {
        CoreError::Network { message: t!("TLS handshake failed: {e}", "TLS 握手失败：{e}") }
    }
}

impl From<tokio::time::error::Elapsed> for CoreError {
    fn from(_: tokio::time::error::Elapsed) -> Self {
        CoreError::Network { message: t!("The server took too long to respond", "连接服务器超时") }
    }
}

/// async-imap formats NO/BAD responses as `code: .., info: Some("text")`; keep the text.
pub fn server_text(raw: &str) -> String {
    if let Some(start) = raw.find("info: Some(\"") {
        let rest = &raw[start + 12..];
        let end = rest.rfind("\")").unwrap_or(rest.len());
        return rest[..end].replace("\\\"", "\"").trim().to_string();
    }
    raw.trim().to_string()
}

#[cfg(test)]
mod tests {
    #[test]
    fn extracts_server_text() {
        assert_eq!(
            super::server_text("code: None, info: Some(\"[UNAVAILABLE] Internal error occurred.\")"),
            "[UNAVAILABLE] Internal error occurred."
        );
        assert_eq!(super::server_text(" plain text "), "plain text");
    }
}
