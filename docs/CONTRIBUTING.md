# Contributing

Create a branch, make a focused change, and open a pull request with a description
of the resulting behavior and relevant validation. All changes after the initial
repository bootstrap go through pull requests before reaching `main`. Maintainers
may merge their own PRs immediately after relevant validation passes.

The Rust crate is in `cli/`. From the repository root:

```bash
cargo fmt --manifest-path cli/Cargo.toml --check
cargo clippy --manifest-path cli/Cargo.toml --locked --all-targets -- -D warnings
cargo test --manifest-path cli/Cargo.toml --locked
cargo build --manifest-path cli/Cargo.toml --locked --release
```

CI defines jobs for Linux, Windows, and macOS. Direct Chrome-cookie decryption
is currently implemented only for macOS. Portable session and Netscape-cookie
imports, search, inspection, and downloads have no Keychain dependency.

Include only synthetic fixtures and examples. Never publish real credentials,
face-group IDs, GPS locations, Google responses, or downloaded photos. A successful
search alone does not prove correct tagging: validate the returned per-photo group
IDs when changing structured people filters.

The project is MIT licensed. Preserve upstream attribution in
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) when adapting protocol mappings.
