//! English (default) and Simplified Chinese strings. The app sets the language once at
//! launch from the user's preferred localization; tests can override it per thread.

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering};

static ZH: AtomicBool = AtomicBool::new(false);
static HOUR24: AtomicBool = AtomicBool::new(false);

thread_local! {
    static OVERRIDE: Cell<Option<bool>> = const { Cell::new(None) };
}

/// Accepts a BCP 47 tag such as "en", "zh-Hans", "zh-Hant-TW".
pub fn set_language(tag: &str) {
    ZH.store(tag.trim().to_ascii_lowercase().starts_with("zh"), Ordering::Relaxed);
}

pub fn set_hour24(on: bool) {
    HOUR24.store(on, Ordering::Relaxed);
}

/// Whether English times use a 24-hour clock (follows the macOS setting).
pub fn hour24() -> bool {
    HOUR24.load(Ordering::Relaxed)
}

pub fn zh() -> bool {
    OVERRIDE.with(|o| o.get()).unwrap_or_else(|| ZH.load(Ordering::Relaxed))
}

/// Runs `f` with the given language on this thread only.
pub fn with_language<R>(zh: bool, f: impl FnOnce() -> R) -> R {
    let prev = OVERRIDE.with(|o| o.replace(Some(zh)));
    let r = f();
    OVERRIDE.with(|o| o.set(prev));
    r
}

/// `t!("English {x}", "中文 {x}")` → String in the current language.
#[macro_export]
macro_rules! t {
    ($en:literal, $zh:literal) => {
        if $crate::i18n::zh() { format!($zh) } else { format!($en) }
    };
    ($en:literal, $zh:literal, $($arg:tt)*) => {
        if $crate::i18n::zh() { format!($zh, $($arg)*) } else { format!($en, $($arg)*) }
    };
}

#[cfg(test)]
mod tests {
    #[test]
    fn switches_per_thread() {
        assert_eq!(super::with_language(true, || t!("Inbox", "收件箱")), "收件箱");
        assert_eq!(super::with_language(false, || t!("Inbox", "收件箱")), "Inbox");
        let n = 3;
        assert_eq!(super::with_language(true, || t!("{n} new", "{n} 封新邮件")), "3 封新邮件");
        assert_eq!(super::with_language(false, || t!("{} new", "{} 封新邮件", n)), "3 new");
    }
}
