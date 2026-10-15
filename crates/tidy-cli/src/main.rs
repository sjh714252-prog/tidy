//! `tidy` CLI. Thin adapter over `tidy-core` (SPEC §4: the core knows
//! nothing about its callers).

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use humansize::{format_size, BINARY};
use tidy_core::report::{render_tree, scan, ScanOptions, Stats, TreeOptions};
use tidy_core::Config;

#[derive(Parser)]
#[command(name = "tidy", version, about = "Desktop & folder organizer")]
struct Cli {
    /// Path to config.toml (defaults to the platform config dir).
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    /// Log verbosity (-v, -vv).
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    verbose: u8,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Scan a directory and print its tree and statistics.
    Scan(ScanArgs),
    /// Print the effective configuration.
    Config,
}

#[derive(Args)]
struct ScanArgs {
    /// Directory to scan (default: current directory).
    path: Option<PathBuf>,
    /// Maximum depth to print (scan still computes full totals).
    #[arg(short, long)]
    depth: Option<usize>,
    /// Output format.
    #[arg(short, long, value_enum, default_value_t = Format::Text)]
    format: Format,
    /// Hide individual files; show directories only.
    #[arg(long)]
    dirs_only: bool,
    /// Also include hidden (dot) entries.
    #[arg(long)]
    hidden: bool,
    /// Number of entries in "largest" lists.
    #[arg(long, default_value_t = 10)]
    top: usize,
    /// Days after which a file counts as stale.
    #[arg(long, default_value_t = 90)]
    stale_days: u64,
    /// Skip the statistics section.
    #[arg(long)]
    no_stats: bool,
}

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Text,
    Json,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    init_logging(cli.verbose);

    match cli.cmd {
        Cmd::Scan(args) => cmd_scan(args),
        Cmd::Config => cmd_config(cli.config),
    }
}

fn init_logging(verbosity: u8) {
    let level = match verbosity {
        0 => "warn",
        1 => "info",
        _ => "debug",
    };
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(level));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();
}

fn cmd_scan(args: ScanArgs) -> Result<()> {
    let path = args
        .path
        .unwrap_or_else(|| std::env::current_dir().expect("cwd"));
    let path = tidy_core::config::expand_home(&path);

    let res = scan(
        &path,
        &ScanOptions {
            max_depth: None,
            skip_hidden: !args.hidden,
            follow_links: false,
        },
    )
    .with_context(|| format!("scanning {}", path.display()))?;

    let stats = (!args.no_stats).then(|| {
        Stats::compute(
            &res.tree,
            args.top,
            Duration::from_secs(args.stale_days * 86400),
        )
    });

    match args.format {
        Format::Json => {
            let out = serde_json::json!({
                "root": res.root,
                "tree": res.tree,
                "stats": stats,
                "errors": res.errors,
            });
            println!("{}", serde_json::to_string_pretty(&out)?);
        }
        Format::Text => {
            print!(
                "{}",
                render_tree(
                    &res.tree,
                    &TreeOptions {
                        max_depth: args.depth,
                        show_files: !args.dirs_only,
                        ..Default::default()
                    },
                )
            );
            if let Some(s) = &stats {
                print_stats(s, args.stale_days);
            }
            if !res.errors.is_empty() {
                eprintln!("\n{} entries could not be read:", res.errors.len());
                for e in res.errors.iter().take(10) {
                    eprintln!("  {e}");
                }
            }
        }
    }
    Ok(())
}

fn print_stats(s: &Stats, stale_days: u64) {
    println!("\n── Summary ──────────────────────────────");
    println!(
        "files: {}   dirs: {}   size: {}   root-level files: {}   stale(>{}d): {}",
        s.total_files,
        s.total_dirs,
        format_size(s.total_bytes, BINARY),
        s.root_level_files,
        stale_days,
        s.stale_files
    );

    println!("\n── By type ──────────────────────────────");
    for t in s.by_type.iter().take(15) {
        println!(
            "{:>10}  {:>6} files  {:>10}",
            t.ext,
            t.count,
            format_size(t.bytes, BINARY)
        );
    }

    println!("\n── Largest dirs ─────────────────────────");
    for d in &s.largest_dirs {
        println!(
            "{:>10}  {:>6} files  {}",
            format_size(d.bytes, BINARY),
            d.files,
            d.path
        );
    }

    println!("\n── Largest files ────────────────────────");
    for f in &s.largest_files {
        println!("{:>10}  {}", format_size(f.bytes, BINARY), f.path);
    }

    if !s.empty_dirs.is_empty() {
        println!(
            "\n── Empty dirs ({}) ──────────────────────",
            s.empty_dirs.len()
        );
        for d in s.empty_dirs.iter().take(20) {
            println!("  {d}");
        }
    }
}

fn cmd_config(path: Option<PathBuf>) -> Result<()> {
    let path = path.or_else(Config::default_path);
    let cfg = match &path {
        Some(p) if p.exists() => Config::load(p)?,
        _ => Config::default(),
    };
    if let Some(p) = &path {
        eprintln!(
            "# config: {} ({})",
            p.display(),
            if p.exists() { "loaded" } else { "defaults" }
        );
    }
    println!("{}", toml::to_string_pretty(&cfg)?);
    Ok(())
}
