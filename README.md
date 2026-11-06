# google-photos-CLI

An unofficial **Rust CLI for Google Photos**. Import your own Google Chrome session,
search your existing library by named person or keyword, and download full-size photos
and videos. The terminal command is `gphotos`.

This project is not affiliated with or endorsed by Google.

## Requirements

- **macOS**, Google Chrome, and a signed-in Google Photos account.
- A current stable Rust toolchain (`cargo` and `rustc`). Install from [rustup.rs](https://rustup.rs/).
- Permission to read Chrome's cookies database and access **Chrome Safe Storage** in macOS Keychain.

Chrome import and secure session storage currently support macOS only. Other browsers
and Windows/Linux session storage are not implemented. No Google Cloud project,
OAuth client, Google password, subscription, or API key is required.

## Install

```bash
cargo install --locked --git https://github.com/andrewfurman/google-photos-CLI
```

From a local checkout:

```bash
cargo install --locked --path .
```

Cargo places `gphotos` in `~/.cargo/bin`. Add that directory to your `PATH` if needed.

## Quick start

1. Open [Google Photos](https://photos.google.com/) in Chrome and sign in.
2. Import your browser session:

   ```bash
   gphotos auth chrome
   ```

   macOS may ask whether `gphotos` can access **Chrome Safe Storage**. Allow access
   if you want to import that browser session. No browser cookie values are displayed.
   The CLI checks Google Photos access before saving the session into Keychain.

3. Search for a named person or keyword:

   ```bash
   gphotos search --person "Emma"
   gphotos search "birthday cake"
   gphotos search beach --person "Emma" --limit 10 --json
   ```

4. Copy a photo ID from a result and download the full-size file:

   ```bash
   gphotos download PHOTO_ID --dir ./photos
   gphotos download PHOTO_ID --output ./my-photo.jpg --json
   ```

Or search and download the displayed results in one command:

```bash
gphotos search "Paris 2025" --limit 10 --download ./photos
```

## Help and command reference

Help works offline and never accesses Keychain or Google Photos.

```bash
gphotos help
gphotos help search
gphotos help download
gphotos help auth chrome
gphotos auth cookies --help
```

| Command | Purpose |
| --- | --- |
| `gphotos auth chrome` | Import the selected Chrome profile's Google Photos session |
| `gphotos auth cookies FILE` | Import an explicitly exported Netscape `cookies.txt` file |
| `gphotos auth status [--json]` | Validate the saved session with Google Photos |
| `gphotos auth logout` | Remove the CLI session from Keychain |
| `gphotos logout` | Alias for `auth logout` |
| `gphotos search [QUERY] [--person NAME]` | Search Google Photos using its own text search |
| `gphotos download PHOTO_ID` | Save Google's full-size media file |

### Chrome profiles and multiple Google accounts

These are separate selections:

```bash
gphotos auth chrome --profile "Profile 1" --account 1
```

- `--profile` is Chrome's profile directory: `Default`, `Profile 1`, etc.
  Find the profile path at `chrome://version`.
- `--account` is Google's account index: the `N` in `photos.google.com/u/N/`.
  It defaults to `0`.
- There is one active saved CLI session. A successful import replaces it.

### Person and keyword search

Search forwards your query to Google Photos' private web search. It can match
people you have named in Google Photos, places, things, dates, and text in images.
Its behavior and result ranking belong to Google.

`--person "Emma"` adds that name before the keyword query. It is **not** an exact
face-ID filter and does not guarantee an intersection of identity and keywords.
For example, `gphotos search beach --person "Emma"` sends `Emma beach`.
The tool does not train a face-recognition model or maintain a local photo index.

Results include photo ID, original filename when available, capture date, media type,
dimensions, and a link to the photo. Filenames require one extra batch metadata call.
The default limit is 20; `--limit` accepts 1–1000.

```bash
gphotos search "birthday cake" --limit 100 --json
```

JSON includes `items`, `next_page_token`, `complete`, and `pages_fetched`.
`complete: false` means more results may be available, not that the search found
nothing else. To continue, repeat the **same query and account** with its token:

```bash
gphotos search "birthday cake" --limit 100 --json --page-token TOKEN
```

Tokens preserve an offset when a Google page contains more results than requested.
Each command stops after at most 50 Google search pages. Changing results between
commands can affect pagination; this is not a transactional snapshot.

### Downloads

```bash
gphotos download PHOTO_ID --dir ./photos
gphotos download PHOTO_ID --output ./exact-filename.jpg --json
```

Downloads prefer Google's **original** URL and otherwise use its **full-size** URL.
They do not substitute a thumbnail. JSON reports the `representation` (`original`
or `full-size`), saved absolute path, byte count, and SHA-256. A Storage Saver photo
can only be downloaded at the quality Google currently stores. Live Photo motion
components are not exported separately.

Files stream through a private temporary file next to the destination. Existing
files are never overwritten, including when a transfer fails. Empty or truncated
responses are rejected. Two photos with the same filename need different `--output`
paths; search-and-download reports collisions as failures rather than replacing files.

`search --download` retains successful downloads, reports individual failures, and
exits nonzero if any download failed. With `--json`, it adds `downloads` and
`download_errors` to the search result. Progress and errors go to stderr.

## Session lifecycle and privacy

The CLI imports only unexpired cookies with domains applicable to
`photos.google.com` (`google.com` and `photos.google.com`, including domain cookies).
It reads Chrome's cookie database **read-only**, decrypts locally using Chrome Safe
Storage, validates access, and stores the imported session in a macOS Keychain item:

- Service: `com.andrewfurman.google-photos-cli`
- Account: `session-v1`

It does not read saved browser passwords. Session cookies and page-derived request
tokens are never printed, committed, or placed in command arguments. There is no
telemetry. Search and download access Google directly; no project server receives
credentials or photos. Commands do not modify your Google Photos library.

The CLI fetches a fresh page-derived request token on connection and persists cookie
updates returned during commands. Chrome activity does **not** guarantee that an
already imported copy stays alive. If the session expires:

```bash
# Sign in / complete any verification at photos.google.com in Chrome, then:
gphotos auth chrome
gphotos auth status
```

There is no background browser keepalive or automatic Chrome cookie scraping.
To remove the saved CLI session without signing Chrome out:

```bash
gphotos logout
```

### Cookie-file fallback

If direct Chrome import is unavailable, an explicitly exported Netscape cookie file
can be imported:

```bash
gphotos auth cookies /path/to/cookies.txt --account 0
```

Cookie files grant account access. Keep them private, never attach them to an issue,
and remove them yourself after import if no longer needed. The CLI does not delete
the source file. Device-bound sessions can prevent reuse even when export succeeds;
this tool does not bypass those protections.

## Limitations and troubleshooting

- **Unofficial web API:** Google's official Photos Library API no longer provides
  general access to existing library items. This tool uses undocumented web RPCs,
  which can change without warning. Automated access may be restricted under
  [Google's terms](https://policies.google.com/terms); rate limits and account
  verification can interrupt use.
- **Chrome import failure:** Check the selected profile, Keychain permission, and
  Chrome's sign-in state. New encryption formats or session binding may prevent
  import. Error messages do not include cookies or raw Google responses.
- **Session expired:** Sign in in Chrome, then rerun `gphotos auth chrome`.
- **HTTP 429:** Stop and wait. The CLI reports numeric `Retry-After` guidance when
  present, spaces RPCs by at least 350 ms after the prior response, and does not
  automatically retry rate limits or downloads.
- **Missing matches:** Try the same query at photos.google.com. This CLI forwards
  text search, not a guaranteed structured people/date query language.
- **Existing file:** Choose another exact `--output` path. Nothing is overwritten.
- **Reinstall/update:** macOS may ask again for Keychain access after the binary changes.

## Development

```bash
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --locked --release
```

Tests use synthetic cookies and RPC data, not real accounts. CI runs offline tests
and builds on macOS. Live tests need your own explicit session import; never commit
cookie files, downloaded photos, or account response captures.

Protocol references and upstream attribution are in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
License: [MIT](LICENSE).
