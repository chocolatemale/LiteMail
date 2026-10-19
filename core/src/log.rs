//! Tiny append-only log file. Never logs passwords or message contents.

use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

static PATH: OnceLock<PathBuf> = OnceLock::new();
static LOCK: Mutex<()> = Mutex::new(());
const MAX_BYTES: u64 = 4 * 1024 * 1024;

pub fn init(path: PathBuf) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = PATH.set(path);
}

pub fn write(level: &str, msg: &str) {
    if std::env::var_os("LITEMAIL_LOG_STDERR").is_some() {
        eprintln!("[core] {level} {msg}");
    }
    let Some(path) = PATH.get() else { return };
    let _g = LOCK.lock();
    if std::fs::metadata(path).map(|m| m.len() > MAX_BYTES).unwrap_or(false) {
        let _ = std::fs::rename(path, path.with_extension("log.1"));
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        let _ = writeln!(f, "{ts} {level} {msg}");
    }
}

#[macro_export]
macro_rules! log_info {
    ($($t:tt)*) => { $crate::log::write("INFO", &format!($($t)*)) };
}

#[macro_export]
macro_rules! log_warn {
    ($($t:tt)*) => { $crate::log::write("WARN", &format!($($t)*)) };
}
