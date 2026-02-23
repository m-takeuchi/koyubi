use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq)]
struct DictEntry {
    name: &'static str,
    size_bytes: u64,
    description: &'static str,
    recommended: bool,
}

const CATALOG: &[DictEntry] = &[
    DictEntry {
        name: "SKK-JISYO.L",
        size_bytes: 4_300_000,
        description: "Large dictionary",
        recommended: true,
    },
    DictEntry {
        name: "SKK-JISYO.jinmei",
        size_bytes: 800_000,
        description: "Personal names",
        recommended: false,
    },
    DictEntry {
        name: "SKK-JISYO.geo",
        size_bytes: 400_000,
        description: "Place names",
        recommended: false,
    },
    DictEntry {
        name: "SKK-JISYO.station",
        size_bytes: 200_000,
        description: "Station names",
        recommended: false,
    },
    DictEntry {
        name: "SKK-JISYO.propernoun",
        size_bytes: 300_000,
        description: "Proper nouns",
        recommended: false,
    },
];

const BASE_URL: &str = "https://raw.githubusercontent.com/skk-dev/dict/master/";

fn main() {
    if let Err(e) = run() {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        print_usage();
        return Ok(());
    }

    match args[1].as_str() {
        "list" => cmd_list(),
        "download" => cmd_download(&args[2..]),
        "status" => cmd_status(),
        "help" | "--help" | "-h" => {
            print_usage();
            Ok(())
        }
        other => {
            eprintln!("Unknown command: {other}");
            print_usage();
            std::process::exit(1);
        }
    }
}

/// Dictionary storage directory.
/// Windows: %APPDATA%\Koyubi\dict\
/// Linux (for testing): ~/.config/koyubi/dict/
fn dict_dir() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let dir = if cfg!(windows) {
        let appdata =
            env::var("APPDATA").map_err(|_| "APPDATA environment variable not set")?;
        PathBuf::from(appdata).join("Koyubi").join("dict")
    } else {
        let home = env::var("HOME").map_err(|_| "HOME environment variable not set")?;
        PathBuf::from(home)
            .join(".config")
            .join("koyubi")
            .join("dict")
    };
    Ok(dir)
}

fn cmd_list() -> Result<(), Box<dyn std::error::Error>> {
    println!("Available dictionaries:");
    println!();
    for entry in CATALOG {
        let rec = if entry.recommended {
            " [recommended]"
        } else {
            ""
        };
        let size_mb = entry.size_bytes as f64 / 1_000_000.0;
        println!(
            "  {:<24} {:>5.1} MB  {}{}",
            entry.name, size_mb, entry.description, rec
        );
    }
    println!();
    println!("Download:");
    println!("  koyubi-dict download --dict <name>");
    println!("  koyubi-dict download --all");
    println!("  koyubi-dict download        (interactive)");
    Ok(())
}

#[derive(Debug, PartialEq)]
struct DownloadOptions {
    dict_names: Vec<String>,
    all: bool,
    quiet: bool,
}

fn parse_download_args(args: &[String]) -> Result<DownloadOptions, String> {
    let mut opts = DownloadOptions {
        dict_names: Vec::new(),
        all: false,
        quiet: false,
    };

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--all" => opts.all = true,
            "--quiet" | "-q" => opts.quiet = true,
            "--dict" | "-d" => {
                i += 1;
                if i >= args.len() {
                    return Err("--dict requires a dictionary name".to_string());
                }
                opts.dict_names.push(args[i].clone());
            }
            other => {
                return Err(format!("Unknown option: {other}"));
            }
        }
        i += 1;
    }

    Ok(opts)
}

fn find_dict(name: &str) -> Option<&'static DictEntry> {
    CATALOG.iter().find(|e| e.name == name)
}

fn resolve_dict_names(names: &[String]) -> Result<Vec<&'static DictEntry>, String> {
    let mut entries = Vec::new();
    for name in names {
        let entry = find_dict(name).ok_or_else(|| format!("Unknown dictionary: {name}"))?;
        entries.push(entry);
    }
    Ok(entries)
}

fn parse_selection(input: &str) -> Result<Vec<&'static DictEntry>, String> {
    let input = input.trim();

    if input.is_empty() {
        return Ok(Vec::new());
    }

    if input == "all" {
        return Ok(CATALOG.iter().collect());
    }

    let mut selected = Vec::new();
    for token in input.split_whitespace() {
        let num: usize = token
            .parse()
            .map_err(|_| format!("Invalid number: {token}"))?;
        if num < 1 || num > CATALOG.len() {
            return Err(format!("Number out of range: {num}"));
        }
        selected.push(&CATALOG[num - 1]);
    }

    Ok(selected)
}

fn cmd_download(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let opts = parse_download_args(args).map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;

    let entries: Vec<&DictEntry> = if opts.all {
        CATALOG.iter().collect()
    } else if opts.dict_names.is_empty() {
        interactive_select()?
    } else {
        resolve_dict_names(&opts.dict_names)
            .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?
    };

    if entries.is_empty() {
        println!("No dictionaries selected.");
        return Ok(());
    }

    let dir = dict_dir()?;
    fs::create_dir_all(&dir)?;

    let mut success = 0u32;
    let mut skipped = 0u32;
    let mut failed = 0u32;

    for entry in &entries {
        let path = dir.join(entry.name);
        if path.exists() {
            if !opts.quiet {
                eprintln!("[{}] Already exists, skipping", entry.name);
            }
            skipped += 1;
            continue;
        }

        match download_one(entry, &dir, opts.quiet) {
            Ok(()) => success += 1,
            Err(e) => {
                eprintln!("[{}] Failed: {e}", entry.name);
                failed += 1;
            }
        }
    }

    if !opts.quiet {
        eprintln!();
        eprintln!("Done: {success} downloaded, {skipped} skipped, {failed} failed");
    }

    if failed > 0 {
        std::process::exit(1);
    }

    Ok(())
}

fn cmd_status() -> Result<(), Box<dyn std::error::Error>> {
    let dir = dict_dir()?;
    println!("Dictionary directory: {}", dir.display());
    println!();

    for entry in CATALOG {
        let path = dir.join(entry.name);
        if path.exists() {
            let meta = fs::metadata(&path)?;
            let size_mb = meta.len() as f64 / 1_000_000.0;
            println!(
                "  [installed] {:<24} {:>5.1} MB",
                entry.name, size_mb
            );
        } else {
            println!("  [missing]   {}", entry.name);
        }
    }

    Ok(())
}

fn interactive_select() -> Result<Vec<&'static DictEntry>, Box<dyn std::error::Error>> {
    println!("Select dictionaries to download:");
    println!();
    for (i, entry) in CATALOG.iter().enumerate() {
        let rec = if entry.recommended {
            " [recommended]"
        } else {
            ""
        };
        let size_mb = entry.size_bytes as f64 / 1_000_000.0;
        println!(
            "  {}. {:<24} {:>5.1} MB  {}{}",
            i + 1,
            entry.name,
            size_mb,
            entry.description,
            rec
        );
    }
    println!();
    println!("Enter numbers separated by spaces (e.g. \"1 2 3\"), or \"all\":");
    eprint!("> ");
    io::stderr().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;

    parse_selection(&input).map_err(|e| e.into())
}

fn download_one(
    entry: &DictEntry,
    dir: &Path,
    quiet: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let url = format!("{BASE_URL}{}", entry.name);
    let part_path = dir.join(format!("{}.part", entry.name));
    let final_path = dir.join(entry.name);

    if !quiet {
        eprint!("[{}] Connecting...", entry.name);
    }

    let result = download_inner(entry, &url, &part_path, &final_path, quiet);

    // Clean up .part file on failure
    if result.is_err() {
        let _ = fs::remove_file(&part_path);
    }

    result
}

fn download_inner(
    entry: &DictEntry,
    url: &str,
    part_path: &Path,
    final_path: &Path,
    quiet: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut response = ureq::get(url).call()?;

    let content_length: Option<u64> = response
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok());

    let mut file = fs::File::create(part_path)?;
    let mut reader = response.body_mut().as_reader();

    let mut downloaded: u64 = 0;
    let mut buf = [0u8; 8192];

    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        downloaded += n as u64;

        if !quiet {
            let dl_mb = downloaded as f64 / 1_000_000.0;
            if let Some(total) = content_length {
                let total_mb = total as f64 / 1_000_000.0;
                let pct = (downloaded as f64 / total as f64 * 100.0) as u32;
                eprint!(
                    "\r[{}] {:.1}/{:.1} MB ({}%)    ",
                    entry.name, dl_mb, total_mb, pct
                );
            } else {
                eprint!("\r[{}] {:.1} MB    ", entry.name, dl_mb);
            }
        }
    }

    drop(file);
    fs::rename(part_path, final_path)?;

    if !quiet {
        let dl_mb = downloaded as f64 / 1_000_000.0;
        eprintln!(
            "\r[{}] {:.1} MB - done                    ",
            entry.name, dl_mb
        );
    }

    Ok(())
}

fn print_usage() {
    eprintln!("koyubi-dict - SKK dictionary manager for Koyubi");
    eprintln!();
    eprintln!("Usage:");
    eprintln!("  koyubi-dict list                        Show available dictionaries");
    eprintln!("  koyubi-dict download                    Interactive dictionary selection");
    eprintln!("  koyubi-dict download --dict <name>      Download specific dictionary");
    eprintln!("  koyubi-dict download --all              Download all dictionaries");
    eprintln!("  koyubi-dict download --quiet            No progress output (for installer)");
    eprintln!("  koyubi-dict status                      Show installed dictionaries");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &[&str]) -> Vec<String> {
        s.iter().map(|s| s.to_string()).collect()
    }

    // --- Catalog tests ---

    #[test]
    fn catalog_has_entries() {
        assert!(CATALOG.len() >= 5);
    }

    #[test]
    fn catalog_has_one_recommended() {
        let recommended: Vec<_> = CATALOG.iter().filter(|e| e.recommended).collect();
        assert_eq!(recommended.len(), 1);
        assert_eq!(recommended[0].name, "SKK-JISYO.L");
    }

    #[test]
    fn catalog_no_duplicate_names() {
        let mut names: Vec<&str> = CATALOG.iter().map(|e| e.name).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), CATALOG.len());
    }

    #[test]
    fn catalog_all_names_start_with_skk_jisyo() {
        for entry in CATALOG {
            assert!(
                entry.name.starts_with("SKK-JISYO."),
                "{} doesn't start with SKK-JISYO.",
                entry.name
            );
        }
    }

    #[test]
    fn catalog_urls_are_valid() {
        for entry in CATALOG {
            let url = format!("{BASE_URL}{}", entry.name);
            assert!(url.starts_with("https://"));
            assert!(url.ends_with(entry.name));
        }
    }

    // --- find_dict tests ---

    #[test]
    fn find_dict_existing() {
        let entry = find_dict("SKK-JISYO.L");
        assert!(entry.is_some());
        assert_eq!(entry.unwrap().name, "SKK-JISYO.L");
    }

    #[test]
    fn find_dict_nonexistent() {
        assert!(find_dict("SKK-JISYO.NONEXISTENT").is_none());
    }

    #[test]
    fn find_dict_empty_string() {
        assert!(find_dict("").is_none());
    }

    // --- parse_download_args tests ---

    #[test]
    fn parse_args_empty() {
        let opts = parse_download_args(&args(&[])).unwrap();
        assert!(opts.dict_names.is_empty());
        assert!(!opts.all);
        assert!(!opts.quiet);
    }

    #[test]
    fn parse_args_all() {
        let opts = parse_download_args(&args(&["--all"])).unwrap();
        assert!(opts.all);
    }

    #[test]
    fn parse_args_quiet() {
        let opts = parse_download_args(&args(&["--quiet"])).unwrap();
        assert!(opts.quiet);
    }

    #[test]
    fn parse_args_quiet_short() {
        let opts = parse_download_args(&args(&["-q"])).unwrap();
        assert!(opts.quiet);
    }

    #[test]
    fn parse_args_single_dict() {
        let opts = parse_download_args(&args(&["--dict", "SKK-JISYO.L"])).unwrap();
        assert_eq!(opts.dict_names, vec!["SKK-JISYO.L"]);
    }

    #[test]
    fn parse_args_dict_short() {
        let opts = parse_download_args(&args(&["-d", "SKK-JISYO.L"])).unwrap();
        assert_eq!(opts.dict_names, vec!["SKK-JISYO.L"]);
    }

    #[test]
    fn parse_args_multiple_dicts() {
        let opts = parse_download_args(&args(&[
            "--dict", "SKK-JISYO.L",
            "--dict", "SKK-JISYO.geo",
        ])).unwrap();
        assert_eq!(opts.dict_names, vec!["SKK-JISYO.L", "SKK-JISYO.geo"]);
    }

    #[test]
    fn parse_args_all_and_quiet() {
        let opts = parse_download_args(&args(&["--all", "--quiet"])).unwrap();
        assert!(opts.all);
        assert!(opts.quiet);
    }

    #[test]
    fn parse_args_dict_missing_value() {
        let result = parse_download_args(&args(&["--dict"]));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("requires a dictionary name"));
    }

    #[test]
    fn parse_args_unknown_option() {
        let result = parse_download_args(&args(&["--bogus"]));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unknown option"));
    }

    // --- resolve_dict_names tests ---

    #[test]
    fn resolve_valid_names() {
        let names = args(&["SKK-JISYO.L", "SKK-JISYO.geo"]);
        let entries = resolve_dict_names(&names).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "SKK-JISYO.L");
        assert_eq!(entries[1].name, "SKK-JISYO.geo");
    }

    #[test]
    fn resolve_unknown_name() {
        let names = args(&["SKK-JISYO.L", "SKK-JISYO.FAKE"]);
        let result = resolve_dict_names(&names);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("SKK-JISYO.FAKE"));
    }

    #[test]
    fn resolve_empty() {
        let entries = resolve_dict_names(&[]).unwrap();
        assert!(entries.is_empty());
    }

    // --- parse_selection tests ---

    #[test]
    fn selection_empty() {
        let entries = parse_selection("").unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn selection_whitespace_only() {
        let entries = parse_selection("   ").unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn selection_all() {
        let entries = parse_selection("all").unwrap();
        assert_eq!(entries.len(), CATALOG.len());
    }

    #[test]
    fn selection_single_number() {
        let entries = parse_selection("1").unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "SKK-JISYO.L");
    }

    #[test]
    fn selection_multiple_numbers() {
        let entries = parse_selection("1 3 5").unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].name, "SKK-JISYO.L");
        assert_eq!(entries[1].name, "SKK-JISYO.geo");
        assert_eq!(entries[2].name, "SKK-JISYO.propernoun");
    }

    #[test]
    fn selection_last_number() {
        let entries = parse_selection(&CATALOG.len().to_string()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, CATALOG.last().unwrap().name);
    }

    #[test]
    fn selection_zero_is_error() {
        let result = parse_selection("0");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("out of range"));
    }

    #[test]
    fn selection_too_large_is_error() {
        let result = parse_selection("99");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("out of range"));
    }

    #[test]
    fn selection_non_number_is_error() {
        let result = parse_selection("abc");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid number"));
    }

    #[test]
    fn selection_with_trailing_newline() {
        let entries = parse_selection("1 2\n").unwrap();
        assert_eq!(entries.len(), 2);
    }

    // --- dict_dir tests ---

    #[test]
    fn dict_dir_returns_path() {
        let dir = dict_dir().unwrap();
        let path_str = dir.to_string_lossy();
        assert!(
            path_str.contains("koyubi") || path_str.contains("Koyubi"),
            "dict_dir should contain 'koyubi': {}",
            path_str
        );
        assert!(
            path_str.contains("dict"),
            "dict_dir should contain 'dict': {}",
            path_str
        );
    }
}
