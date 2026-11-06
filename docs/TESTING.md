# Testing search behavior and metadata

From the repository root:

```bash
cargo fmt --manifest-path cli/Cargo.toml --check
cargo clippy --manifest-path cli/Cargo.toml --locked --all-targets -- -D warnings
cargo test --manifest-path cli/Cargo.toml --locked
```

CI runs synthetic offline tests on Linux, Windows, and macOS. No credentials,
Google account, location history, or real photographs are supplied to CI.

## Offline coverage

- Repeated people resolve to distinct group IDs, require AND rather than OR, and
  do not confuse identical labels or photo owners with matching face IDs.
- Ambiguous/missing names fail; duplicate person constraints are removed.
- Free-form browser queries are preserved; location aliases such as `Philly` and
  `Philadelphia, Pennsylvania` are forwarded, without local geocoding.
- Date bounds are validated and inclusive in the photo's returned timezone,
  including timezone boundary cases and missing timestamps.
- Pagination tokens preserve offsets and are bound to query/account/people/dates.
- Batched metadata is correlated by echoed request ID, including reordered and
  missing responses, to prevent assigning one photo's people to another photo.
- Inspection includes filename, caption/description, capture date/time, camera
  make/model/lens/settings, latitude/longitude, and named/unnamed face groups.
- Help documents the query tactics, long-list limits, cursor continuation,
  inspection fields, session lifetime, and platform-appropriate refresh commands.
- Session portability, file permissions, secret-free errors, and download
  truncation/path safeguards retain their regression coverage.

## Opt-in live verification

An ignored integration test validates results against **your own account**. It
requires an already authenticated CLI and known test inputs in your library.
Environment variables below contain test criteria, not session credentials:

```bash
export GPHOTOS_LIVE_PERSON_A='Alice Example'
export GPHOTOS_LIVE_PERSON_B='Bob Example'
export GPHOTOS_LIVE_KEYWORD='birthday cake'
export GPHOTOS_LIVE_LOCATION='Philadelphia, Pennsylvania'
export GPHOTOS_LIVE_FROM='2026-10-01'
export GPHOTOS_LIVE_TO='2026-10-31'
export GPHOTOS_LIVE_MIN_RESULTS='1'
cargo test --manifest-path cli/Cargo.toml --locked --test live -- --ignored --nocapture
```

Choose unique full names and criteria known to have matches. For bulk testing,
set the expected minimum to 100 if your library has at least that many photos
containing both people. The test:

1. Requests up to 100 photos containing both selected face groups.
2. Independently reads every returned photo's metadata and confirms **both** IDs.
3. Follows a continuation token and checks for overlap with the earlier page.
4. Checks known keyword and location queries return results.
5. Checks a known date range and verifies every returned capture date.
6. Confirms inspection exposes the expected metadata fields (null is allowed for
   values Google does not provide).

The test saves returned session-cookie updates even on a handled failure. It
prints counts/status only. Never commit credentials, live captures, real face IDs,
GPS positions, captions, or images. Search meaning and completeness remain
Google's; these checks verify the CLI's filtering and paging, not the accuracy
of Google's image classifier or place recognition.
