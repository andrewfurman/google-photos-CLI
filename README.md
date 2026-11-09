# google-photos-CLI

An unofficial **Rust CLI for Google Photos**. Search your library the way you would in Google Photos,
inspect camera/location/people metadata, and download full-size photos and videos. The command is `gphotos`.

**macOS, Linux, and Windows** support portable session and cookie-file imports.
Direct import from Chrome's encrypted cookie database is currently macOS-only.
The project is not affiliated with or endorsed by Google.

## Install

Install a current stable Rust toolchain from [rustup.rs](https://rustup.rs/), then:

```bash
cargo install --locked --git https://github.com/andrewfurman/google-photos-CLI
```

Cargo places `gphotos` in `~/.cargo/bin` (`%USERPROFILE%\.cargo\bin` on Windows).
Add that directory to your PATH if needed. From a checkout:

```bash
cargo install --locked --path cli
```

## Authenticate once

Choose one method. No Google Cloud project, API key, or Google password is needed.

### Import Chrome on macOS

Sign in at [Google Photos](https://photos.google.com/) in Chrome, then:

```bash
gphotos auth chrome
```

macOS may ask for **Chrome Safe Storage** access to decrypt the browser cookies.
That permission is needed only during Chrome import, not for normal commands.
The validated session is saved in a private `.env` file.

For a different Chrome profile or Google account:

```bash
gphotos auth chrome --profile "Profile 1" --account 1
```

`--profile` is the Chrome profile directory shown at `chrome://version`.
`--account` is independently the `N` in `photos.google.com/u/N/`; it defaults to 0.

### Import browser cookies on any platform

Export your own Google Photos cookies in the **Netscape `cookies.txt` format**
using a browser tool you trust, then:

```bash
gphotos auth cookies /path/to/cookies.txt
```

This works on macOS, Linux, and Windows. Only unexpired cookies applicable to
Google Photos are imported. The source file is not deleted; remove it yourself
if no longer needed. Chrome/Google session protections may prevent reuse in some
cases; the CLI reports authentication failure rather than bypassing protections.

### Copy one session value to another machine

After authenticating on one machine:

```bash
gphotos auth export --output session.env
```

Copy that file securely to your other machine, install `gphotos`, and run:

```bash
gphotos auth import /path/to/session.env
gphotos auth status
```

The file has **one value**:

```dotenv
GPHOTOS_SESSION=gphotos_v1_REPLACE_WITH_YOUR_EXPORTED_SESSION
```

This is an encoded bundle of cookies and account selection, **not a permanent
Google API token**. Encoding is not encryption. Possession of the value grants
session access. Google may expire it, rotate cookies, or require verification
after a change of machine or IP address. A copied session is not guaranteed to
work forever or on every machine. The CLI fetches its current request token from
Google Photos when it connects.

For quick clipboard copying, `gphotos auth export --token` prints only the value:

```bash
# macOS; on Linux use wl-copy/xclip, or on Windows use clip/Set-Clipboard
gphotos auth export --token | pbcopy
```

A file containing just that value can also be imported with `gphotos auth import`.
Normal commands never print the value. Avoid sharing it in issues or shell history.

## Search, inspect, and download

Pass a single phrase to `gphotos search`, just like typing into the Google Photos
search bar. Use names, places, dates, objects, or a mix of them.

```bash
gphotos search "birthday cake"
gphotos search "Philadelphia October 2026"
gphotos search "Emma and Lucy at the beach"
```

Use a photo ID from the results to inspect its metadata or download it:

```bash
gphotos inspect PHOTO_ID
gphotos inspect PHOTO_ID --json
gphotos download PHOTO_ID --dir ./photos
gphotos download PHOTO_ID --output ./exact-filename.jpg --json

# Download each displayed search result
gphotos search "Paris 2025" --limit 10 --download ./photos
```

## Advanced search options

### Add text terms in scripts

Free-form query meaning and synonyms are Google's. The CLI does not geocode a
place name, expand "Philly" itself, or run its own image-content classifier.
For clearer scripts, these flags add text to the same Google query:

```bash
gphotos search --location "Philadelphia, Pennsylvania"
gphotos search --location Philly --tag "birthday cake"
```

`--tag` means a content/tag **search phrase**, not a separate Google custom-tag
API or a guaranteed exact object predicate. Repeated `--tag` terms are joined
with the other query text. `--location` is not a strict GPS boundary filter.

### Tagged people: require everyone in the same photo

`--person` resolves a unique full or partial name from Google's **People & Pets
face groups**. Repeat it for **AND**, not OR: every selected person must be tagged
in each returned photo. Example people below are placeholders; use the names
shown by `gphotos people` for your own library.

```bash
gphotos people --name Emma
gphotos search --person "Emma Example" --person "Lucy Example" --all --limit 500
gphotos search beach --person "Emma Example" --person "Lucy Example" --all
```

Ambiguous names produce an error instead of choosing someone silently. Use the
full name or exact IDs; repeated `--person-id` filters also require **all** IDs.
Duplicate constraints are removed. `--include-unnamed` on `people` lists unnamed
groups too. That command reports the groups Google returned, not a guaranteed
exhaustive inventory.

People-only searches start from a Google face-group result set. Multiple-person
searches verify the exact IDs in each photo's metadata. Person+keyword searches
verify the selected IDs on keyword results, since Google's combined filter is
unreliable. Metadata is fetched in read-only batches of at most 20 photos to make
long lists practical. Unknown people metadata is excluded and counted; no names
are inferred from image recognition, filenames, captions, or photo ownership.

### Explicit date ranges

Date phrases can be part of the browser-style query. For exact inclusive bounds:

```bash
gphotos search --from 2026-10-01 --to 2026-10-31 --all
gphotos search Philly --from 2026-01-01 --to 2026-12-31 --all
gphotos search --person "Emma Example" --person "Lucy Example" --from 2026-10-01 --to 2026-10-31 --all
```

Bounds must be valid `YYYY-MM-DD` dates, with `--from` on or before `--to`. The CLI
checks **capture dates in each photo's returned timezone**; it does not use upload
dates. A missing capture date is excluded and counted. Google Photos may group
very early-morning photos under the prior day in its UI; the explicit CLI bounds
use the returned calendar date.

When only dates and/or people are specified, the CLI reads Google's descending
capture-date timeline, positioned near `--to`, and verifies the selected dates and
people. When query/location/content text is present, Google supplies matching
photos and the CLI verifies the date bounds on those results. Dates and people
are strict checks; text/place/content interpretation remains Google's.

### Photo metadata

`gphotos inspect PHOTO_ID --json` returns the metadata exposed by the web API:

- Filename, caption (`caption`, also preserved as `description`), capture/upload date/time with timezone, dimensions, file size, and media type.
- Camera manufacturer/model, lens when available, focal length, aperture, ISO, and exposure time.
- Latitude/longitude and Google Photos' location labels when available.
- Tagged face groups, albums, upload source, favorites/archive flags, storage quality and usage.
- Video duration and Live Photo presence when returned.

Missing values remain `null`; unavailable fields are not guessed. These fields
are Google's metadata, not a promise to expose every possible EXIF/IPTC tag.
A location may have been estimated or edited in Google Photos.

### Pagination and long result lists

```bash
# Fetch a page, then use its next_page_token with the identical filters
gphotos search "birthday cake" --limit 100 --json
gphotos search "birthday cake" --limit 100 --json --page-token TOKEN

# Follow pages automatically; suitable for hundreds of matching photos
gphotos search --person "Emma Example" --person "Lucy Example" --all --limit 500 --json

# Allow a longer run, or continue an interrupted/limited bulk search
gphotos search --location Philly --all --limit 1000 --max-seconds 300 --json
gphotos search --location Philly --all --limit 1000 --max-seconds 300 --json --page-token TOKEN
```

Without `--all`, the default is 20 matches, at most 50 pages, and up to 100
candidates when local verification is needed. With `--all`, the default caps are
10,000 matches, 1,000 Google pages, and a 120-second **soft** time budget.
`--limit` accepts 1–10000; `--max-seconds` accepts 1–86400. Time is checked between
requests, so a slow in-flight request can overrun the budget.

See [Google Photos rate limits and timeouts](docs/RATE_LIMITS.md) for observed
server responses, CLI limits, and how to retry or resume.

Text searches default to `--sort relevant`. Use `--sort recent` for Google's
recent matching set. These are separate result sets, like the sections in Chrome.
`--all` raises scan limits with either sort. To reproduce the old text-search
behavior of `--all`, use `--sort recent --all`. People-only and date-only
searches use recent capture-date ordering.

JSON includes `items`, `people`, `person_match: "all"`, `ordering`, `date_range`,
`candidates_checked`, missing-metadata counts, `complete`, `stop_reason`,
`pages_fetched`, and `next_page_token`. The legacy single `person` field is
preserved when one person is selected. A stopped search returns its collected
matches; `complete: false` and a cursor indicate more may be available. Reasons
include `result_limit`, `candidate_limit`, `time_limit`, and `page_limit`.

The CLI prints the stop reason and resume flags to stderr. Continue with the same
query, people, dates, account, and sort. You can change `--all` or the limits when
resuming. For a cursor from an older text search with `--all`, use `--sort recent`. Tokens
preserve offsets within a page and cannot be reused for different filters.
Duplicate results are removed within each run. Google's changing library/search
results can affect pagination between commands; this is not a snapshot or a
promise to enumerate every possible match. Missing metadata may exclude photos
even when the selected source result set is completely scanned.

### Downloads

Downloads prefer Google's **original** URL, falling back to its **full-size** URL.
A thumbnail is never substituted. JSON reports `representation`, saved absolute
path, byte count, and SHA-256. Storage Saver media can only be downloaded at the
quality Google currently stores; Live Photo motion components are not exported
separately. Some videos have no direct full-size URL in the response; those
return an explicit download error.

Files stream through a private temporary file and existing files are never
overwritten. Empty/truncated transfers are rejected. Filename collisions need a
different `--output` path. `search --download` retains successes, reports each
failure, and exits nonzero if any failed. JSON adds `downloads` and
`download_errors`; progress/errors go to stderr.

## Session file and sample.env

The root [sample.env](sample.env) contains one placeholder setting:
`GPHOTOS_SESSION`. It does not contain a usable credential.

The default saved session file is:

| Platform | Location |
| --- | --- |
| macOS | `~/Library/Application Support/gphotos/.env` |
| Linux | `$XDG_CONFIG_HOME/gphotos/.env`, or `~/.config/gphotos/.env` |
| Windows | `%APPDATA%\gphotos\.env` |

Unix files are created with mode `0600`; Windows exports restrict the file ACL
to the current user. These files contain credentials in plaintext and should be
kept private. There is no telemetry or project backend; requests go directly to
Google. Session files, cookie files, account captures, and photos are never part
of the public repository.

To keep an ignored `.env` beside your checkout instead:

```bash
cp sample.env .env
gphotos --session-file .env auth chrome  # macOS
# Or: gphotos --session-file .env auth cookies /path/to/cookies.txt
gphotos --session-file .env search "birthday cake"
```

Use `--session-file PATH` for each command, or set `GPHOTOS_SESSION_FILE` once in
your shell. A local `.env` is not automatically discovered from arbitrary working
directories. `GPHOTOS_SESSION` can also be supplied directly as an environment
variable for ephemeral use; cookie updates cannot be written back to the parent
shell's environment variable.

### Session lifetime and refresh

There is **no reliable single lifetime** for `GPHOTOS_SESSION`. It packages cookies
with different expiry rules, and Google can expire or revoke access earlier than
any listed cookie expiry. It is not a permanent API token. This browser-session
method has no supported OAuth refresh token or guaranteed renewal endpoint.
`gphotos auth status` checks current usability rather than promising a future
expiry date.

When authentication fails:

1. Open Google Photos in your browser and sign in or complete Google's verification.
2. Import fresh cookies: `gphotos auth chrome` on macOS, or `gphotos auth cookies FILE` on any supported platform.
3. Run `gphotos auth status` to verify access.
4. Export a new file with `gphotos auth export --output new-session.env`; import it on the other machine with `gphotos auth import new-session.env`.

Normal file-based commands persist cookie updates. No browser keepalive runs in
the background. Browser activity does not guarantee an imported copy stays valid.
If it expires, sign in in your browser and import fresh cookies/session data.

```bash
gphotos auth status
gphotos logout
```

Logout clears the selected local session file; it does not revoke exported copies
or sign your browser out. Unset `GPHOTOS_SESSION` too if you supplied it in your shell.
For users upgrading from version 0.1, `gphotos auth migrate-keychain` performs a
one-time macOS Keychain import. Normal current commands do not use Keychain.

## Help

Help works offline and never reads session credentials.

```bash
gphotos help
gphotos help search
gphotos help inspect
gphotos help auth export
gphotos help auth import
```

| Command | Purpose |
| --- | --- |
| `auth chrome` | Import Chrome cookies on macOS |
| `auth cookies FILE` | Import Netscape cookies on any supported platform |
| `auth import FILE` | Import an exported `.env` or session-value file |
| `auth export --output FILE` | Export one portable value to a new private file |
| `auth export --token` | Explicitly print the copyable session value |
| `auth status [--json]` | Validate authentication without displaying credentials |
| `auth migrate-keychain` | Migrate an existing 0.1 session on macOS |
| `logout` / `auth logout` | Clear the selected local session file |
| `people [--name NAME]` | List Google's returned face groups |
| `search [QUERY]` | One search interface for text, repeated people, locations, content phrases, dates, and pagination |
| `inspect PHOTO_ID` | Read photo metadata and people tags |
| `download PHOTO_ID` | Save full-size media |

## Limitations and troubleshooting

This uses Google's undocumented web API because the official Library API does
not provide general reading/search access to an existing library. Google can
change it without notice. Automated access may be restricted under
[Google's terms](https://policies.google.com/terms).

- **Authentication expired:** sign in in your browser and import again. A portable bundle is not a permanent API token.
- **Chrome import fails:** check profile, macOS file permissions, and Chrome Safe Storage permission. Linux/Windows users should import a cookie file or exported session instead.
- **Person not found:** check `gphotos people --name NAME` and Google's face grouping. Only groups returned by Google can be resolved by name; use a known `--person-id` if necessary.
- **Metadata missing:** inspect reports null/unknown instead of inferring values.
- **HTTP 429:** wait before retrying; numeric Retry-After guidance is reported when present. RPCs are spaced at least 350 ms after the prior response. Rate limits/downloads are not automatically retried.
- **Destination exists:** choose another output filename; existing data is preserved.

## Repository, contributor, and agent instructions

Keep Rust source, `Cargo.toml`, and `Cargo.lock` under **`cli/`**, supporting
documentation under **`docs/`**, and setup and repository instructions in this
README. Cargo does not require its files at the repository root.

Changes after the initial bootstrap go through **branch → pull request → merge**;
do not push changes directly to `main`. Maintainers may merge their own PRs
immediately after relevant validation passes, with no mandatory waiting period.

Before merging, run these checks from the repository root:

```bash
cargo fmt --manifest-path cli/Cargo.toml --check
cargo clippy --manifest-path cli/Cargo.toml --locked --all-targets -- -D warnings
cargo test --manifest-path cli/Cargo.toml --locked
```

CI defines offline build/test jobs for Linux, Windows, and macOS.
See [Contributing](docs/CONTRIBUTING.md) for additional guidance and [Testing](docs/TESTING.md) for coverage and opt-in live verification.

- Use synthetic test data. Never commit session values, cookie files, account captures, real locations, face-group data, or downloaded media.
- Keep `sample.env` to one placeholder session value and keep populated `.env` files ignored.
- Do not infer tagged people from filenames, ownership, descriptions, or search keywords. Use Google's face-group IDs and per-photo face-group metadata.

License: [MIT](LICENSE). Upstream attribution is preserved in
[Third-party notices](docs/THIRD_PARTY_NOTICES.md).
