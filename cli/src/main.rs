use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use google_photos_cli::{
    auth::{self, Session},
    client::{self, PhotosClient, SearchOptions, SearchSort},
};
use std::path::PathBuf;

const EXAMPLES: &str = "GET STARTED:
  macOS Chrome:   gphotos auth chrome
  Any platform:   gphotos auth cookies /path/to/cookies.txt
  Other machine:  gphotos auth import /path/to/session.env

SEARCH LIKE GOOGLE PHOTOS:
  gphotos search \"birthday cake\"
  gphotos search \"Philadelphia October 2026\"
  gphotos search \"Emma and Lucy at the beach\"

Enter one phrase with names, places, dates, or objects. Google interprets it
like a search in the browser. Run 'gphotos help search' for more options.

ADVANCED SEARCH OPTIONS:
  gphotos search --person \"Emma Example\" --person \"Lucy Example\"
  gphotos search --from 2026-10-01 --to 2026-10-31
  gphotos search \"birthday cake\" --all --limit 100 --json

PHOTO METADATA AND DOWNLOAD:
  gphotos inspect PHOTO_ID --json
  gphotos download PHOTO_ID --dir ./photos
  gphotos help inspect
  gphotos help search

PORTABLE AUTHENTICATION:
  gphotos auth export --output session.env
  gphotos auth import session.env
  gphotos auth status
  gphotos help auth

One GPHOTOS_SESSION value stores the cookie bundle in a private .env file.
Normal commands never use Keychain. This is not a permanent Google API token:
there is no reliable single lifetime or supported OAuth refresh token. Google
can expire/revoke it before individual cookies expire. Browser use does not
ensure the imported session stays valid. Sign in in your browser and reimport
fresh cookies/session data when authentication fails; export a fresh copy for
other machines. Use --session-file PATH to select a different private file.

Text queries, locations, content/tag phrases and free-form dates go to Google's
search. Explicit --from/--to bounds are verified against capture-date metadata.
Repeated --person filters require ALL selected face-group IDs in the same photo.
--sort selects relevant or recent results. --all raises paging and scan limits.
Partial results have complete=false, stop_reason, and next_page_token.

Unofficial project; not affiliated with Google. Read commands never modify
Google Photos. Run 'gphotos help COMMAND' for full details.";

#[derive(Parser)]
#[command(name="gphotos", version, about="Search and download your Google Photos from the terminal", long_about="Unofficial Google Photos CLI. Reuse your Chrome session, search your existing library, and download full-size media. Portable cookie/session import supports macOS, Linux, and Windows. Direct Chrome import currently supports macOS.", after_help=EXAMPLES)]
struct Cli {
    /// Session .env path (default: your platform's config directory/gphotos/.env)
    #[arg(long, global = true, env = "GPHOTOS_SESSION_FILE")]
    session_file: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Import, check, refresh, or remove the locally saved session
    #[command(
        after_help = "SESSION LIFETIME AND REFRESH:\n  gphotos auth status\n\nNo single guaranteed expiry exists: GPHOTOS_SESSION packages several cookies.\nGoogle can revoke it before their expiry dates. There is no supported OAuth\nrefresh token for this method. Normal commands persist returned cookie updates\nto the selected .env file, but cannot renew a revoked Google login.\n\nWhen authentication fails:\n  1. Sign in / complete any verification at photos.google.com in your browser.\n  2. macOS: gphotos auth chrome; any OS: gphotos auth cookies /path/to/cookies.txt\n  3. gphotos auth status\n  4. gphotos auth export --output new-session.env\n     On another machine: gphotos auth import new-session.env\n\nExport destinations must be new files; existing exports are not overwritten."
    )]
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
    },
    /// Search Google's text, people, places, dates, and content; paginate long lists
    #[command(after_help = r#"SEARCH LIKE GOOGLE PHOTOS:
  gphotos search "birthday cake"
  gphotos search "Philadelphia October 2026"
  gphotos search "Emma and Lucy at the beach"

Enter one phrase with names, places, dates, or objects. Google interprets it
like a search in the browser. --location and --tag add text to that same query.

ADVANCED SEARCH OPTIONS:
  gphotos search --person "Emma Example" --person "Lucy Example"
  gphotos search --from 2026-10-01 --to 2026-10-31
  gphotos search beach --person "Emma Example" --from 2026-10-01

Repeat --person and/or --person-id to require every selected face group in the
same photo. Names must resolve uniquely. Use full names to disambiguate them.
Multiple people and person+keyword searches verify exact per-photo face-group
IDs. Photos with unknown tags are excluded.

--from and --to enforce inclusive YYYY-MM-DD capture dates in each photo's
returned timezone. Photos with unknown capture dates are excluded. Date-only
searches use Google's chronological library, positioned at --to.

PAGINATION AND OUTPUT:
  gphotos search "birthday cake" --sort recent --all --limit 100 --json
  gphotos search "birthday cake" --limit 100 --json --page-token TOKEN

Default: 20 matches, up to 100 candidates when local verification is needed.
--all follows pages automatically: default cap 10000 matches, 1000 pages, 120s
search budget. --limit and --max-seconds override caps. --sort relevant (default
for text) and --sort recent select separate Google result sets. People-only and
date-only searches use recent order. --all works with either sort.

An incomplete search reports why it stopped and how to resume. Keep the same
query, filters, account, and sort when using --page-token. You can change scan
limits, including --all. JSON includes ordering, people, person_match,
candidates_checked, complete, stop_reason, and next_page_token.
--download DIRECTORY downloads displayed matches without changing the library."#)]
    Search(SearchArgs),
    /// List Google's returned face groups and their IDs (named groups by default)
    People {
        /// Filter names by case-insensitive substring
        #[arg(long)]
        name: Option<String>,
        /// Include unnamed face groups
        #[arg(long)]
        include_unnamed: bool,
        #[arg(long)]
        json: bool,
    },
    /// Show full available photo metadata: filename, camera/lens, coordinates, dates, caption, people
    #[command(
        alias = "get",
        after_help = "EXAMPLES:\n  gphotos inspect PHOTO_ID\n  gphotos inspect PHOTO_ID --json\n  gphotos get PHOTO_ID --json\n\nJSON includes filename, caption (also description), taken_at and uploaded_at\nwith timezone, dimensions, file size, camera make/model, lens when supplied,\nfocal length, aperture, ISO, exposure time, latitude/longitude, location labels,\nGoogle-tagged people, albums, source and storage attributes. Missing fields\nremain null. people_status=not_returned does not mean nobody is in the image.\nThis is Google's available metadata, not a promise of every possible EXIF tag."
    )]
    Inspect {
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// Download full-size media by the ID returned from search
    #[command(
        after_help = "EXAMPLES:\n  gphotos download PHOTO_ID --dir ./photos\n  gphotos download PHOTO_ID --output ./vacation.jpg --json\n\nPrefers Google's original download URL; otherwise uses its full-size URL.\nJSON reports 'original' or 'full-size', byte count, SHA-256, and saved path.\nNever downloads a thumbnail or overwrites an existing file.\nGoogle's stored version may differ from the file originally uploaded if\nStorage Saver was used. Live Photo motion components are not exported separately."
    )]
    Download(DownloadArgs),
    /// Remove the CLI's saved session (does not sign Chrome out)
    Logout,
}
#[derive(Subcommand)]
enum AuthCommand {
    /// Import Chrome cookies on macOS; save a portable session .env
    #[command(
        after_help = "EXAMPLES:\n  gphotos auth chrome\n  gphotos auth chrome --profile \"Profile 1\" --account 1\n\nSign in at photos.google.com in Chrome first. Choose the Chrome profile\ndirectory (Default, Profile 1, etc.). --account is the N in /u/N/ when a\nprofile has multiple Google accounts; it is independent of --profile.\nmacOS may ask you to allow access to Chrome Safe Storage.\nThe CLI validates Google Photos access before saving its private .env file.\nRe-run this command to refresh an expired session. If Chrome's encryption\nor device-bound sessions prevent reuse, cookie import may also fail; the\nCLI reports the failure rather than trying to bypass browser protections."
    )]
    Chrome {
        /// Chrome profile directory, e.g. Default or 'Profile 1'
        #[arg(long, default_value = "Default")]
        profile: String,
        /// Google account index within that browser profile (/u/N/)
        #[arg(long, default_value_t = 0)]
        account: u32,
    },
    /// Import an explicitly exported Netscape cookies.txt file
    #[command(
        after_help = "EXAMPLE:\n  gphotos auth cookies /path/to/cookies.txt --account 0\n\nImports only Google Photos-compatible domains and unexpired cookies.\nThe source file is not deleted. Treat it as a password; don't commit or share\nit. The CLI saves the validated session in a private .env file on any platform."
    )]
    Cookies {
        file: PathBuf,
        #[arg(long, default_value_t = 0)]
        account: u32,
    },
    /// Import an exported session .env file or a file containing just its token
    Import { file: PathBuf },
    /// Export one portable session value (explicitly outputs credentials)
    #[command(group=clap::ArgGroup::new("destination").required(true).args(["output","token"]))]
    Export {
        /// Write GPHOTOS_SESSION=... to a new private .env file
        #[arg(long, conflicts_with = "token")]
        output: Option<PathBuf>,
        /// Print just the copyable session value; pipe directly to a clipboard
        #[arg(long)]
        token: bool,
    },
    /// One-time migration of a 0.1 macOS Keychain session to portable .env storage
    MigrateKeychain,
    /// Validate the saved session; display its file path, never its value
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Remove the session from the selected local .env file
    Logout,
}
#[derive(Args)]
struct SearchArgs {
    /// A Google Photos search query (quote multiple words)
    query: Option<String>,
    /// Named tagged person (unique full or partial face-group name)
    #[arg(long, help_heading = "Advanced filters")]
    person: Vec<String>,
    /// Exact face-group ID from 'gphotos people'
    #[arg(long, help_heading = "Advanced filters")]
    person_id: Vec<String>,
    /// Location phrase sent unchanged to Google's search (e.g. Philly)
    #[arg(long)]
    location: Option<String>,
    /// Content/tag phrase sent to Google's search; repeat to add terms
    #[arg(long)]
    tag: Vec<String>,
    /// Inclusive capture-date lower bound, YYYY-MM-DD
    #[arg(long, help_heading = "Advanced filters")]
    from: Option<String>,
    /// Inclusive capture-date upper bound, YYYY-MM-DD
    #[arg(long, help_heading = "Advanced filters")]
    to: Option<String>,
    /// Select the Google result set (default: relevant for text, recent otherwise)
    #[arg(long, value_enum)]
    sort: Option<SearchSort>,
    /// Follow Google result pages automatically (default cap: 10000 matches)
    #[arg(long)]
    all: bool,
    /// Search time budget including retries; partial results include a cursor
    #[arg(long,default_value_t=120,value_parser=clap::value_parser!(u64).range(1..=86400))]
    max_seconds: u64,
    /// Maximum matches (default 20, or 10000 with --all)
    #[arg(long,value_parser=clap::value_parser!(u32).range(1..=10000))]
    limit: Option<u32>,
    /// Continue the same search using next_page_token from JSON output
    #[arg(long)]
    page_token: Option<String>,
    /// Download each displayed result into this directory
    #[arg(long, value_name = "DIRECTORY")]
    download: Option<PathBuf>,
    /// Print machine-readable JSON (progress and errors go to stderr)
    #[arg(long)]
    json: bool,
}
#[derive(Args)]
struct DownloadArgs {
    /// Photo/media ID from gphotos search (not the filename or URL)
    id: String,
    /// Exact output filename; existing files are never overwritten
    #[arg(short, long, conflicts_with = "dir")]
    output: Option<PathBuf>,
    /// Directory for the original filename (defaults to current directory)
    #[arg(long)]
    dir: Option<PathBuf>,
    #[arg(long)]
    json: bool,
}
fn print_json(value: &impl serde::Serialize) -> Result<()> {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    serde_json::to_writer_pretty(&mut out, value)?;
    writeln!(out)?;
    Ok(())
}
fn with_saved<T>(run: impl FnOnce(&mut PhotosClient) -> Result<T>) -> Result<T> {
    let mut client = PhotosClient::connect(auth::load()?)?;
    let result = run(&mut client);
    // Even a failed search may have rotated session cookies.
    let saved = client.save();
    if result.is_err() {
        if saved.is_err() {
            eprintln!("Warning: could not save updated session in the private .env file");
        }
        result
    } else {
        saved.context(
            "Command finished, but updated session could not be saved in the private .env file",
        )?;
        result
    }
}
fn run(cli: Cli) -> Result<()> {
    if let Some(path) = cli.session_file {
        std::env::set_var("GPHOTOS_SESSION_FILE", path);
    }
    match cli.command {
        Command::Auth { command } => {
            match command {
                AuthCommand::Chrome { profile, account } => {
                    eprintln!("Importing Chrome session; macOS may prompt for Chrome Safe Storage access…");
                    let store = auth::import_chrome(&profile)?;
                    let session = Session::new(&store, account, Some(profile))?;
                    let client = PhotosClient::connect(session)?;
                    client.save()?;
                    println!(
                        "Google Photos session saved to {}. Account: {} (index {}).",
                        auth::session_path()?.display(),
                        client.account_email().unwrap_or("signed in"),
                        client.account_index()
                    );
                }
                AuthCommand::Cookies { file, account } => {
                    let session = Session::new(&auth::import_netscape(&file)?, account, None)?;
                    let client = PhotosClient::connect(session)?;
                    client.save()?;
                    println!(
                        "Google Photos session saved to {}.",
                        auth::session_path()?.display()
                    );
                }
                AuthCommand::Import { file } => {
                    let client = PhotosClient::connect(Session::import_file(&file)?)?;
                    client.save()?;
                    println!(
                        "Session imported and verified. Saved to {}.",
                        auth::session_path()?.display()
                    );
                }
                AuthCommand::MigrateKeychain => {
                    let client = PhotosClient::connect(auth::import_legacy_keychain()?)?;
                    client.save()?;
                    println!(
                        "Session migrated to {}. Normal commands no longer use Keychain.",
                        auth::session_path()?.display()
                    );
                }
                AuthCommand::Export { output, token } => {
                    let session = auth::load()?;
                    if token {
                        println!("{}", session.token()?);
                    } else if let Some(path) = output {
                        session.export(&path)?;
                        println!("Private session exported to {}. Copy it securely to the other machine and run 'gphotos auth import FILE'.",path.display());
                    }
                }
                AuthCommand::Status { json } => with_saved(|client| {
                    if json {
                        print_json(
                            &serde_json::json!({"authenticated":true,"account":client.account_email(),"account_index":client.account_index(),"storage":if std::env::var_os("GPHOTOS_SESSION").is_some(){"environment"}else{"session_env_file"},"session_file":if std::env::var_os("GPHOTOS_SESSION").is_some(){None}else{Some(auth::session_path()?)}}),
                        )?;
                    } else {
                        println!(
                            "Authenticated: {} (account index {}). Session file: {}.",
                            client.account_email().unwrap_or("Google Photos"),
                            client.account_index(),
                            auth::session_path()?.display()
                        );
                    }
                    Ok(())
                })?,
                AuthCommand::Logout => {
                    auth::logout()?;
                    println!("Local session file cleared. Chrome and exported copies remain signed in. Unset GPHOTOS_SESSION too if you supplied it in your shell.");
                }
            }
        }
        Command::Logout => {
            auth::logout()?;
            println!("Local session file cleared. Chrome and exported copies remain signed in. Unset GPHOTOS_SESSION too if you supplied it in your shell.");
        }
        Command::Search(args) => {
            let dates = google_photos_cli::search::DateRange::parse(
                args.from.as_deref(),
                args.to.as_deref(),
            )?;
            let query = google_photos_cli::search::compose_query(
                args.query.as_deref(),
                args.location.as_deref(),
                &args.tag,
            );
            if query.is_empty()
                && args.person.is_empty()
                && args.person_id.is_empty()
                && !dates.active()
            {
                bail!("Provide a search query, --person NAME, --location PLACE, --tag TERM, or --from/--to dates. See 'gphotos help search'");
            }
            let count = args.person.len() + args.person_id.len();
            if count > 1 || (count > 0 && (!query.is_empty() || dates.active())) {
                eprintln!("Verifying that every selected person is tagged in each result…");
            }
            with_saved(|client| {
                let result = client.search_people(
                    &query,
                    &args.person,
                    &args.person_id,
                    SearchOptions {
                        limit: args.limit.unwrap_or(if args.all { 10000 } else { 20 }) as usize,
                        all: args.all,
                        sort: args.sort,
                        max_seconds: args.max_seconds,
                        dates,
                    },
                    args.page_token.as_deref(),
                )?;
                if let Some(token) = &result.next_page_token {
                    let reason = match result.stop_reason.as_str() {
                        "result_limit" => "reached the result limit",
                        "candidate_limit" => "reached the candidate scan limit",
                        "time_limit" => "reached the time budget",
                        "page_limit" => "reached the page limit",
                        other => other,
                    };
                    eprintln!(
                        "Search incomplete: {reason} after checking {} candidate(s).",
                        result.candidates_checked
                    );
                    eprintln!(
                        "Resume with the same query and filters. Keep sort order ({}), and set --page-token {token}.",
                        result.ordering
                    );
                    if result.stop_reason == "candidate_limit" {
                        eprintln!("Add --all to scan beyond the default candidate limit.");
                    }
                }
                let mut downloads = Vec::new();
                let mut failures = Vec::new();
                if let Some(dir) = &args.download {
                    for item in &result.items {
                        eprintln!(
                            "Downloading {}…",
                            item.filename.as_deref().unwrap_or(&item.id)
                        );
                        match client.download(&item.id, None, dir) {
                            Ok(saved) => downloads.push(saved),
                            Err(e) => failures
                                .push(serde_json::json!({"id":item.id,"error":format!("{e:#}")})),
                        }
                    }
                }
                if args.json {
                    let mut value = serde_json::to_value(&result)?;
                    if args.download.is_some() {
                        value["downloads"] = serde_json::to_value(&downloads)?;
                        value["download_errors"] = serde_json::to_value(&failures)?;
                    }
                    print_json(&value)?;
                } else {
                    println!("{} result(s) for {:?}", result.items.len(), result.query);
                    if !result.people.is_empty() {
                        println!(
                            "Tagged people (ALL required): {}",
                            result
                                .people
                                .iter()
                                .map(|p| p.name.as_deref().unwrap_or(&p.id))
                                .collect::<Vec<_>>()
                                .join(" AND ")
                        );
                    }
                    println!("Order: {}", result.ordering);
                    for item in &result.items {
                        println!(
                            "{}  {}  {}  {}",
                            item.id,
                            item.taken_at.as_deref().unwrap_or("date unknown"),
                            item.media_type,
                            item.filename.as_deref().unwrap_or("filename unavailable")
                        );
                    }
                    if result.people_metadata_unavailable > 0 {
                        eprintln!(
                            "Skipped {} result(s) whose people metadata was unavailable.",
                            result.people_metadata_unavailable
                        );
                    }
                    if result.date_metadata_unavailable > 0 {
                        eprintln!(
                            "Skipped {} result(s) with unavailable capture dates.",
                            result.date_metadata_unavailable
                        );
                    }
                    for saved in &downloads {
                        println!(
                            "Saved {} ({} bytes, {})",
                            saved.path.display(),
                            saved.bytes,
                            saved.representation
                        );
                    }
                    for error in &failures {
                        eprintln!("Download failed for {}: {}", error["id"], error["error"]);
                    }
                }
                if !failures.is_empty() {
                    bail!(
                        "{} download(s) failed; successful downloads were retained",
                        failures.len()
                    );
                }
                Ok(())
            })?;
        }
        Command::People {
            name,
            include_unnamed,
            json,
        } => with_saved(|client| {
            let people = client.people()?;
            let returned = people.len();
            let people = people
                .into_iter()
                .filter(|p| include_unnamed || p.name.is_some())
                .filter(|p| {
                    name.as_deref().is_none_or(|q| {
                        p.name
                            .as_deref()
                            .is_some_and(|n| n.to_lowercase().contains(&q.to_lowercase()))
                    })
                })
                .collect::<Vec<_>>();
            if json {
                print_json(
                    &serde_json::json!({"people":people,"groups_returned_by_google":returned,"coverage":"groups_returned_by_google"}),
                )?;
            } else {
                for p in people {
                    println!("{}  {}", p.id, p.name.as_deref().unwrap_or("unnamed"));
                }
            }
            Ok(())
        })?,
        Command::Inspect { id, json } => {
            client::check_id(&id)?;
            with_saved(|client| {
                let info = client.inspect(&id)?;
                if json {
                    print_json(&info)?;
                } else {
                    print_inspection(&info);
                }
                Ok(())
            })?;
        }
        Command::Download(args) => {
            client::check_id(&args.id)?;
            with_saved(|client| {
                let result = client.download(
                    &args.id,
                    args.output.as_deref(),
                    args.dir.as_deref().unwrap_or(std::path::Path::new(".")),
                )?;
                if args.json {
                    print_json(&result)?;
                } else {
                    println!(
                        "Saved {} ({} bytes, {}).",
                        result.path.display(),
                        result.bytes,
                        result.representation
                    );
                }
                Ok(())
            })?;
        }
    }
    Ok(())
}
fn print_inspection(info: &google_photos_cli::protocol::Inspection) {
    println!("Photo: {}", info.id);
    println!(
        "Filename: {}",
        info.filename.as_deref().unwrap_or("unknown")
    );
    println!("Taken: {}", info.taken_at.as_deref().unwrap_or("unknown"));
    println!(
        "Uploaded: {}",
        info.uploaded_at.as_deref().unwrap_or("unknown")
    );
    println!(
        "Dimensions: {} × {}",
        info.width
            .map(|v| v.to_string())
            .unwrap_or_else(|| "unknown".into()),
        info.height
            .map(|v| v.to_string())
            .unwrap_or_else(|| "unknown".into())
    );
    println!(
        "File size: {}",
        info.size_bytes
            .map(|v| format!("{v} bytes"))
            .unwrap_or_else(|| "unknown".into())
    );
    if let Some(camera) = &info.camera {
        println!(
            "Camera: {} {}",
            camera.make.as_deref().unwrap_or(""),
            camera.model.as_deref().unwrap_or("")
        );
        println!(
            "Camera settings: {}",
            serde_json::to_string(camera).unwrap_or_default()
        );
    } else {
        println!("Camera: unknown");
    }
    if let Some(location) = &info.location {
        println!("Location: {}", location.labels.join(", "));
        if let (Some(lat), Some(lon)) = (location.latitude, location.longitude) {
            println!("Coordinates: {lat}, {lon}");
        }
    } else {
        println!("Location: unknown");
    }
    if info.people_status == "available" {
        println!(
            "Tagged people: {}",
            if info.people.is_empty() {
                "none returned".into()
            } else {
                info.people
                    .iter()
                    .map(|p| {
                        format!(
                            "{} ({})",
                            p.name.as_deref().unwrap_or("unnamed face group"),
                            p.id
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        );
    } else {
        println!("Tagged people: unavailable (Google returned no face-group metadata)");
    }
    if let Some(description) = &info.description {
        println!("Caption: {description}");
    }
    if !info.albums.is_empty() {
        println!(
            "Albums: {}",
            info.albums
                .iter()
                .map(|a| a.title.as_deref().unwrap_or("untitled"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    println!("Source: {}", info.source.join(", "));
    println!(
        "Storage quality: {}",
        info.storage_quality.as_deref().unwrap_or("unknown")
    );
    println!("Link: {}", info.url);
}
fn main() {
    if let Err(err) = run(Cli::parse()) {
        eprintln!("Error: {err:#}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod argument_tests {
    use super::*;
    #[test]
    fn one_search_command_accepts_all_search_tactics_and_long_list_controls() {
        let cli = Cli::try_parse_from([
            "gphotos",
            "search",
            "birthday",
            "--person",
            "Alice Example",
            "--person",
            "Bob Example",
            "--location",
            "Philly",
            "--tag",
            "cake",
            "--from",
            "2026-01-01",
            "--to",
            "2026-12-31",
            "--all",
            "--limit",
            "500",
            "--max-seconds",
            "300",
            "--json",
        ])
        .unwrap();
        let Command::Search(args) = cli.command else {
            panic!("wrong command")
        };
        assert_eq!(args.person, vec!["Alice Example", "Bob Example"]);
        assert_eq!(args.location.as_deref(), Some("Philly"));
        assert_eq!(args.tag, vec!["cake"]);
        assert!(args.all);
        assert_eq!(args.limit, Some(500));
        assert_eq!(args.max_seconds, 300);
    }
    #[test]
    fn free_form_browser_query_is_a_single_unchanged_argument() {
        let query = "Alice and Bob in Philadelphia October 2026";
        let cli = Cli::try_parse_from(["gphotos", "search", query]).unwrap();
        let Command::Search(args) = cli.command else {
            panic!("wrong command")
        };
        assert_eq!(args.query.as_deref(), Some(query));
    }
}
