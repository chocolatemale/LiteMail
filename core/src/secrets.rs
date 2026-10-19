//! Password storage: the login keychain.
//!
//! An item belongs to the app that created it: the keychain hands it back to builds that match
//! LiteMail's designated requirement (bundle id + Developer ID team, see `build-app.sh`) without
//! asking, and asks the user for their Mac password before any other program can read it.
//! Never store passwords in a plain file — any program running as the user could read it.
//!
//! The prompts users saw on 2026-10-19 came from items written by ad-hoc builds (cdhash
//! requirement, a different "app" every build). Items written by the signed app don't prompt.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use crate::error::{CoreError, Result};
use crate::{log_info, log_warn};

pub trait SecretStore: Send + Sync {
    fn get(&self, account_id: i64) -> Result<String>;
    fn set(&self, account_id: i64, secret: &str) -> Result<()>;
    fn delete(&self, account_id: i64);
}

pub const SERVICE: &str = "uk.icoco.LiteMail";
const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;

/// Keychain-backed store with an in-memory cache so the keychain is read once per launch.
pub struct Keychain {
    /// Per data directory, see `Store::install_id`.
    prefix: String,
    /// Held while talking to the keychain, so concurrent readers (sync, IDLE, send) wait for
    /// one answer instead of each starting their own.
    cache: Mutex<HashMap<i64, String>>,
}

impl Keychain {
    pub fn new(prefix: String) -> Keychain {
        Keychain { prefix, cache: Mutex::new(HashMap::new()) }
    }

    fn key(&self, account_id: i64) -> String {
        format!("{}-password-{account_id}", self.prefix)
    }
}

impl SecretStore for Keychain {
    fn get(&self, account_id: i64) -> Result<String> {
        let mut cache = self.cache.lock().unwrap();
        if let Some(s) = cache.get(&account_id) {
            return Ok(s.clone());
        }
        let bytes = security_framework::passwords::get_generic_password(SERVICE, &self.key(account_id))
            .map_err(|e| read_error(e.code()))?;
        let s = String::from_utf8(bytes).map_err(|_| CoreError::Auth { message: t!("The password in the keychain is damaged", "钥匙串中的密码已损坏") })?;
        cache.insert(account_id, s.clone());
        Ok(s)
    }

    fn set(&self, account_id: i64, secret: &str) -> Result<()> {
        let mut cache = self.cache.lock().unwrap();
        // Replace rather than update, so the item's access list is the current app's.
        let _ = security_framework::passwords::delete_generic_password(SERVICE, &self.key(account_id));
        security_framework::passwords::set_generic_password(SERVICE, &self.key(account_id), secret.as_bytes())
            .map_err(|e| CoreError::Storage { message: t!("Couldn't write to the keychain: {e}", "无法写入钥匙串：{e}") })?;
        cache.insert(account_id, secret.to_string());
        Ok(())
    }

    fn delete(&self, account_id: i64) {
        let mut cache = self.cache.lock().unwrap();
        let _ = security_framework::passwords::delete_generic_password(SERVICE, &self.key(account_id));
        cache.remove(&account_id);
    }
}

/// "Not found" means the password really is missing; anything else (the user chose Deny, the
/// prompt was dismissed, the keychain is locked) means LiteMail wasn't allowed to read it.
fn read_error(status: i32) -> CoreError {
    if status == ERR_SEC_ITEM_NOT_FOUND {
        CoreError::Auth { message: t!("No password is saved for this account; please enter it again", "这个账户没有保存密码，请重新输入") }
    } else {
        CoreError::Auth {
            message: t!(
                "LiteMail couldn't read this account's password from the keychain (error {status}); please enter it again",
                "LiteMail 没能从钥匙串读取这个账户的密码（错误 {status}），请重新输入"
            ),
        }
    }
}

/// One early build (2026-10-19) kept passwords in `<data dir>/passwords`. Moves any found
/// there into the keychain, then overwrites and removes the file.
pub fn import_password_file(store: &dyn SecretStore, path: &Path) {
    let Ok(text) = std::fs::read_to_string(path) else { return };
    let mut ok = true;
    for (id, secret) in parse_lines(&text) {
        match store.set(id, &secret) {
            Ok(()) => log_info!("account {id}: password moved from the old password file to the keychain"),
            Err(e) => {
                ok = false;
                log_warn!("account {id}: couldn't move the password to the keychain: {e}");
            }
        }
    }
    if ok {
        let _ = std::fs::write(path, vec![0u8; text.len()]);
        let _ = std::fs::remove_file(path);
    }
}

fn parse_lines(text: &str) -> HashMap<i64, String> {
    text.lines()
        .filter_map(|l| l.split_once('=').and_then(|(k, v)| Some((k.parse().ok()?, v.to_string()))))
        .collect()
}

fn missing() -> CoreError {
    CoreError::Auth { message: t!("No password is saved for this account; please enter it again", "这个账户没有保存密码，请重新输入") }
}

#[derive(Default)]
pub struct MemorySecrets(Mutex<HashMap<i64, String>>);

impl SecretStore for MemorySecrets {
    fn get(&self, account_id: i64) -> Result<String> {
        self.0.lock().unwrap().get(&account_id).cloned().ok_or_else(missing)
    }

    fn set(&self, account_id: i64, secret: &str) -> Result<()> {
        self.0.lock().unwrap().insert(account_id, secret.to_string());
        Ok(())
    }

    fn delete(&self, account_id: i64) {
        self.0.lock().unwrap().remove(&account_id);
    }
}

/// Test-only store: a plain text file of `id=password` lines. Used only when the app runs
/// against the local test servers (LITEMAIL_TEST_SECRETS_FILE) with throwaway passwords, so
/// tests never touch the login keychain.
pub struct FileSecrets {
    path: std::path::PathBuf,
    lock: Mutex<()>,
}

impl FileSecrets {
    pub fn new(path: std::path::PathBuf) -> FileSecrets {
        FileSecrets { path, lock: Mutex::new(()) }
    }

    fn read(&self) -> HashMap<i64, String> {
        parse_lines(&std::fs::read_to_string(&self.path).unwrap_or_default())
    }

    fn write(&self, map: &HashMap<i64, String>) -> Result<()> {
        let body: String = map.iter().map(|(k, v)| format!("{k}={v}\n")).collect();
        std::fs::write(&self.path, body)?;
        Ok(())
    }
}

impl SecretStore for FileSecrets {
    fn get(&self, account_id: i64) -> Result<String> {
        let _g = self.lock.lock().unwrap();
        self.read().remove(&account_id).ok_or_else(missing)
    }

    fn set(&self, account_id: i64, secret: &str) -> Result<()> {
        let _g = self.lock.lock().unwrap();
        let mut m = self.read();
        m.insert(account_id, secret.to_string());
        self.write(&m)
    }

    fn delete(&self, account_id: i64) {
        let _g = self.lock.lock().unwrap();
        let mut m = self.read();
        m.remove(&account_id);
        let _ = self.write(&m);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keychain_errors_say_what_happened() {
        let msg = |e: CoreError| match e {
            CoreError::Auth { message } => message,
            other => panic!("{other:?}"),
        };
        crate::i18n::with_language(false, || {
            assert!(msg(read_error(-25300)).contains("No password"));
            assert!(msg(read_error(-128)).contains("couldn't read") && msg(read_error(-128)).contains("-128"));
        });
    }

    #[test]
    fn old_password_file_moves_into_the_store_and_disappears() {
        let dir = std::env::temp_dir().join(format!("lm-import-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("passwords");
        std::fs::write(&path, "1=abcd-efgh-ijkl-mnop\n2=with=equals\n").unwrap();
        let store = MemorySecrets::default();
        import_password_file(&store, &path);
        assert_eq!(store.get(1).unwrap(), "abcd-efgh-ijkl-mnop");
        assert_eq!(store.get(2).unwrap(), "with=equals");
        assert!(!path.exists(), "the plain file is gone");
        import_password_file(&store, &path); // no file: nothing happens
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Uses the login keychain (throwaway item names, removed at the end), so it only runs on
    /// request: `cargo test --lib -- --ignored keychain_round_trip`.
    #[test]
    #[ignore = "uses the login keychain"]
    fn keychain_round_trip() {
        let prefix = format!("lmtest{}", std::process::id());
        let k = Keychain::new(prefix.clone());
        k.set(1, "secret-1").unwrap();
        assert_eq!(Keychain::new(prefix.clone()).get(1).unwrap(), "secret-1");
        k.set(1, "secret-2").unwrap();
        assert_eq!(Keychain::new(prefix.clone()).get(1).unwrap(), "secret-2");
        k.delete(1);
        assert!(Keychain::new(prefix).get(1).is_err());
    }
}
