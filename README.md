# google-photos-CLI

An unofficial **Rust CLI for Google Photos**. Search your own library by keyword
or tagged person, inspect camera/location/people metadata, and download full-size
photos and videos. The command is `gphotos`.

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

```bash
# Keywords, objects, places, or text in images
gphotos search "birthday cake" --limit 20

# Discover named people and their face-group IDs
gphotos people --name Emma

# Photos tagged with a particular person
gphotos search --person Emma
gphotos search --person Lucy

# Combine a keyword with the structured person filter
gphotos search beach --person Emma --json

# Camera, GPS/location, dates, file information, and tagged people
gphotos inspect PHOTO_ID
gphotos inspect PHOTO_ID --json

# Full-size media by an ID returned from search
gphotos download PHOTO_ID --dir ./photos
gphotos download PHOTO_ID --output ./exact-filename.jpg --json

# Download each displayed search result
gphotos search "Paris 2025" --limit 10 --download ./photos
```

### Tagged people

`--person` resolves the name against Google's returned **People & Pets face
groups**, then searches using the selected **face-group ID**. It does not simply
add the name to a keyword query. A unique full or partial name works. If names
are ambiguous, use the full name or an explicit ID:

```bash
gphotos people --name Alex --json
gphotos search --person-id FACE_GROUP_ID
```

`gphotos people` shows named groups; `--include-unnamed` includes unnamed groups.
It reports the groups Google returned, not a guaranteed exhaustive inventory.
A missing or ambiguous name produces an error instead of falling back to text
search. Face grouping must be available in your account.

`inspect` reads the per-photo face-group metadata. Each returned tag includes its
ID and name when assigned. Unnamed groups retain their IDs. A photo owner, an
album member, a filename, or a search term is never treated as a person tag.
`people_status: "not_returned"` means Google did not supply the field; it does
not imply no people appear in the image.

### Photo metadata

`gphotos inspect PHOTO_ID --json` returns the metadata exposed by the web API:

- Filename, description, capture/upload time, dimensions, file size, and media type.
- Camera manufacturer/model, lens when available, focal length, aperture, ISO, and exposure time.
- Latitude/longitude and Google Photos' location labels when available.
- Tagged face groups, albums, upload source, favorites/archive flags, storage quality and usage.
- Video duration and Live Photo presence when returned.

Missing values remain `null`; unavailable fields are not guessed. These fields
are Google's metadata, not a promise to expose every possible EXIF/IPTC tag.
A location may have been estimated or edited in Google Photos.

### Search results and pagination

Keyword searches use Google's most-relevant result set. People-only searches use
Google's recent ordering. JSON identifies the selected `ordering`; `complete` refers
to that result set, not every possible match in your library. Person filtering uses
the structured face-group filter. When combined with a keyword, the CLI instead
checks each keyword result's per-photo tags for the exact selected group ID,
because Google's combined filter is unreliable. It checks up to 100 candidates
per command; use `next_page_token` to continue. Missing people metadata is excluded
and counted in `people_metadata_unavailable`, rather than treated as a match.

Results include ID, filename, date, type, dimensions, and a photo link.
The default limit is 20; `--limit` accepts 1–1000.

JSON includes `items`, `person`, `ordering`, `next_page_token`, `complete`, and `pages_fetched`.
`complete: false` means more results may be available. To continue, use the same
query, person, and account with the returned token:

```bash
gphotos search --person Emma --limit 100 --json --page-token TOKEN
```

Each command stops after at most 50 Google search pages. Tokens preserve offsets
within a page. Changing results between commands can affect pagination; this is
not a transactional snapshot.

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
one-time macOS Keychain import. Normal version 0.2 commands do not use Keychain.

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
| `search [QUERY] [--person NAME]` | Search keywords and/or tagged people |
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
See [Contributing](docs/CONTRIBUTING.md) for additional guidance.

- Use synthetic test data. Never commit session values, cookie files, account captures, real locations, face-group data, or downloaded media.
- Keep `sample.env` to one placeholder session value and keep populated `.env` files ignored.
- Do not infer tagged people from filenames, ownership, descriptions, or search keywords. Use Google's face-group IDs and per-photo face-group metadata.

License: [MIT](LICENSE). Upstream attribution is preserved in
[Third-party notices](docs/THIRD_PARTY_NOTICES.md).
