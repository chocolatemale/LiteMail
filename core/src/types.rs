//! Records and enums shared with the Swift UI through UniFFI.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum Security {
    /// Implicit TLS (IMAPS 993, SMTPS 465).
    Tls,
    /// Plain connection upgraded with STARTTLS (IMAP 143, SMTP 587).
    StartTls,
    /// No encryption. Only accepted for localhost (test servers).
    None,
}

impl Security {
    pub fn as_str(&self) -> &'static str {
        match self {
            Security::Tls => "tls",
            Security::StartTls => "starttls",
            Security::None => "none",
        }
    }

    pub fn parse(s: &str) -> Security {
        match s {
            "starttls" => Security::StartTls,
            "none" => Security::None,
            _ => Security::Tls,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub security: Security,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ProviderKind {
    ICloud,
    Gmail,
    Other,
}

impl ProviderKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProviderKind::ICloud => "icloud",
            ProviderKind::Gmail => "gmail",
            ProviderKind::Other => "other",
        }
    }

    pub fn parse(s: &str) -> ProviderKind {
        match s {
            "icloud" => ProviderKind::ICloud,
            "gmail" => ProviderKind::Gmail,
            _ => ProviderKind::Other,
        }
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct ProviderPreset {
    pub kind: ProviderKind,
    pub name: String,
    pub imap: ServerConfig,
    pub smtp: ServerConfig,
    pub server_saves_sent: bool,
    /// Where the user creates an app-specific password.
    pub password_help_url: String,
    pub password_help_text: String,
    pub domains: Vec<String>,
}

/// Everything needed to add an account.
#[derive(Debug, Clone, uniffi::Record)]
pub struct AccountSetup {
    pub email: String,
    pub display_name: String,
    pub provider: ProviderKind,
    pub imap: ServerConfig,
    pub smtp: ServerConfig,
    pub username: String,
    pub password: String,
    /// Keep every message from the last N days on this Mac. 0 = keep everything.
    pub sync_days: u32,
    /// Accept self-signed certificates. Only honoured for localhost.
    pub allow_invalid_certs: bool,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct Account {
    pub id: i64,
    pub email: String,
    pub display_name: String,
    pub provider: ProviderKind,
    pub imap: ServerConfig,
    pub smtp: ServerConfig,
    pub username: String,
    pub sync_days: u32,
    pub sync_interval_minutes: u32,
    pub server_saves_sent: bool,
    pub signature: String,
    pub notify: bool,
    pub allow_invalid_certs: bool,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct AccountSettings {
    pub display_name: String,
    pub sync_days: u32,
    pub sync_interval_minutes: u32,
    pub signature: String,
    pub notify: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum FolderRole {
    Inbox,
    Sent,
    Drafts,
    Archive,
    Junk,
    Trash,
    /// Gmail "All Mail".
    All,
    /// Gmail "Starred" and similar virtual folders.
    Flagged,
    /// Gmail "Important".
    Important,
    Custom,
}

impl FolderRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            FolderRole::Inbox => "inbox",
            FolderRole::Sent => "sent",
            FolderRole::Drafts => "drafts",
            FolderRole::Archive => "archive",
            FolderRole::Junk => "junk",
            FolderRole::Trash => "trash",
            FolderRole::All => "all",
            FolderRole::Flagged => "flagged",
            FolderRole::Important => "important",
            FolderRole::Custom => "custom",
        }
    }

    pub fn parse(s: &str) -> FolderRole {
        match s {
            "inbox" => FolderRole::Inbox,
            "sent" => FolderRole::Sent,
            "drafts" => FolderRole::Drafts,
            "archive" => FolderRole::Archive,
            "junk" => FolderRole::Junk,
            "trash" => FolderRole::Trash,
            "all" => FolderRole::All,
            "flagged" => FolderRole::Flagged,
            "important" => FolderRole::Important,
            _ => FolderRole::Custom,
        }
    }

    /// Sort position in the sidebar.
    pub fn rank(&self) -> u8 {
        match self {
            FolderRole::Inbox => 0,
            FolderRole::Drafts => 1,
            FolderRole::Sent => 2,
            FolderRole::Archive => 3,
            FolderRole::Junk => 4,
            FolderRole::Trash => 5,
            FolderRole::Flagged => 6,
            FolderRole::Important => 7,
            FolderRole::All => 8,
            FolderRole::Custom => 9,
        }
    }

    /// Virtual folders that only mirror other folders (Gmail). Not synced by default
    /// so the same message isn't stored twice.
    pub fn is_virtual(&self) -> bool {
        matches!(self, FolderRole::All | FolderRole::Flagged | FolderRole::Important)
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct Folder {
    pub id: i64,
    pub account_id: i64,
    /// Raw (modified UTF-7) server name, used for IMAP commands.
    pub name: String,
    /// Decoded last path component.
    pub display_name: String,
    /// Decoded full path, e.g. "Projects/2026".
    pub path: String,
    pub depth: u32,
    pub role: FolderRole,
    pub sync_enabled: bool,
    pub unread: u32,
    pub total: u32,
    pub last_sync: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum Mailbox {
    Folder { id: i64 },
    /// Inboxes of every account.
    AllInboxes,
    /// Flagged messages across accounts.
    Flagged,
    /// Unread messages in every inbox.
    Unread,
    /// Every synced folder except Trash and Junk (used for search).
    Everything,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct MessageQuery {
    pub mailbox: Mailbox,
    /// Free text plus optional operators: from: to: subject: has:attachment is:unread is:flagged.
    pub search: Option<String>,
    pub limit: u32,
    /// Keyset pagination: return messages strictly older than (before_date, before_id).
    pub before_date: Option<i64>,
    pub before_id: Option<i64>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct MessageSummary {
    pub id: i64,
    pub account_id: i64,
    pub folder_id: i64,
    pub subject: String,
    pub from_name: String,
    pub from_addr: String,
    pub to_line: String,
    /// Received date, unix seconds.
    pub date: i64,
    pub snippet: String,
    pub seen: bool,
    pub flagged: bool,
    pub answered: bool,
    pub has_attachments: bool,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct EmailAddress {
    pub name: String,
    pub address: String,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct AttachmentInfo {
    pub index: u32,
    pub filename: String,
    pub mime_type: String,
    pub size: u64,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct MessageDetail {
    pub id: i64,
    pub account_id: i64,
    pub folder_id: i64,
    pub subject: String,
    pub from: EmailAddress,
    pub to: Vec<EmailAddress>,
    pub cc: Vec<EmailAddress>,
    pub reply_to: Vec<EmailAddress>,
    /// Date header (falls back to received date), unix seconds.
    pub date: i64,
    pub seen: bool,
    pub flagged: bool,
    pub attachments: Vec<AttachmentInfo>,
    pub has_remote_content: bool,
    /// Rendered document for the web view (CSP-locked, JavaScript-free).
    pub html: String,
    pub list_unsubscribe: Option<String>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct Draft {
    pub account_id: i64,
    pub to: String,
    pub cc: String,
    pub bcc: String,
    pub subject: String,
    pub body: String,
    /// Absolute file paths to attach.
    pub attachments: Vec<String>,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    /// Local id of the message being answered (gets the \Answered flag).
    pub reply_to_message: Option<i64>,
    /// Local id of a message whose attachments are forwarded.
    pub forward_from: Option<i64>,
    /// Local id of the server draft this continues; it is removed once sent or re-saved.
    pub draft_source: Option<i64>,
    /// Sender when it isn't the account's own address (an alias); None = the account address.
    pub from: Option<EmailAddress>,
}

/// An address an account can send from.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SendAddress {
    pub account_id: i64,
    pub name: String,
    pub address: String,
    pub kind: SendAddressKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum SendAddressKind {
    /// The address the account signs in with.
    Primary,
    /// Added by the user in Settings.
    Added,
    /// Found by LiteMail: sent from before (Sent folder), or the provider's guaranteed variant.
    Found,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum SyncPhase {
    Idle { last_sync: i64 },
    Connecting,
    /// Connected and looking for changes (every full or manual sync reports this, so a refresh
    /// always shows that something is happening).
    Checking,
    Syncing { folder: String, done: u32, total: u32 },
    Offline { message: String },
    AuthFailed { message: String },
}

#[derive(Debug, Clone, uniffi::Enum)]
pub enum CoreEvent {
    AccountsChanged,
    FoldersChanged { account_id: i64 },
    MessagesChanged { account_id: i64 },
    SyncState { account_id: i64, phase: SyncPhase },
    NewMail { account_id: i64, count: u32, message_id: i64, from: String, subject: String },
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct StorageInfo {
    pub messages: u64,
    pub bytes: u64,
}
