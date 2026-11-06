use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use google_photos_cli::{
    auth::{self, Session},
    client::{self, PhotosClient},
};
use std::path::PathBuf;

const EXAMPLES: &str = "GET STARTED (macOS):
  1. Sign in at https://photos.google.com in Google Chrome.
  2. gphotos auth chrome
  3. gphotos search --person \"Emma\"
  4. gphotos search \"birthday cake\" --json
  5. gphotos download PHOTO_ID --dir ./photos

EXAMPLES:
  gphotos search beach --person \"Emma\" --limit 10
  gphotos search \"Paris 2025\" --download ./photos
  gphotos download PHOTO_ID --output ./original.jpg
  gphotos auth status
  gphotos auth chrome --profile \"Profile 1\" --account 1
  gphotos help search
  gphotos help auth chrome

AUTHENTICATION:
  Imports only cookies applicable to Google Photos. macOS may ask for access
  to Chrome Safe Storage. Session cookies are stored in your macOS Keychain;
  your Google password is never requested. When access expires, sign in with
  Chrome and rerun 'gphotos auth chrome'. Browser activity does not guarantee
  that a previously imported session stays valid.

SEARCH:
  Uses Google Photos' own web search: named people, places, things, dates,
  and text in images. --person adds a name to that query; it is not an exact
  face-ID filter. Results match Google Photos' interpretation of the query.

Unofficial project, not affiliated with Google. Uses an undocumented web API
that Google may change. Read commands never modify your Google Photos library.";

#[derive(Parser)]
#[command(name="gphotos", version, about="Search and download your Google Photos from the terminal", long_about="Unofficial Google Photos CLI. Reuse your Chrome session, search your existing library, and download full-size media. Chrome import and secure session storage currently require macOS.", after_help=EXAMPLES)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Import, check, refresh, or remove the locally saved session
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
    },
    /// Search by keywords, named people, places, dates, or text in images
    #[command(
        after_help = "EXAMPLES:\n  gphotos search \"birthday cake\"\n  gphotos search --person \"Emma\"\n  gphotos search beach --person \"Emma\" --json\n  gphotos search \"Paris 2025\" --download ./photos --limit 10\n\n--person contributes a name to Google's text query, not an exact identity filter.\nUse the same names you have assigned to people in Google Photos.\nResults are limited by default; JSON includes next_page_token and complete.\nContinue with the identical query and --page-token TOKEN.\n--download retrieves every displayed result, without changing your library."
    )]
    Search(SearchArgs),
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
    /// Import your signed-in Google Chrome session into macOS Keychain
    #[command(
        after_help = "EXAMPLES:\n  gphotos auth chrome\n  gphotos auth chrome --profile \"Profile 1\" --account 1\n\nSign in at photos.google.com in Chrome first. Choose the Chrome profile\ndirectory (Default, Profile 1, etc.). --account is the N in /u/N/ when a\nprofile has multiple Google accounts; it is independent of --profile.\nmacOS may ask you to allow access to Chrome Safe Storage.\nThe CLI validates Google Photos access before saving the imported session.\nRe-run this command to refresh an expired session. If Chrome's encryption\nor device-bound sessions prevent reuse, cookie import may also fail; the\nCLI reports the failure rather than trying to bypass browser protections."
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
        after_help = "EXAMPLE:\n  gphotos auth cookies /path/to/cookies.txt --account 0\n\nImports only Google Photos-compatible domains and unexpired cookies.\nThe source file is not deleted. Treat it as a password; don't commit or share\nit. On macOS the CLI saves the validated session in Keychain."
    )]
    Cookies {
        file: PathBuf,
        #[arg(long, default_value_t = 0)]
        account: u32,
    },
    /// Verify the saved session with Google Photos; never displays cookies
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Remove the saved CLI session from Keychain
    Logout,
}
#[derive(Args)]
struct SearchArgs {
    /// A Google Photos search query (quote multiple words)
    query: Option<String>,
    /// Named person in Google Photos; combined with the keyword query
    #[arg(long)]
    person: Option<String>,
    /// Maximum results returned (1–1000); use pagination for more
    #[arg(long, default_value_t=20, value_parser=clap::value_parser!(u32).range(1..=1000))]
    limit: u32,
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
            eprintln!("Warning: could not save updated session in Keychain");
        }
        result
    } else {
        saved.context("Command finished, but updated session could not be saved in Keychain")?;
        result
    }
}
fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Auth { command } => {
            match command {
                AuthCommand::Chrome { profile, account } => {
                    eprintln!("Importing Chrome session; macOS may prompt for Chrome Safe Storage access…");
                    let store = auth::import_chrome(&profile)?;
                    let session = Session::new(&store, account, Some(profile))?;
                    let client = PhotosClient::connect(session)?;
                    client.save()?;
                    println!("Google Photos session imported into macOS Keychain. Account: {} (index {}).", client.account_email().unwrap_or("signed in"), client.account_index());
                }
                AuthCommand::Cookies { file, account } => {
                    let session = Session::new(&auth::import_netscape(&file)?, account, None)?;
                    let client = PhotosClient::connect(session)?;
                    client.save()?;
                    println!("Google Photos session imported into macOS Keychain.");
                }
                AuthCommand::Status { json } => with_saved(|client| {
                    if json {
                        print_json(
                            &serde_json::json!({"authenticated":true,"account":client.account_email(),"account_index":client.account_index(),"storage":"macOS Keychain"}),
                        )?;
                    } else {
                        println!("Authenticated: {} (account index {}). Session stored in macOS Keychain.", client.account_email().unwrap_or("Google Photos"), client.account_index());
                    }
                    Ok(())
                })?,
                AuthCommand::Logout => {
                    auth::logout()?;
                    println!("CLI session removed. Chrome remains signed in.");
                }
            }
        }
        Command::Logout => {
            auth::logout()?;
            println!("CLI session removed. Chrome remains signed in.");
        }
        Command::Search(args) => {
            let query = [
                args.person.as_deref().unwrap_or(""),
                args.query.as_deref().unwrap_or(""),
            ]
            .into_iter()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
            if query.is_empty() {
                bail!("Provide a search query or --person NAME. See 'gphotos help search'");
            }
            with_saved(|client| {
                let result =
                    client.search(&query, args.limit as usize, args.page_token.as_deref())?;
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
                    for item in &result.items {
                        println!(
                            "{}  {}  {}  {}",
                            item.id,
                            item.taken_at.as_deref().unwrap_or("date unknown"),
                            item.media_type,
                            item.filename.as_deref().unwrap_or("filename unavailable")
                        );
                    }
                    if let Some(token) = &result.next_page_token {
                        eprintln!("More results available. Continue the same search with --page-token '{token}'");
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
fn main() {
    if let Err(err) = run(Cli::parse()) {
        eprintln!("Error: {err:#}");
        std::process::exit(1);
    }
}
