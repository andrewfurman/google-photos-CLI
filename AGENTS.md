# Repository workflow

- Keep code in `cli/`, supporting documentation in `docs/`, and setup instructions in the root README.
- Make changes on a branch and open a pull request; do not push changes directly to `main`.
- Run `cargo fmt --manifest-path cli/Cargo.toml --check`, `cargo clippy --manifest-path cli/Cargo.toml --locked --all-targets -- -D warnings`, and `cargo test --manifest-path cli/Cargo.toml --locked` before merging.
- Maintainers may merge their own pull requests immediately after relevant validation passes. There is no mandatory waiting period.
- Tests use synthetic data. Never commit session values, cookie files, account captures, locations, face-group data, or downloaded media.
- Keep `sample.env` to one placeholder session value, and keep populated `.env` files ignored.
- Do not infer tagged people from filenames, ownership, descriptions, or search keywords. Use Google's face-group IDs and per-photo face-group metadata.
