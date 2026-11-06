//! Import only cookies applicable to photos.google.com; never log secret values.
use anyhow::{anyhow, bail, Context, Result};
use reqwest_cookie_store::CookieStore;
use serde::{Deserialize, Serialize};
use std::{io::Cursor, path::Path};

const SERVICE: &str = "com.andrewfurman.google-photos-cli";
const ACCOUNT: &str = "session-v1";

// Deliberately does not implement Debug: this contains credentials.
#[derive(Serialize, Deserialize)]
pub struct Session {
    pub account_index: u32,
    pub chrome_profile: Option<String>,
    pub cookies: serde_json::Value,
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
        })
    }
    pub fn store(&self) -> Result<CookieStore> {
        let bytes = serde_json::to_vec(&self.cookies)?;
        cookie_store::serde::json::load(Cursor::new(bytes))
            .map_err(|_| anyhow!("Saved session is invalid; run 'gphotos auth chrome' again"))
    }
    #[cfg(target_os = "macos")]
    pub fn save(&self) -> Result<()> {
        security_framework::passwords::set_generic_password(
            SERVICE,
            ACCOUNT,
            &serde_json::to_vec(self)?,
        )
        .context("Could not save Google Photos session in macOS Keychain")
    }
    #[cfg(not(target_os = "macos"))]
    pub fn save(&self) -> Result<()> {
        bail!("Session storage currently requires macOS Keychain")
    }
}

#[cfg(target_os = "macos")]
pub fn load() -> Result<Session> {
    use security_framework::passwords::{generic_password, PasswordOptions};
    let bytes = generic_password(PasswordOptions::new_generic_password(SERVICE, ACCOUNT))
        .context("No accessible saved session. Run 'gphotos auth chrome' (or unlock Keychain)")?;
    serde_json::from_slice(&bytes).context("Invalid saved session; run 'gphotos auth chrome'")
}
#[cfg(not(target_os = "macos"))]
pub fn load() -> Result<Session> {
    bail!("Session storage currently requires macOS Keychain")
}

#[cfg(target_os = "macos")]
pub fn logout() -> Result<()> {
    match security_framework::passwords::delete_generic_password(SERVICE, ACCOUNT) {
        Ok(()) => Ok(()),
        Err(e) if e.code() == -25300 => Ok(()),
        Err(e) => Err(e).context("Could not remove CLI session from Keychain"),
    }
}
#[cfg(not(target_os = "macos"))]
pub fn logout() -> Result<()> {
    bail!("Session storage currently requires macOS Keychain")
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
        .context("Chrome Safe Storage access denied or unavailable. Allow access in the macOS Keychain prompt, or use 'gphotos auth cookies FILE'")?;
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
        bail!("Unsupported Chrome cookie encryption. Use 'gphotos auth cookies FILE' instead");
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
