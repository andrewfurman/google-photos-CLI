use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use google_photos_cli::{
    auth::{self, Session},
    client::{self, PhotosClient},
};
use std::path::PathBuf;

const EXAMPLES: &str = "GET STARTED:
  macOS Chrome:   gphotos auth chrome
  Any platform:   gphotos auth cookies /path/to/cookies.txt
  Other machine:  gphotos auth import /path/to/session.env

EXAMPLES:
  gphotos search \"birthday cake\" --json
  gphotos people --name Emma
  gphotos search --person Emma
  gphotos search --person Lucy --limit 10
  gphotos search beach --person Emma --json
  gphotos inspect PHOTO_ID
  gphotos inspect PHOTO_ID --json
  gphotos download PHOTO_ID --dir ./photos
  gphotos auth export --output session.env
  gphotos help auth
  gphotos help search

SESSION STORAGE:
  Saves one GPHOTOS_SESSION value in a private .env file. It packages browser
  cookies, not a permanent Google API token. Normal commands never use Keychain.
  Export/import that value between macOS, Linux, and Windows. Sessions may expire
  or require Google verification after being copied. Browser activity does not
  guarantee that an imported session stays valid. Use --session-file PATH to
  choose a file; 'auth status' shows the default path without revealing secrets.

SEARCH AND INSPECT:
  Keywords use Google's own search. --person resolves a named face group and
  uses Google's structured person filter. inspect lists the returned people,
  camera settings, location, timestamps, dimensions, file size, albums and source.
  Missing metadata stays unknown; no names are inferred from image recognition.

Unofficial project; not affiliated with Google. Uses an undocumented web API.
Read commands never modify your Google Photos library.";

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
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
    },
    /// Search by keyword or a tagged person's named face group
    #[command(
        after_help = "EXAMPLES:\n  gphotos search \"birthday cake\"\n  gphotos search --person Emma\n  gphotos search --person Lucy --json\n  gphotos search beach --person Emma\n\n--person resolves a unique named Google Photos face group. Full names and\n--person-id disambiguate people with similar names; see 'gphotos people'.\nWith a keyword, Google receives both the query and the face-group filter.\nJSON includes person, next_page_token and complete. Continue with the same\nquery/person/account and --page-token TOKEN. --download saves displayed results."
    )]
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
    /// Show photo metadata and Google Photos' tagged people
    #[command(
        after_help = "EXAMPLES:\n  gphotos inspect PHOTO_ID\n  gphotos inspect PHOTO_ID --json\n\nShows camera make/model, exposure, aperture, focal length, ISO, location\ncoordinates and labels, dates, dimensions, file size, source, albums,\nstorage quality, and tagged people when Google returns those fields.\nMissing fields are null. people_status=not_returned does not imply that\nno people appear in the image. This shows Google's metadata, not all\npossible EXIF fields or newly inferred identities."
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
    #[arg(long)]
    person: Option<String>,
    /// Exact face-group ID from 'gphotos people'
    #[arg(long, conflicts_with = "person")]
    person_id: Option<String>,
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
            let query = args.query.as_deref().unwrap_or("").trim();
            if query.is_empty() && args.person.is_none() && args.person_id.is_none() {
                bail!("Provide a search query or --person NAME. See 'gphotos help search'");
            }
            with_saved(|client| {
                let result = if args.person.is_some() || args.person_id.is_some() {
                    client.search_person(
                        query,
                        args.person.as_deref(),
                        args.person_id.as_deref(),
                        args.limit as usize,
                        args.page_token.as_deref(),
                    )?
                } else {
                    client.search(query, args.limit as usize, args.page_token.as_deref())?
                };
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
                    if let Some(person) = &result.person {
                        println!(
                            "Tagged person: {} ({})",
                            person.name.as_deref().unwrap_or("unnamed"),
                            person.id
                        );
                    }
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
        println!("Description: {description}");
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
