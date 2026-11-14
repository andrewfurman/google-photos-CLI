//! Portable session bundles; only explicit Chrome import / legacy migration touches Keychain.
use anyhow::{anyhow, bail, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use reqwest_cookie_store::CookieStore;
use serde::{Deserialize, Serialize};
use std::{
    io::{Cursor, Write},
    path::{Path, PathBuf},
};

const PREFIX: &str = "gphotos_v1_";

// Deliberately does not implement Debug: this contains credentials.
#[derive(Clone, Serialize, Deserialize)]
pub struct Session {
    pub account_index: u32,
    pub chrome_profile: Option<String>,
    pub cookies: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) local_chrome_account: Option<String>,
    #[serde(skip)]
    source: Option<PathBuf>,
    #[serde(skip)]
    environment_only: bool,
}
impl Session {
    pub fn new(store: &CookieStore, account_index: u32, profile: Option<String>) -> Result<Self> {
        let mut bytes = Vec::new();
        cookie_store::serde::json::save_incl_expired_and_nonpersistent(store, &mut bytes)
            .map_err(|_| anyhow!("Could not serialize session"))?;
        Ok(Self {
            account_index,
            chrome_profile: profile,
            cookies: serde_json::from_slice(&bytes)?,
            local_chrome_account: None,
            source: None,
            environment_only: false,
        })
    }
    pub fn store(&self) -> Result<CookieStore> {
        let bytes = serde_json::to_vec(&self.cookies)?;
        let store = cookie_store::serde::json::load(Cursor::new(bytes)).map_err(|_| {
            anyhow!("Saved session is invalid; import a new session or cookie file")
        })?;
        Ok(store)
    }
    pub fn token(&self) -> Result<String> {
        Ok(format!(
            "{PREFIX}{}",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(self)?)
        ))
    }
    pub fn from_token(token: &str) -> Result<Self> {
        let invalid = || {
            anyhow!("Invalid GPHOTOS_SESSION value. Use a session exported by 'google-photos-web-cli auth export', or import a Netscape cookie file")
        };
        let token = token.trim().strip_prefix(PREFIX).ok_or_else(invalid)?;
        if token.len() > 1024 * 1024 {
            return Err(invalid());
        }
        let bytes = URL_SAFE_NO_PAD.decode(token).map_err(|_| invalid())?;
        let session: Self = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
        if session.store()?.iter_unexpired().next().is_none()
            && session.local_chrome_account.is_none()
        {
            bail!("The session contains no unexpired cookies. Import fresh browser cookies");
        }
        Ok(session)
    }
    pub fn save(&self) -> Result<()> {
        if self.environment_only {
            return Ok(());
        }
        write_env(
            self.source.as_deref().unwrap_or(&session_path()?),
            &self.token()?,
            true,
        )
    }
    pub fn save_store(&self, store: &CookieStore) -> Result<()> {
        let mut updated = Self::new(store, self.account_index, self.chrome_profile.clone())?;
        updated.local_chrome_account = self.local_chrome_account.clone();
        updated.source = self.source.clone();
        updated.environment_only = self.environment_only;
        updated.save()
    }
    /// Imported sessions are explicitly saved in the selected destination, not their source file.
    pub fn import_file(path: &Path) -> Result<Self> {
        let mut session = read_session(path)?;
        session.local_chrome_account = None;
        Ok(session)
    }
    pub fn portable_token(&self) -> Result<String> {
        let mut portable = self.clone();
        portable.local_chrome_account = None;
        portable.token()
    }
    pub fn chrome_refresh_target(&self) -> Option<(String, u32, String)> {
        if !cfg!(target_os = "macos") || self.environment_only {
            return None;
        }
        Some((
            self.chrome_profile.clone()?,
            self.account_index,
            self.local_chrome_account.clone()?,
        ))
    }
    pub fn export(&self, path: &Path) -> Result<()> {
        write_env(path, &self.portable_token()?, false)
    }
}

pub fn session_path() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("GPHOTOS_SESSION_FILE") {
        if path.is_empty() {
            bail!("GPHOTOS_SESSION_FILE must not be empty");
        }
        return Ok(path.into());
    }
    let config = dirs::config_dir()
        .context("Cannot locate your configuration directory; use --session-file PATH")?;
    let path = config.join("google-photos-web-cli").join(".env");
    // Sessions saved before the rename (when the command was `gphotos`) keep working.
    let legacy = config.join("gphotos").join(".env");
    if !path.exists() && legacy.exists() {
        return Ok(legacy);
    }
    Ok(path)
}
fn read_session(path: &Path) -> Result<Session> {
    if path.to_string_lossy().starts_with(PREFIX) {
        bail!("Pass a session file path, not the session value as a command argument");
    }
    let text = std::fs::read_to_string(path).with_context(|| format!("Cannot read session file {}. Run 'google-photos-web-cli auth chrome', 'google-photos-web-cli auth cookies FILE', or 'google-photos-web-cli auth import FILE'",path.display()))?;
    if text.trim().starts_with(PREFIX) {
        return Session::from_token(text.trim());
    }
    let entries = dotenvy::from_read_iter(text.as_bytes());
    for entry in entries {
        let (key,value) = entry.map_err(|_| anyhow!("Invalid session .env file; expected GPHOTOS_SESSION= followed by the exported value"))?;
        if key == "GPHOTOS_SESSION" {
            return Session::from_token(&value);
        }
    }
    bail!("No GPHOTOS_SESSION value found in the session file")
}
pub fn load() -> Result<Session> {
    if let Ok(token) = std::env::var("GPHOTOS_SESSION") {
        let mut session = Session::from_token(&token)?;
        session.environment_only = true;
        return Ok(session);
    }
    let path = session_path()?;
    let mut session = read_session(&path)?;
    session.source = Some(path);
    Ok(session)
}
pub fn logout() -> Result<()> {
    let path = session_path()?;
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let remaining = text
                .lines()
                .filter(|line| !session_assignment(line))
                .collect::<Vec<_>>()
                .join("\n");
            if remaining.trim().is_empty() {
                std::fs::remove_file(&path)?;
            } else {
                atomic_private_write(&path, format!("{remaining}\n").as_bytes(), true)?;
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).context("Cannot remove the saved session"),
    }
}
fn session_assignment(line: &str) -> bool {
    let line = line
        .trim_start()
        .strip_prefix("export ")
        .unwrap_or(line.trim_start());
    line.split_once('=')
        .is_some_and(|(key, _)| key.trim() == "GPHOTOS_SESSION")
}
fn write_env(path: &Path, token: &str, replace: bool) -> Result<()> {
    let existing = if replace {
        match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e).context("Cannot read existing session file"),
        }
    } else {
        String::new()
    };
    let mut output = existing
        .lines()
        .filter(|line| !session_assignment(line))
        .collect::<Vec<_>>()
        .join("\n");
    if !output.is_empty() {
        output.push('\n');
    }
    output.push_str(&format!("GPHOTOS_SESSION={token}\n"));
    atomic_private_write(path, output.as_bytes(), replace)
}
fn atomic_private_write(path: &Path, bytes: &[u8], replace: bool) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    if !parent.exists() {
        std::fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    // NamedTempFile is created 0600 on Unix. Restrict Windows ACLs before writing credentials.
    #[cfg(windows)]
    restrict_windows_acl(file.path())?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    if replace {
        file.persist(path)
            .map_err(|_| anyhow!("Cannot save session file {}", path.display()))?;
    } else {
        file.persist_noclobber(path)
            .map_err(|_| anyhow!("Cannot export session: destination exists or is not writable"))?;
    }
    Ok(())
}
#[cfg(windows)]
fn restrict_windows_acl(path: &Path) -> Result<()> {
    let identity = std::process::Command::new("whoami")
        .output()
        .context("Cannot determine Windows user")?;
    if !identity.status.success() {
        bail!("Cannot determine Windows user");
    }
    let identity = String::from_utf8(identity.stdout)?.trim().to_owned();
    let status = std::process::Command::new("icacls")
        .arg(path)
        .args(["/inheritance:r", "/grant:r", &format!("{identity}:(F)")])
        .output()
        .context("Cannot restrict session file access")?;
    if !status.status.success() {
        bail!("Could not restrict Windows session file permissions");
    }
    Ok(())
}

/// One-time migration from 0.1. Normal commands never query Keychain.
#[cfg(target_os = "macos")]
pub fn import_legacy_keychain() -> Result<Session> {
    use security_framework::passwords::{generic_password, PasswordOptions};
    let bytes = generic_password(PasswordOptions::new_generic_password(
        "com.andrewfurman.google-photos-cli",
        "session-v1",
    ))
    .context("No accessible legacy Keychain session")?;
    serde_json::from_slice(&bytes).context("Invalid legacy session")
}
#[cfg(not(target_os = "macos"))]
pub fn import_legacy_keychain() -> Result<Session> {
    bail!("Legacy Keychain migration is only available on macOS; import an exported session or Netscape cookie file instead")
}

pub fn photos_domain(host: &str) -> bool {
    matches!(
        host,
        ".google.com" | "google.com" | ".photos.google.com" | "photos.google.com"
    )
}

pub fn import_netscape(path: &Path) -> Result<CookieStore> {
    let text = std::fs::read_to_string(path).context("Could not read cookies file")?;
    let mut store = CookieStore::default();
    let mut count = 0;
    for (i, line) in text.lines().enumerate() {
        let line = line.strip_prefix("#HttpOnly_").unwrap_or(line);
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let fields: Vec<_> = line.splitn(7, '\t').collect();
        if fields.len() != 7 {
            bail!("Invalid Netscape cookies file at line {}", i + 1);
        }
        if !photos_domain(fields[0]) {
            continue;
        }
        let expires: i64 = fields[4].parse().context("Invalid cookie expiration")?;
        if expires > 0 && expires <= chrono::Utc::now().timestamp() {
            continue;
        }
        add_cookie(
            &mut store,
            fields[0],
            fields[2],
            fields[5],
            fields[6],
            fields[3] == "TRUE",
            expires,
        )?;
        count += 1;
    }
    if count == 0 {
        bail!("No unexpired Google Photos cookies found in this file");
    }
    Ok(store)
}

fn add_cookie(
    store: &mut CookieStore,
    host: &str,
    path: &str,
    name: &str,
    value: &str,
    secure: bool,
    expires: i64,
) -> Result<()> {
    // Preserve host-only vs domain cookies. Reject unexpected delimiters rather than injecting attributes.
    if name.contains(['\r', '\n', ';', '='])
        || value.contains(['\r', '\n', ';'])
        || path.contains(['\r', '\n', ';'])
    {
        bail!("Unsupported cookie encoding; session was not imported");
    }
    let mut raw = format!("{name}={value}; Path={path}");
    if host.starts_with('.') {
        raw.push_str(&format!("; Domain={host}"));
    }
    if secure {
        raw.push_str("; Secure");
    }
    if expires > 0 {
        if let Some(date) = chrono::DateTime::from_timestamp(expires, 0) {
            raw.push_str(&format!(
                "; Expires={}",
                date.format("%a, %d %b %Y %H:%M:%S GMT")
            ));
        }
    }
    let origin = url::Url::parse(&format!("https://{}/", host.trim_start_matches('.')))?;
    store
        .parse(&raw, &origin)
        .map_err(|_| anyhow!("Could not import a Google cookie"))?;
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn import_chrome(profile: &str) -> Result<CookieStore> {
    use rusqlite::{Connection, OpenFlags};
    use security_framework::passwords::{generic_password, PasswordOptions};
    use sha2::{Digest, Sha256};
    if profile != "Default"
        && !(profile.starts_with("Profile ") && profile[8..].parse::<u32>().is_ok())
    {
        bail!("Profile must be 'Default' or 'Profile N' (for example 'Profile 1')");
    }
    let root = dirs::home_dir()
        .context("Cannot locate home directory")?
        .join("Library/Application Support/Google/Chrome")
        .join(profile);
    let path = [root.join("Cookies"), root.join("Network/Cookies")]
        .into_iter()
        .find(|p| p.is_file())
        .context("Chrome cookies database not found for that profile")?;
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .context("Cannot read Chrome cookies database; check macOS file permissions")?;
    db.busy_timeout(std::time::Duration::from_secs(5))?;
    let version: String = db.query_row("SELECT value FROM meta WHERE key='version'", [], |r| {
        r.get(0)
    })?;
    let version: u32 = version
        .parse()
        .context("Unknown Chrome cookies database version")?;
    let secret = generic_password(PasswordOptions::new_generic_password("Chrome Safe Storage", "Chrome"))
        .context("Chrome Safe Storage access denied or unavailable. Allow access in the macOS Keychain prompt, or use 'google-photos-web-cli auth cookies FILE'")?;
    let mut key = [0u8; 16];
    pbkdf2::pbkdf2_hmac::<sha1::Sha1>(&secret, b"saltysalt", 1003, &mut key);
    let mut stmt = db.prepare("SELECT host_key, path, name, value, encrypted_value, is_secure, expires_utc FROM cookies WHERE host_key IN ('.google.com','google.com','.photos.google.com','photos.google.com')")?;
    let mut rows = stmt.query([])?;
    let mut store = CookieStore::default();
    let mut count = 0;
    while let Some(row) = rows.next()? {
        let host: String = row.get(0)?;
        let path: String = row.get(1)?;
        let name: String = row.get(2)?;
        let plain: String = row.get(3)?;
        let encrypted: Vec<u8> = row.get(4)?;
        let secure: bool = row.get(5)?;
        let chrome_expires: i64 = row.get(6)?;
        let expires = if chrome_expires == 0 {
            0
        } else {
            chrome_expires / 1_000_000 - 11_644_473_600
        };
        if expires != 0 && expires <= chrono::Utc::now().timestamp() {
            continue;
        }
        let value = if encrypted.is_empty() {
            plain
        } else {
            let mut data = decrypt(&encrypted, &key)?;
            if version >= 24 {
                let hash = Sha256::digest(host.as_bytes());
                if !data.starts_with(&hash) {
                    bail!("Chrome cookie host verification failed; nothing was saved");
                }
                data.drain(..32);
            }
            String::from_utf8(data).context("Unsupported Chrome cookie text encoding")?
        };
        if value.is_empty() {
            continue;
        }
        add_cookie(&mut store, &host, &path, &name, &value, secure, expires)?;
        count += 1;
    }
    if count == 0 {
        bail!("No Google cookies found. Sign in at photos.google.com in this Chrome profile first");
    }
    Ok(store)
}
#[cfg(not(target_os = "macos"))]
pub fn import_chrome(_: &str) -> Result<CookieStore> {
    bail!("Chrome session import currently supports macOS only")
}

#[cfg(target_os = "macos")]
fn decrypt(data: &[u8], key: &[u8; 16]) -> Result<Vec<u8>> {
    use cbc::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
    if !data.starts_with(b"v10") {
        bail!("Unsupported Chrome cookie encryption. Use 'google-photos-web-cli auth cookies FILE' instead");
    }
    cbc::Decryptor::<aes::Aes128>::new(key.into(), (&[b' '; 16]).into())
        .decrypt_padded_vec_mut::<Pkcs7>(&data[3..])
        .map_err(|_| anyhow!("Could not decrypt Chrome session; nothing was saved"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scopes_and_persists_session_cookies() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("cookies.txt");
        std::fs::write(&file, "# Netscape HTTP Cookie File\n.google.com\tTRUE\t/\tTRUE\t0\tSID\ttest-value\n.evilgoogle.com\tTRUE\t/\tTRUE\t0\tBAD\tignore\n").unwrap();
        let store = import_netscape(&file).unwrap();
        let session = Session::new(&store, 0, None).unwrap();
        let loaded = session.store().unwrap();
        assert_eq!(loaded.iter_any().count(), 1);
        assert_eq!(
            loaded
                .get_request_values(&url::Url::parse("https://photos.google.com/").unwrap())
                .count(),
            1
        );
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn decrypts_chrome_and_rejects_unknown_versions() {
        use cbc::cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyIvInit};
        let key = [4u8; 16];
        let encrypted = cbc::Encryptor::<aes::Aes128>::new((&key).into(), (&[b' '; 16]).into())
            .encrypt_padded_vec_mut::<Pkcs7>(b"synthetic-cookie");
        let data = [b"v10".as_slice(), &encrypted].concat();
        assert_eq!(decrypt(&data, &key).unwrap(), b"synthetic-cookie");
        assert!(decrypt(b"v20other", &key).is_err());
    }
}

#[cfg(test)]
mod portable_tests {
    use super::*;
    fn sample() -> Session {
        let mut store = CookieStore::default();
        add_cookie(
            &mut store,
            ".google.com",
            "/",
            "SID",
            "synthetic-secret",
            true,
            0,
        )
        .unwrap();
        Session::new(&store, 1, Some("Default".into())).unwrap()
    }
    #[test]
    fn portable_bundle_roundtrips_without_keychain_and_exports_one_value() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.env");
        let mut s = sample();
        s.local_chrome_account = Some("synthetic@example.invalid".into());
        assert!(Session::from_token(&s.token().unwrap())
            .unwrap()
            .local_chrome_account
            .is_some());
        s.export(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().count(), 1);
        assert!(text.starts_with("GPHOTOS_SESSION=gphotos_v1_"));
        let imported = Session::import_file(&path).unwrap();
        assert_eq!(imported.account_index, 1);
        assert!(read_session(&path).unwrap().local_chrome_account.is_none());
        assert!(imported.chrome_refresh_target().is_none());
        assert_eq!(imported.store().unwrap().iter_any().count(), 1);
        assert!(s.export(&path).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
    #[test]
    fn updates_session_file_without_removing_other_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.env");
        std::fs::write(&path, "OTHER_SETTING=keep\nGPHOTOS_SESSION=old\n").unwrap();
        write_env(&path, &sample().token().unwrap(), true).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("OTHER_SETTING=keep"));
        assert!(!text.contains("SESSION=old"));
        assert_eq!(
            text.lines()
                .filter(|s| s.starts_with("GPHOTOS_SESSION="))
                .count(),
            1
        );
    }
    #[test]
    fn malformed_secret_errors_never_echo_the_input() {
        let value = "gphotos_v1_private-invalid-secret";
        let err = Session::from_token(value).err().unwrap().to_string();
        assert!(!err.contains("private-invalid-secret"));
    }
}
