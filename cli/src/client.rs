use crate::{
    auth::Session,
    protocol::{self, Photo, RateLimit, SessionExpired},
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum SearchSort {
    Relevant,
    Recent,
}

#[derive(Clone)]
pub struct SearchOptions {
    pub limit: usize,
    pub all: bool,
    pub sort: Option<SearchSort>,
    pub max_seconds: u64,
    pub dates: crate::search::DateRange,
}
impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            limit: 20,
            all: false,
            sort: None,
            max_seconds: 120,
            dates: crate::search::DateRange::default(),
        }
    }
}

pub struct PhotosClient {
    http: Client,
    cookies: Arc<CookieStoreMutex>,
    session: Session,
    globals: Value,
    last_request: Instant,
    #[cfg(test)]
    rpc_endpoint: Option<String>,
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
    pub people: Vec<protocol::Person>,
    pub person_match: String,
    pub date_range: Value,
    pub date_metadata_unavailable: usize,
    pub stop_reason: String,
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

#[derive(Debug)]
struct RequestTimedOut;

impl std::fmt::Display for RequestTimedOut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Google Photos request timed out")
    }
}

impl std::error::Error for RequestTimedOut {}

fn network<T>(result: reqwest::Result<T>) -> Result<T> {
    result.map_err(|e| {
        if e.is_timeout() {
            return RequestTimedOut.into();
        }
        anyhow!(
            "Google Photos network request failed{}",
            e.status()
                .map(|s| format!(" (HTTP {s})"))
                .unwrap_or_default()
        )
    })
}
fn retry_after(value: &str, now: chrono::DateTime<chrono::Utc>) -> Option<Duration> {
    let value = value.trim();
    if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) {
        // An unrepresentably large delay must never become a short fallback retry.
        return Some(Duration::from_secs(value.parse().unwrap_or(u64::MAX)));
    }
    let date = chrono::DateTime::parse_from_rfc2822(value).ok()?;
    Some(date.signed_duration_since(now).to_std().unwrap_or_default())
}

fn retry_search<T>(
    deadline: Instant,
    mut request: impl FnMut(Duration) -> Result<T>,
) -> Result<Option<T>> {
    let mut backoff = Duration::from_secs(1);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(None);
        }
        match request(remaining) {
            Ok(value) => return Ok(Some(value)),
            Err(error) if error.is::<RequestTimedOut>() => return Ok(None),
            Err(error) => {
                let Some(limit) = error.downcast_ref::<RateLimit>() else {
                    return Err(error);
                };
                let delay = limit.retry_after.unwrap_or(backoff);
                if delay >= deadline.saturating_duration_since(Instant::now()) {
                    eprintln!("{limit} The retry would exceed the search time budget.");
                    return Ok(None);
                }
                eprintln!(
                    "Google Photos is limiting requests ({}). Retrying in {:.1} seconds…",
                    limit.source,
                    delay.as_secs_f64()
                );
                std::thread::sleep(delay);
                backoff = (backoff * 2).min(Duration::from_secs(30));
            }
        }
    }
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
        let store = session.store()?;
        if store.iter_unexpired().next().is_none() {
            return Err(SessionExpired.into());
        }
        let cookies = Arc::new(CookieStoreMutex::new(store));
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
        if status == reqwest::StatusCode::UNAUTHORIZED
            || response.url().host_str() == Some("accounts.google.com")
        {
            return Err(SessionExpired.into());
        }
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
            #[cfg(test)]
            rpc_endpoint: None,
        };
        Ok(client)
    }
    pub fn enable_chrome_refresh(&mut self) {
        self.session.local_chrome_account = self.account_email().map(str::to_owned);
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
        self.rpc_with_timeout(rpc, &payload, Duration::from_secs(120))
    }
    fn rpc_with_timeout(&mut self, rpc: &str, payload: &Value, timeout: Duration) -> Result<Value> {
        let framing = json!([[[rpc, payload.to_string(), null, "cli"]]]).to_string();
        self.rpc_call(rpc, framing, timeout, |body| protocol::rpc_data(body, rpc))
    }
    pub fn rpc_batch(&mut self, rpc: &str, payloads: &[Value]) -> Result<Vec<Value>> {
        self.rpc_batch_with_timeout(rpc, payloads, Duration::from_secs(120))
    }
    fn rpc_batch_with_timeout(
        &mut self,
        rpc: &str,
        payloads: &[Value],
        timeout: Duration,
    ) -> Result<Vec<Value>> {
        if payloads.is_empty() {
            return Ok(Vec::new());
        }
        if payloads.len() > 20 {
            bail!("Metadata batches are limited to 20 photos");
        }
        let requests = payloads
            .iter()
            .enumerate()
            .map(|(i, p)| json!([rpc, p.to_string(), null, format!("p{i}")]))
            .collect::<Vec<_>>();
        self.rpc_call(rpc, json!([requests]).to_string(), timeout, |body| {
            protocol::rpc_batch_data(body, rpc, payloads.len())
        })
    }
    fn rpc_call<T>(
        &mut self,
        rpc: &str,
        framing: String,
        timeout: Duration,
        decode: impl FnOnce(&str) -> Result<T>,
    ) -> Result<T> {
        let started = Instant::now();
        let wait = Duration::from_millis(350).saturating_sub(self.last_request.elapsed());
        if wait >= timeout {
            return Err(RequestTimedOut.into());
        }
        std::thread::sleep(wait);
        let path = self.globals["Im6cmf"]
            .as_str()
            .context("Missing Google Photos application path")?;
        if !path.starts_with('/') || path.contains(['?', '#']) {
            bail!("Unexpected Google Photos application path");
        }
        let endpoint = format!("https://photos.google.com{path}/data/batchexecute");
        #[cfg(test)]
        let endpoint = self.rpc_endpoint.clone().unwrap_or(endpoint);
        let source = format!("/u/{}/", self.session.account_index);
        let remaining = timeout.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return Err(RequestTimedOut.into());
        }
        let response = network(
            self.http
                .post(endpoint)
                .timeout(remaining.min(Duration::from_secs(120)))
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
        let delay = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|value| retry_after(value, chrono::Utc::now()));
        if status.as_u16() == 429 {
            return Err(RateLimit {
                source: "HTTP 429",
                retry_after: delay,
            }
            .into());
        }
        if status == reqwest::StatusCode::UNAUTHORIZED
            || response.url().host_str() == Some("accounts.google.com")
        {
            return Err(SessionExpired.into());
        }
        if !status.is_success() {
            bail!("Google Photos RPC returned HTTP {status}; sign in with Chrome and reimport if authentication expired");
        }
        let mut result = decode(&network(response.text())?);
        if let Err(error) = &mut result {
            if let Some(limit) = error.downcast_mut::<RateLimit>() {
                limit.retry_after = delay;
            }
        }
        result
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
    pub fn photo_faces(&mut self, id: &str) -> Result<protocol::PhotoFaces> {
        check_id(id)?;
        let ext = self.rpc("fDcn4b", json!([id, 1, null, null, 1]))?;
        protocol::parse_photo_faces(&ext, id)
    }
    /// Resolve exactly one existing face group for a write, by unique name or exact ID.
    pub fn resolve_person(
        &mut self,
        name: Option<&str>,
        id: Option<&str>,
    ) -> Result<protocol::Person> {
        let index = self.people()?;
        resolve_tag_target(&index, name, id)
    }
    /// Assign a detected face to an existing face group (web "Edit faces" > Done).
    /// The caller must have checked the face and person; this sends one write.
    pub fn tag_face(
        &mut self,
        photo: &protocol::PhotoFaces,
        face_id: &str,
        person_id: &str,
    ) -> Result<()> {
        let payload = protocol::tag_face_payload(&photo.media_key, face_id, person_id);
        let data = self.rpc("k9FTge", payload)?;
        protocol::confirm_face_tag(&data, &photo.id, face_id, person_id)
    }
    pub fn search(
        &mut self,
        query: &str,
        limit: usize,
        token: Option<&str>,
    ) -> Result<SearchResult> {
        self.search_people(
            query,
            &[],
            &[],
            SearchOptions {
                limit,
                ..SearchOptions::default()
            },
            token,
        )
    }
    pub fn search_person(
        &mut self,
        query: &str,
        name: Option<&str>,
        person_id: Option<&str>,
        limit: usize,
        token: Option<&str>,
    ) -> Result<SearchResult> {
        self.search_people(
            query,
            &name.map(str::to_owned).into_iter().collect::<Vec<_>>(),
            &person_id.map(str::to_owned).into_iter().collect::<Vec<_>>(),
            SearchOptions {
                limit,
                ..SearchOptions::default()
            },
            token,
        )
    }
    pub fn search_people(
        &mut self,
        query: &str,
        names: &[String],
        ids: &[String],
        options: SearchOptions,
        token: Option<&str>,
    ) -> Result<SearchResult> {
        let query = query.trim();
        if query.is_empty() && names.is_empty() && ids.is_empty() && !options.dates.active() {
            bail!("Provide a search query, --person NAME, or --from/--to dates");
        }
        if !(1..=10000).contains(&options.limit) || !(1..=86400).contains(&options.max_seconds) {
            bail!("limit must be 1–10000 and max-seconds must be 1–86400");
        }
        let deadline = Instant::now() + Duration::from_secs(options.max_seconds);
        let index = if names.is_empty() && ids.is_empty() {
            Vec::new()
        } else {
            let payload = json!([null, null, null, [[1, null, 1]]]);
            let data = retry_search(deadline, |remaining| {
                self.rpc_with_timeout("yQelMe", &payload, remaining)
            })?
            .context("Search time limit reached while resolving people. Retry with the same filters and page token, if supplied")?;
            protocol::people_index(&data)?
        };
        let people = resolve_people(&index, names, ids)?;
        self.search_filtered(query, people, options, token, deadline)
    }
    fn search_filtered(
        &mut self,
        query: &str,
        people: Vec<protocol::Person>,
        options: SearchOptions,
        token: Option<&str>,
        deadline: Instant,
    ) -> Result<SearchResult> {
        let timeline = query.is_empty() && options.dates.active();
        let verify_people =
            !people.is_empty() && (!query.is_empty() || people.len() > 1 || timeline);
        let required = people.iter().map(|p| p.id.clone()).collect::<Vec<_>>();
        let sort = match (query.is_empty(), options.sort) {
            (true, Some(SearchSort::Relevant)) => {
                bail!("--sort relevant requires a text query. People-only and date-only searches use recent order");
            }
            (true, _) => SearchSort::Recent,
            (false, sort) => sort.unwrap_or(SearchSort::Relevant),
        };
        let ordering = match sort {
            SearchSort::Relevant => "relevant",
            SearchSort::Recent => "recent",
        };
        let date_range = json!({"from":options.dates.from.map(|d|d.to_string()),"to":options.dates.to.map(|d|d.to_string())});
        let scope=json!({"query":query,"people":required,"ordering":ordering,"dates":date_range,"timeline":timeline}).to_string();
        let mut cursor = decode_cursor(token, &scope, self.session.account_index)?;
        let mut items = Vec::new();
        let mut pages = 0;
        let mut seen_pages = std::collections::HashSet::new();
        let mut seen_ids = std::collections::HashSet::new();
        let mut next = None;
        let mut candidates_checked = 0;
        let mut people_metadata_unavailable = 0;
        let mut date_metadata_unavailable = 0;
        let mut stop_reason = "complete".to_owned();
        let mut stop = false;
        let candidate_limit = if options.all {
            usize::MAX
        } else if verify_people || options.dates.active() {
            100
        } else {
            usize::MAX
        };
        loop {
            if !seen_pages.insert(cursor.server_token.clone()) {
                bail!("Google repeated a search cursor; stopped rather than claiming complete results");
            }
            let (rpc, payload) = if timeline {
                (
                    "lcxiM",
                    json!([
                        cursor.server_token,
                        options.dates.server_before_ms(),
                        100,
                        null,
                        1,
                        3
                    ]),
                )
            } else if query.is_empty() {
                (
                    "EzkLib",
                    protocol::person_search_payload(&people[0].id, cursor.server_token.as_deref()),
                )
            } else {
                (
                    "EzkLib",
                    protocol::keyword_search_payload_ordered(
                        query,
                        cursor.server_token.as_deref(),
                        sort == SearchSort::Recent,
                    ),
                )
            };
            let Some(data) = retry_search(deadline, |remaining| {
                self.rpc_with_timeout(rpc, &payload, remaining)
            })?
            else {
                next = Some(encode_cursor(&cursor)?);
                stop_reason = "time_limit".into();
                break;
            };
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
            let mut metadata = std::collections::HashMap::<String, Option<bool>>::new();
            for (index, item) in rows.iter().enumerate().skip(cursor.offset) {
                if Instant::now() >= deadline {
                    next = Some(encode_cursor(&Cursor {
                        query: scope.clone(),
                        server_token: cursor.server_token.clone(),
                        offset: index,
                        account: self.session.account_index,
                    })?);
                    stop_reason = "time_limit".into();
                    stop = true;
                    break;
                }
                if seen_ids.insert(item.id.clone()) {
                    let date_matches = match options.dates.matches(item.taken_at.as_deref()) {
                        Some(value) => value,
                        None => {
                            date_metadata_unavailable += 1;
                            false
                        }
                    };
                    let people_match = if date_matches && verify_people {
                        if !metadata.contains_key(&item.id) {
                            let mut batch_ids = std::collections::HashSet::new();
                            let candidates = rows[index..]
                                .iter()
                                .take(20)
                                .filter(|p| {
                                    options.dates.matches(p.taken_at.as_deref()) == Some(true)
                                        && batch_ids.insert(p.id.clone())
                                })
                                .collect::<Vec<_>>();
                            let payloads = candidates
                                .iter()
                                .map(|p| json!([p.id, 1, null, null, 1]))
                                .collect::<Vec<_>>();
                            let Some(values) = retry_search(deadline, |remaining| {
                                self.rpc_batch_with_timeout("fDcn4b", &payloads, remaining)
                            })?
                            else {
                                cursor.offset = index;
                                next = Some(encode_cursor(&cursor)?);
                                stop_reason = "time_limit".into();
                                stop = true;
                                break;
                            };
                            for (photo, value) in candidates.iter().zip(values) {
                                metadata.insert(
                                    photo.id.clone(),
                                    protocol::photo_has_people(&value, &photo.id, &required)?,
                                );
                            }
                        }
                        match metadata.get(&item.id).copied().flatten() {
                            Some(value) => value,
                            None => {
                                people_metadata_unavailable += 1;
                                false
                            }
                        }
                    } else {
                        true
                    };
                    candidates_checked += 1;
                    if date_matches && people_match {
                        items.push(item.clone());
                    }
                }
                if items.len() >= options.limit || candidates_checked >= candidate_limit {
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
                        stop_reason = if items.len() >= options.limit {
                            "result_limit"
                        } else {
                            "candidate_limit"
                        }
                        .into();
                    }
                    stop = true;
                    break;
                }
            }
            if options.all {
                eprintln!(
                    "Search page {pages}: {} match(es), {candidates_checked} candidates checked",
                    items.len()
                );
            }
            if stop {
                break;
            }
            // The library endpoint is descending by capture timestamp. Keep a timezone margin.
            if timeline
                && rows
                    .last()
                    .is_some_and(|p| options.dates.older_than_range(p.taken_at.as_deref()))
            {
                break;
            }
            let Some(server_token) = server_next else {
                break;
            };
            if pages >= if options.all { 1000 } else { 50 } {
                next = Some(encode_cursor(&Cursor {
                    query: scope.clone(),
                    server_token: Some(server_token),
                    offset: 0,
                    account: self.session.account_index,
                })?);
                stop_reason = "page_limit".into();
                break;
            }
            cursor.server_token = Some(server_token);
            cursor.offset = 0;
        }
        // A stopped search must not issue another RPC before the retry delay.
        if stop_reason != "time_limit" {
            for chunk in items.chunks_mut(500) {
                let ids = chunk.iter().map(|p| p.id.clone()).collect::<Vec<_>>();
                let payload = protocol::batch_payload(&ids);
                let Some(data) = retry_search(deadline, |remaining| {
                    self.rpc_with_timeout("EWgK9e", &payload, remaining)
                })?
                else {
                    eprintln!("Search time budget reached before all filenames could be fetched.");
                    break;
                };
                protocol::filenames(&data, chunk);
            }
        }
        Ok(SearchResult {
            query: query.into(),
            person: if people.len() == 1 {
                Some(people[0].clone())
            } else {
                None
            },
            person_filter: if people.is_empty() {
                None
            } else {
                Some(
                    if verify_people {
                        "verified_photo_tags"
                    } else {
                        "google_face_group"
                    }
                    .into(),
                )
            },
            people,
            person_match: "all".into(),
            items,
            complete: next.is_none(),
            next_page_token: next,
            pages_fetched: pages,
            ordering: ordering.into(),
            candidates_checked,
            people_metadata_unavailable,
            date_metadata_unavailable,
            date_range,
            stop_reason,
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
fn resolve_people(
    index: &[protocol::Person],
    names: &[String],
    ids: &[String],
) -> Result<Vec<protocol::Person>> {
    let mut resolved = Vec::new();
    for name in names {
        let name = name.trim();
        if name.is_empty() {
            bail!("--person must not be empty");
        }
        let exact = index
            .iter()
            .filter(|p| {
                p.name
                    .as_deref()
                    .is_some_and(|n| n.eq_ignore_ascii_case(name))
            })
            .collect::<Vec<_>>();
        let matches = if exact.is_empty() {
            index
                .iter()
                .filter(|p| {
                    p.name
                        .as_deref()
                        .is_some_and(|n| n.to_lowercase().contains(&name.to_lowercase()))
                })
                .collect::<Vec<_>>()
        } else {
            exact
        };
        match matches.as_slice(){
            [person]=>resolved.push((*person).clone()),
            []=>bail!("No named face group matches {name:?}. Check 'google-photos-web-cli people --name NAME'"),
            _=>bail!("More than one face group matches {name:?}. Use the full name or --person-id from 'google-photos-web-cli people --name NAME'"),
        }
    }
    for id in ids {
        check_id(id)?;
        resolved.push(
            index
                .iter()
                .find(|p| p.id == *id)
                .cloned()
                .unwrap_or(protocol::Person {
                    id: id.clone(),
                    name: None,
                }),
        );
    }
    let mut seen = std::collections::HashSet::new();
    resolved.retain(|p| seen.insert(p.id.clone()));
    Ok(resolved)
}

fn resolve_tag_target(
    index: &[protocol::Person],
    name: Option<&str>,
    id: Option<&str>,
) -> Result<protocol::Person> {
    match (name, id) {
        (Some(name), None) => Ok(resolve_people(index, &[name.to_owned()], &[])?.remove(0)),
        (None, Some(id)) => {
            check_id(id)?;
            index.iter().find(|p| p.id == id).cloned().with_context(|| {
                format!("No face group with ID {id:?}. Check 'google-photos-web-cli people --include-unnamed'")
            })
        }
        _ => bail!("Choose exactly one of --person NAME or --person-id ID"),
    }
}
pub fn check_id(id: &str) -> Result<()> {
    if !(8..=256).contains(&id.len())
        || !id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    {
        bail!("Expected a Google Photos photo ID from 'google-photos-web-cli search', containing letters, digits, '-' or '_'");
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
    fn retry_after_accepts_seconds_and_http_dates() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-11-09T20:00:00Z")
            .unwrap()
            .to_utc();
        assert_eq!(retry_after("12", now), Some(Duration::from_secs(12)));
        assert_eq!(
            retry_after("Mon, 09 Nov 2026 20:00:07 GMT", now),
            Some(Duration::from_secs(7))
        );
        assert_eq!(
            retry_after("Mon, 09 Nov 2026 19:59:00 GMT", now),
            Some(Duration::ZERO)
        );
        assert_eq!(retry_after("invalid", now), None);
    }

    #[test]
    fn limited_search_retries_and_preserves_a_resumable_partial_page() {
        use std::{
            io::{BufRead, BufReader},
            net::TcpListener,
        };

        fn rpc_frame(rpc: &str, payload: Value) -> String {
            json!([["wrb.fr", rpc, payload.to_string()]]).to_string()
        }

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let responses = [
                (
                    "EzkLib",
                    "200 OK",
                    "",
                    r#"[["wrb.fr","EzkLib",null,null,null,[8]]]"#.into(),
                ),
                (
                    "EzkLib",
                    "200 OK",
                    "",
                    rpc_frame("EzkLib", json!([[["first"]], "next"])),
                ),
                (
                    "EzkLib",
                    "429 Too Many Requests",
                    "Retry-After: 120\r\n",
                    String::new(),
                ),
                (
                    "EzkLib",
                    "200 OK",
                    "",
                    rpc_frame("EzkLib", json!([[["second"]]])),
                ),
                ("EWgK9e", "200 OK", "", rpc_frame("EWgK9e", json!([]))),
            ];
            for (index, (rpc, status, headers, body)) in responses.into_iter().enumerate() {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(&mut stream);
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                assert!(line.contains(&format!("rpcids={rpc}")));
                let mut length = 0;
                loop {
                    line.clear();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" {
                        break;
                    }
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = value.trim().parse().unwrap();
                    }
                }
                let mut request_body = vec![0; length];
                reader.read_exact(&mut request_body).unwrap();
                if matches!(index, 2 | 3) {
                    let (_, framing) = url::form_urlencoded::parse(&request_body)
                        .find(|(key, _)| key == "f.req")
                        .unwrap();
                    let frame: Value = serde_json::from_str(&framing).unwrap();
                    let payload: Value =
                        serde_json::from_str(frame[0][0][1].as_str().unwrap()).unwrap();
                    assert_eq!(payload[2], "next");
                }
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
        });
        let mut client = PhotosClient {
            http: Client::builder().no_proxy().build().unwrap(),
            cookies: Arc::new(CookieStoreMutex::default()),
            session: Session::new(&cookie_store::CookieStore::default(), 0, None).unwrap(),
            globals: json!({"Im6cmf":"/_","FdrFJe":"","cfb2h":"","SNlM0e":""}),
            last_request: Instant::now(),
            rpc_endpoint: Some(endpoint),
        };
        let partial = client.search("beach", 20, None).unwrap();
        assert_eq!(partial.stop_reason, "time_limit");
        assert!(!partial.complete);
        assert_eq!(partial.items.len(), 1);
        assert_eq!(partial.items[0].id, "first");
        let resumed = client
            .search("beach", 20, partial.next_page_token.as_deref())
            .unwrap();
        assert!(resumed.complete);
        assert_eq!(resumed.items.len(), 1);
        assert_eq!(resumed.items[0].id, "second");
        server.join().unwrap();
    }

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

#[cfg(test)]
mod people_resolution_tests {
    use super::*;
    fn index() -> Vec<protocol::Person> {
        vec![
            protocol::Person {
                id: "group-alice-id".into(),
                name: Some("Alice Example".into()),
            },
            protocol::Person {
                id: "group-bob-id".into(),
                name: Some("Bob Example".into()),
            },
            protocol::Person {
                id: "group-bob-other-id".into(),
                name: Some("Bob Other".into()),
            },
        ]
    }
    #[test]
    fn tag_targets_must_be_one_existing_group() {
        let index = index();
        assert_eq!(
            resolve_tag_target(&index, Some("alice example"), None)
                .unwrap()
                .id,
            index[0].id
        );
        let id = index[0].id.clone();
        assert_eq!(
            resolve_tag_target(&index, None, Some(&id)).unwrap().name,
            index[0].name
        );
        assert!(resolve_tag_target(&index, None, Some("AF1QipNotAGroup")).is_err());
        assert!(resolve_tag_target(&index, Some("Nobody"), None).is_err());
        assert!(resolve_tag_target(&index, Some("Bob"), None).is_err());
        assert!(resolve_tag_target(&index, None, None).is_err());
        assert!(resolve_tag_target(&index, Some("x"), Some(&id)).is_err());
    }
    #[test]
    fn multiple_names_resolve_once_and_duplicate_constraints_are_removed() {
        let groups = resolve_people(
            &index(),
            &["Alice".into(), "Bob Example".into(), "Alice".into()],
            &[],
        )
        .unwrap();
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[1].id, "group-bob-id");
        assert!(resolve_people(&index(), &["Bob".into()], &[]).is_err());
        assert!(resolve_people(&index(), &["Missing".into()], &[]).is_err());
    }
    #[test]
    fn pagination_tokens_are_bound_to_all_people_and_date_bounds() {
        let scope =
            json!({"people":["group-alice-id","group-bob-id"],"from":"2026-01-01"}).to_string();
        let token = encode_cursor(&Cursor {
            query: scope.clone(),
            server_token: Some("next-google-page".into()),
            offset: 17,
            account: 0,
        })
        .unwrap();
        assert_eq!(decode_cursor(Some(&token), &scope, 0).unwrap().offset, 17);
        let changed = json!({"people":["group-alice-id"],"from":"2026-01-01"}).to_string();
        assert!(decode_cursor(Some(&token), &changed, 0).is_err());
        let changed =
            json!({"people":["group-alice-id","group-bob-id"],"from":"2026-02-01"}).to_string();
        assert!(decode_cursor(Some(&token), &changed, 0).is_err());
    }
}
