use crate::{
    auth::Session,
    protocol::{self, Photo},
};
use anyhow::{anyhow, bail, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use reqwest::{blocking::Client, redirect::Policy};
use reqwest_cookie_store::CookieStoreMutex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

pub struct PhotosClient {
    http: Client,
    cookies: Arc<CookieStoreMutex>,
    session: Session,
    globals: Value,
    last_request: Instant,
}
#[derive(Serialize)]
pub struct SearchResult {
    pub query: String,
    pub items: Vec<Photo>,
    pub next_page_token: Option<String>,
    pub complete: bool,
    pub pages_fetched: usize,
    pub person: Option<protocol::Person>,
    pub ordering: String,
    pub candidates_checked: usize,
    pub people_metadata_unavailable: usize,
    pub person_filter: Option<String>,
}
#[derive(Serialize, Deserialize)]
struct Cursor {
    query: String,
    server_token: Option<String>,
    offset: usize,
    account: u32,
}
#[derive(Serialize)]
pub struct DownloadResult {
    pub id: String,
    pub path: PathBuf,
    pub bytes: u64,
    pub representation: String,
    pub sha256: String,
}

fn network<T>(result: reqwest::Result<T>) -> Result<T> {
    result.map_err(|e| {
        anyhow!(
            "Google Photos network request failed{}",
            e.status()
                .map(|s| format!(" (HTTP {s})"))
                .unwrap_or_default()
        )
    })
}
fn allowed_host(url: &url::Url) -> bool {
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port_or_known_default() == Some(443)
        && url.host_str().is_some_and(|h| {
            h == "google.com"
                || h.ends_with(".google.com")
                || h == "googleusercontent.com"
                || h.ends_with(".googleusercontent.com")
        })
}
impl PhotosClient {
    pub fn connect(session: Session) -> Result<Self> {
        let cookies = Arc::new(CookieStoreMutex::new(session.store()?));
        let http = Client::builder()
            .cookie_provider(cookies.clone())
            .timeout(Duration::from_secs(120))
            .connect_timeout(Duration::from_secs(20))
            .user_agent(concat!("google-photos-cli/", env!("CARGO_PKG_VERSION")))
            .redirect(Policy::custom(|attempt| {
                if attempt.previous().len() >= 8 {
                    attempt.error("Too many Google redirects")
                } else if allowed_host(attempt.url()) {
                    attempt.follow()
                } else {
                    attempt.stop()
                }
            }))
            .build()?;
        let url = format!("https://photos.google.com/u/{}/", session.account_index);
        let response = network(http.get(&url).send())?;
        let status = response.status();
        if !status.is_success() {
            bail!("Google Photos login check returned HTTP {status}. Import fresh cookies or an exported session");
        }
        if response.url().host_str() != Some("photos.google.com") {
            bail!("Google requires a browser sign-in. Open photos.google.com in Chrome, then import fresh cookies or an exported session");
        }
        if session.account_index > 0
            && !response
                .url()
                .path()
                .starts_with(&format!("/u/{}/", session.account_index))
        {
            bail!("Google redirected away from the selected account. Check --account and reimport");
        }
        let globals = protocol::globals(&network(response.text())?)?;
        let client = Self {
            http,
            cookies,
            session,
            globals,
            last_request: Instant::now(),
        };
        Ok(client)
    }
    pub fn account_email(&self) -> Option<&str> {
        self.globals["oPEP7c"].as_str()
    }
    pub fn account_index(&self) -> u32 {
        self.session.account_index
    }
    pub fn save(&self) -> Result<()> {
        let store = self
            .cookies
            .lock()
            .map_err(|_| anyhow!("Session storage is busy"))?;
        self.session.save_store(&store)
    }
    pub fn rpc(&mut self, rpc: &str, payload: Value) -> Result<Value> {
        let wait = Duration::from_millis(350).saturating_sub(self.last_request.elapsed());
        std::thread::sleep(wait);
        let path = self.globals["Im6cmf"]
            .as_str()
            .context("Missing Google Photos application path")?;
        if !path.starts_with('/') || path.contains(['?', '#']) {
            bail!("Unexpected Google Photos application path");
        }
        let endpoint = format!("https://photos.google.com{path}/data/batchexecute");
        let source = format!("/u/{}/", self.session.account_index);
        let framing = json!([[[rpc, payload.to_string(), null, "cli"]]]).to_string();
        let response = network(
            self.http
                .post(endpoint)
                .query(&[
                    ("rpcids", rpc),
                    ("source-path", &source),
                    ("f.sid", self.globals["FdrFJe"].as_str().unwrap()),
                    ("bl", self.globals["cfb2h"].as_str().unwrap()),
                    ("rt", "c"),
                ])
                .form(&[
                    ("f.req", framing.as_str()),
                    ("at", self.globals["SNlM0e"].as_str().unwrap()),
                ])
                .send(),
        );
        self.last_request = Instant::now();
        let response = response?;
        let status = response.status();
        if status.as_u16() == 429 {
            let delay = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .filter(|s| s.bytes().all(|c| c.is_ascii_digit()));
            bail!(
                "Google Photos rate limited this session (HTTP 429). {}",
                delay
                    .map(|d| format!("Retry after {d} seconds."))
                    .unwrap_or_else(|| "Wait before retrying.".into())
            );
        }
        if !status.is_success() {
            bail!("Google Photos RPC returned HTTP {status}; sign in with Chrome and reimport if authentication expired");
        }
        protocol::rpc_data(&network(response.text())?, rpc)
    }
    pub fn people(&mut self) -> Result<Vec<protocol::Person>> {
        let data = self.rpc("yQelMe", json!([null, null, null, [[1, null, 1]]]))?;
        protocol::people_index(&data)
    }
    pub fn inspect(&mut self, id: &str) -> Result<protocol::Inspection> {
        check_id(id)?;
        let ext = self.rpc("fDcn4b", json!([id, 1, null, null, 1]))?;
        let info = self.rpc("VrseUb", json!([id, null, null, null, null]))?;
        protocol::parse_inspection(&ext, &info, id, self.session.account_index)
    }
    pub fn search(
        &mut self,
        query: &str,
        limit: usize,
        token: Option<&str>,
    ) -> Result<SearchResult> {
        self.search_filtered(query, limit, token, None)
    }
    pub fn search_person(
        &mut self,
        query: &str,
        name: Option<&str>,
        person_id: Option<&str>,
        limit: usize,
        token: Option<&str>,
    ) -> Result<SearchResult> {
        let people = self.people()?;
        let person = if let Some(id) = person_id {
            check_id(id)?;
            people
                .into_iter()
                .find(|p| p.id == id)
                .unwrap_or(protocol::Person {
                    id: id.into(),
                    name: None,
                })
        } else {
            let name = name.unwrap_or("").trim();
            if name.is_empty() {
                bail!("--person must not be empty");
            }
            let mut matches = people
                .into_iter()
                .filter(|p| {
                    p.name
                        .as_deref()
                        .is_some_and(|n| n.to_lowercase().contains(&name.to_lowercase()))
                })
                .collect::<Vec<_>>();
            let exact = matches
                .iter()
                .filter(|p| {
                    p.name
                        .as_deref()
                        .is_some_and(|n| n.eq_ignore_ascii_case(name))
                })
                .cloned()
                .collect::<Vec<_>>();
            if exact.len() == 1 {
                exact[0].clone()
            } else if matches.len() == 1 {
                matches.remove(0)
            } else if matches.is_empty() {
                bail!("No named Google Photos face group matches {name:?}. Use 'gphotos people --name NAME' to check the groups Google returned");
            } else {
                bail!("More than one face group matches {name:?}. Use the full name or --person-id from 'gphotos people --name NAME'");
            }
        };
        self.search_filtered(query, limit, token, Some(person))
    }
    fn search_filtered(
        &mut self,
        query: &str,
        limit: usize,
        token: Option<&str>,
        person: Option<protocol::Person>,
    ) -> Result<SearchResult> {
        let query = query.trim();
        if query.is_empty() && person.is_none() {
            bail!("Provide a keyword query or --person NAME");
        }
        if !(1..=1000).contains(&limit) {
            bail!("limit must be between 1 and 1000");
        }
        let verify_person = person.is_some() && !query.is_empty();
        let ordering = if query.is_empty() {
            "recent"
        } else {
            "relevant"
        };
        let scope=json!({"query":query,"person":person.as_ref().map(|p|p.id.as_str()),"ordering":ordering,"verified_tags":verify_person}).to_string();
        let mut cursor = decode_cursor(token, &scope, self.session.account_index)?;
        let mut items = Vec::new();
        let mut pages = 0;
        let mut seen_pages = std::collections::HashSet::new();
        let mut seen_ids = std::collections::HashSet::new();
        let mut next = None;
        let mut candidates_checked = 0;
        let mut people_metadata_unavailable = 0;
        let mut stop = false;
        loop {
            if !seen_pages.insert(cursor.server_token.clone()) {
                bail!("Google repeated a search cursor; stopped rather than returning incomplete results as complete");
            }
            let payload = if query.is_empty() {
                protocol::person_search_payload(
                    &person.as_ref().unwrap().id,
                    cursor.server_token.as_deref(),
                )
            } else {
                protocol::keyword_search_payload(query, cursor.server_token.as_deref())
            };
            let data = self.rpc("EzkLib", payload)?;
            let (mut rows, server_next) = protocol::parse_page(&data)?;
            for item in &mut rows {
                item.url = format!(
                    "https://photos.google.com/u/{}/photo/{}",
                    self.session.account_index, item.id
                );
            }
            pages += 1;
            if cursor.offset > rows.len() {
                bail!("Search results changed; rerun without --page-token");
            }
            for (index, item) in rows.iter().enumerate().skip(cursor.offset) {
                if seen_ids.insert(item.id.clone()) {
                    candidates_checked += 1;
                    let matches = if verify_person {
                        let data = self.rpc("fDcn4b", json!([item.id, 1, null, null, 1]))?;
                        match protocol::photo_has_person(
                            &data,
                            &item.id,
                            &person.as_ref().unwrap().id,
                        )? {
                            Some(matches) => matches,
                            None => {
                                people_metadata_unavailable += 1;
                                false
                            }
                        }
                    } else {
                        true
                    };
                    if matches {
                        items.push(item.clone());
                    }
                }
                if items.len() == limit || (verify_person && candidates_checked >= 100) {
                    stop = true;
                    let resume = if index + 1 < rows.len() {
                        Some((cursor.server_token.clone(), index + 1))
                    } else {
                        server_next.clone().map(|t| (Some(t), 0))
                    };
                    if let Some((server_token, offset)) = resume {
                        next = Some(encode_cursor(&Cursor {
                            query: scope.clone(),
                            server_token,
                            offset,
                            account: self.session.account_index,
                        })?);
                    }
                    break;
                }
            }
            if stop {
                break;
            }
            let Some(server_token) = server_next else {
                break;
            };
            if pages >= 50 {
                next = Some(encode_cursor(&Cursor {
                    query: scope.clone(),
                    server_token: Some(server_token),
                    offset: 0,
                    account: self.session.account_index,
                })?);
                break;
            }
            cursor.server_token = Some(server_token);
            cursor.offset = 0;
        }
        if !items.is_empty() {
            let ids: Vec<_> = items.iter().map(|p| p.id.clone()).collect();
            let data = self.rpc("EWgK9e", protocol::batch_payload(&ids))?;
            protocol::filenames(&data, &mut items);
        }
        Ok(SearchResult {
            query: query.into(),
            items,
            complete: next.is_none(),
            next_page_token: next,
            pages_fetched: pages,
            person_filter: person.as_ref().map(|_| {
                if verify_person {
                    "verified_photo_tags"
                } else {
                    "google_face_group"
                }
                .into()
            }),
            person,
            ordering: ordering.into(),
            candidates_checked,
            people_metadata_unavailable,
        })
    }
    pub fn download(
        &mut self,
        id: &str,
        output: Option<&Path>,
        directory: &Path,
    ) -> Result<DownloadResult> {
        check_id(id)?;
        let data = self.rpc("VrseUb", json!([id, null, null, null, null]))?;
        let (address, representation) =
            if let Some(url) = protocol::field(&data, 7).as_str().filter(|s| !s.is_empty()) {
                (url, "original")
            } else if let Some(url) = protocol::field(&data, 1).as_str().filter(|s| !s.is_empty()) {
                (url, "full-size")
            } else {
                bail!("Google Photos returned no full-size download for this item");
            };
        let url =
            url::Url::parse(address).map_err(|_| anyhow!("Invalid Google Photos media URL"))?;
        if !allowed_host(&url) {
            bail!("Refusing an unexpected media download host");
        }
        let info = self.rpc("fDcn4b", json!([id, 1, null, null, 1]))?;
        let filename = protocol::field(protocol::field(&info, 0), 2)
            .as_str()
            .map(|s| safe_filename(s, id))
            .unwrap_or_else(|| format!("{id}.download"));
        let target = output
            .map(Path::to_path_buf)
            .unwrap_or_else(|| directory.join(filename));
        let parent = target
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        std::fs::create_dir_all(parent).context("Could not create download directory")?;
        if target.exists() {
            bail!(
                "Destination already exists: {}. Choose a different --output path",
                target.display()
            );
        }
        let mut response = network(self.http.get(url).send())?;
        if response.status().as_u16() != 200 {
            bail!("Google Photos download returned HTTP {}", response.status());
        }
        if response
            .headers()
            .get("content-type")
            .and_then(|s| s.to_str().ok())
            .is_some_and(|s| s.starts_with("text/") || s.contains("json"))
        {
            bail!(
                "Google returned a login/error page instead of a media file; reimport your session"
            );
        }
        let expected = response.content_length();
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        let (bytes, hash) = copy_media(&mut response, &mut temporary, expected)?;
        temporary.as_file().sync_all()?;
        temporary
            .persist_noclobber(&target)
            .map_err(|_| anyhow!("Could not save download; destination may already exist"))?;
        Ok(DownloadResult {
            id: id.into(),
            path: std::fs::canonicalize(target)?,
            bytes,
            representation: representation.into(),
            sha256: hash,
        })
    }
}
pub fn check_id(id: &str) -> Result<()> {
    if !(8..=256).contains(&id.len())
        || !id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    {
        bail!("Expected a Google Photos photo ID from 'gphotos search', containing letters, digits, '-' or '_'");
    }
    Ok(())
}
fn encode_cursor(cursor: &Cursor) -> Result<String> {
    Ok(URL_SAFE_NO_PAD.encode(serde_json::to_vec(cursor)?))
}
fn decode_cursor(token: Option<&str>, query: &str, account: u32) -> Result<Cursor> {
    let Some(token) = token else {
        return Ok(Cursor {
            query: query.into(),
            server_token: None,
            offset: 0,
            account,
        });
    };
    let invalid =
        || anyhow!("Invalid --page-token; use the token returned by the same search and account");
    if token.len() > 65536 {
        return Err(invalid());
    }
    let cursor: Cursor =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(token).map_err(|_| invalid())?)
            .map_err(|_| invalid())?;
    if cursor.query != query || cursor.account != account {
        return Err(invalid());
    }
    Ok(cursor)
}
fn safe_filename(name: &str, fallback: &str) -> String {
    let name: String = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, ':' | '\0'))
        .collect();
    let name = name.trim_matches([' ', '.']);
    if name.is_empty() || name.len() > 200 {
        format!("{fallback}.download")
    } else {
        name.into()
    }
}
fn copy_media(
    reader: &mut impl Read,
    writer: &mut impl Write,
    expected: Option<u64>,
) -> Result<(u64, String)> {
    let mut hash = Sha256::new();
    let mut count = 0;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|_| anyhow!("Media transfer interrupted; destination was not replaced"))?;
        if read == 0 {
            break;
        }
        writer.write_all(&buffer[..read])?;
        hash.update(&buffer[..read]);
        count += read as u64;
    }
    if count == 0 || expected.is_some_and(|n| n != count) {
        bail!("Incomplete media transfer; destination was not replaced");
    }
    Ok((count, format!("{:x}", hash.finalize())))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cursor_is_bound_to_query_and_account() {
        let token = encode_cursor(&Cursor {
            query: "Emma beach".into(),
            server_token: Some("page".into()),
            offset: 12,
            account: 0,
        })
        .unwrap();
        assert_eq!(
            decode_cursor(Some(&token), "Emma beach", 0).unwrap().offset,
            12
        );
        assert!(decode_cursor(Some(&token), "Kate", 0).is_err());
        assert!(decode_cursor(Some(&token), "Emma beach", 1).is_err());
    }
    #[test]
    fn paths_and_hosts_cannot_escape() {
        assert_eq!(safe_filename("../../hello.jpg", "fallback"), "hello.jpg");
        assert_eq!(safe_filename("..\\hello.jpg", "fallback"), "hello.jpg");
        assert!(check_id("../../secret").is_err());
        for url in [
            "https://google.com.evil.test/file",
            "http://photos.google.com/file",
            "https://user@photos.google.com/file",
        ] {
            assert!(!allowed_host(&url::Url::parse(url).unwrap()));
        }
        assert!(allowed_host(
            &url::Url::parse("https://lh3.googleusercontent.com/a").unwrap()
        ));
    }
    #[test]
    fn detects_truncation_and_empty_downloads() {
        assert!(copy_media(&mut &b"abc"[..], &mut Vec::new(), Some(4)).is_err());
        assert!(copy_media(&mut &b""[..], &mut Vec::new(), None).is_err());
        let mut out = Vec::new();
        assert_eq!(
            copy_media(&mut &b"abc"[..], &mut out, Some(3)).unwrap().0,
            3
        );
        assert_eq!(out, b"abc");
    }
}
