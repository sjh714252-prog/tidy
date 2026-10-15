//! `tidy` CLI. Thin adapter over `tidy-core` (SPEC §4: the core knows
//! nothing about its callers).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use humansize::{format_size, BINARY};
use tidy_core::classify::{FileFacts, RuleSet};
use tidy_core::engine::{new_batch_id, Action, Engine};
use tidy_core::mover::{Mover, UndoReport};
use tidy_core::report::{render_tree, scan, ScanOptions, Stats, TreeOptions};
use tidy_core::store::Store;
use tidy_core::watcher::{existing_files, IgnoreFilter, WatchOptions, Watcher};
use tidy_core::Config;

/// How often a locked/busy file is retried before giving up.
const MAX_RETRIES: u32 = 5;

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
    /// Watch the configured folders and organise new files by rules.toml.
    Watch(WatchArgs),
    /// Undo moves (default: the most recent batch).
    Undo(UndoArgs),
    /// Show recent moves from move_log.
    Log(LogArgs),
    /// List files waiting for confirmation (rules with confirm = true).
    Pending,
    /// Show which rule would apply to the given files (moves nothing).
    Classify(ClassifyArgs),
}

#[derive(Args)]
struct WatchArgs {
    /// Rules file (default: classify.rules from the config).
    #[arg(long)]
    rules: Option<PathBuf>,
    /// Also organise files already in the watch folders at startup.
    #[arg(long)]
    process_existing: bool,
    /// Print what would happen without moving anything.
    #[arg(long)]
    dry_run: bool,
}

#[derive(Args)]
#[group(multiple = false)]
struct UndoArgs {
    /// Undo the most recent batch (default).
    #[arg(long)]
    last: bool,
    /// Undo a whole batch (id or unique prefix, see `tidy log`).
    #[arg(long)]
    batch: Option<String>,
    /// Undo a single move_log entry.
    #[arg(long)]
    id: Option<i64>,
}

#[derive(Args)]
struct LogArgs {
    /// Number of entries.
    #[arg(short = 'n', long, default_value_t = 20)]
    limit: usize,
}

#[derive(Args)]
struct ClassifyArgs {
    /// Files to classify.
    #[arg(required = true)]
    paths: Vec<PathBuf>,
    /// Rules file (default: classify.rules from the config).
    #[arg(long)]
    rules: Option<PathBuf>,
}

#[derive(Args)]
struct ScanArgs {
    /// Directory to scan (default: current directory).
    path: Option<PathBuf>,
    /// Maximum depth to print (scan still computes full totals unless
    /// --scan-depth is given).
    #[arg(short, long)]
    depth: Option<usize>,
    /// Maximum depth to descend while scanning. Use on network drives or
    /// huge trees; totals then cover only the scanned portion.
    #[arg(long)]
    scan_depth: Option<usize>,
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
        Cmd::Watch(args) => cmd_watch(cli.config, args),
        Cmd::Undo(args) => cmd_undo(cli.config, args),
        Cmd::Log(args) => cmd_log(cli.config, args),
        Cmd::Pending => cmd_pending(cli.config),
        Cmd::Classify(args) => cmd_classify(cli.config, args),
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
            max_depth: args.scan_depth,
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

/// Load the config given by `--config` (must exist) or the default
/// location (may be absent → defaults).
fn load_config(explicit: Option<PathBuf>) -> Result<Config> {
    match explicit {
        Some(p) => Config::load(&p).with_context(|| format!("loading {}", p.display())),
        None => match Config::default_path() {
            Some(p) if p.exists() => Ok(Config::load(&p)?),
            Some(p) => {
                let mut c = Config::default();
                c.resolve_relative(p.parent().unwrap_or_else(|| Path::new("")));
                Ok(c)
            }
            None => Ok(Config::default()),
        },
    }
}

fn load_rules(cfg: &Config, override_path: Option<PathBuf>) -> Result<RuleSet> {
    let path = override_path.unwrap_or_else(|| cfg.classify.rules.clone());
    let rules = RuleSet::load(&path)?;
    if rules.is_empty() {
        eprintln!("warning: {} has no enabled rules", path.display());
    }
    eprintln!(
        "# rules: {} ({} rules)",
        path.display(),
        rules.rules().len()
    );
    Ok(rules)
}

fn open_mover(cfg: &Config) -> Result<Mover> {
    let db = cfg.store.db_path();
    let store = Store::open(&db).with_context(|| format!("opening {}", db.display()))?;
    Ok(Mover::new(store, cfg.mover.duplicate_policy))
}

fn cmd_watch(config: Option<PathBuf>, args: WatchArgs) -> Result<()> {
    let cfg = load_config(config)?;
    if cfg.watches.is_empty() {
        bail!("no [[watch]] folders configured");
    }
    let rules = load_rules(&cfg, args.rules)?;
    let mover = open_mover(&cfg)?;
    let pruned = mover
        .store()
        .prune_move_log(cfg.mover.undo_retention_days)?;
    if pruned > 0 {
        tracing::info!(pruned, "old move_log rows pruned");
    }
    eprintln!("# db: {}", cfg.store.db_path().display());
    let expected = mover.expected();
    let filter = IgnoreFilter::new(&cfg.classify.ignore_globs)?;
    let engine = Engine::new(rules, mover, filter.clone()).dry_run(args.dry_run);

    let (watcher, rx) = Watcher::start(
        &cfg.watches,
        WatchOptions::from_config(&cfg.classify),
        expected,
    )?;

    if args.process_existing {
        let batch = new_batch_id();
        for root in &cfg.watches {
            for path in existing_files(root, &filter) {
                report_action(engine.handle(&path, &batch), &path);
            }
        }
    }

    for r in watcher.roots() {
        eprintln!("# watching: {}", r.display());
    }
    eprintln!(
        "# ready{} — Ctrl+C to stop",
        if args.dry_run { " (dry run)" } else { "" }
    );

    let mut retries: HashMap<PathBuf, u32> = HashMap::new();
    for ev in rx {
        match engine.handle(&ev.path, &new_batch_id()) {
            Err(e) if e.is_transient() => {
                let n = retries.entry(ev.path.clone()).or_default();
                *n += 1;
                if *n <= MAX_RETRIES {
                    tracing::info!(path = %ev.path.display(), attempt = *n, "busy, retrying: {e}");
                    watcher.requeue(ev.path);
                } else {
                    retries.remove(&ev.path);
                    report_action(Err(e), &ev.path);
                }
            }
            res => {
                retries.remove(&ev.path);
                report_action(res, &ev.path);
            }
        }
    }
    Ok(())
}

fn report_action(res: Result<Action, tidy_core::engine::EngineError>, path: &Path) {
    match res {
        Ok(Action::Moved {
            src,
            dst,
            rule,
            batch_id,
            ..
        }) => println!(
            "moved     {} -> {}  [{}] batch {}",
            src.display(),
            dst.display(),
            rule,
            short(&batch_id)
        ),
        Ok(Action::Pending {
            path,
            rule,
            suggested,
        }) => println!(
            "pending   {} (suggested {})  [{}] confirm=true",
            path.display(),
            suggested.display(),
            rule
        ),
        Ok(Action::WouldMove {
            path,
            rule,
            dest_dir,
            confirm,
        }) => println!(
            "would {}  {} -> {}  [{}]",
            if confirm { "ask " } else { "move" },
            path.display(),
            dest_dir.display(),
            rule
        ),
        Ok(Action::Duplicate {
            src,
            existing,
            rule,
        }) => println!(
            "duplicate {} (identical to {}; left in place)  [{}]",
            src.display(),
            existing.display(),
            rule
        ),
        Ok(Action::AlreadyInPlace { path, rule }) => {
            tracing::info!(path = %path.display(), rule, "already in place")
        }
        Ok(Action::NoRule(p)) => println!("no rule   {}", p.display()),
        Ok(Action::Skipped { path, reason }) => {
            println!("skipped   {} ({reason})", path.display())
        }
        Err(e) => eprintln!("error     {}: {e:#}", path.display()),
    }
}

fn short(id: &str) -> &str {
    &id[..id.len().min(8)]
}

fn fmt_time(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|| ts.to_string())
}

fn cmd_undo(config: Option<PathBuf>, args: UndoArgs) -> Result<()> {
    let cfg = load_config(config)?;
    let mover = open_mover(&cfg)?;
    let report = if let Some(id) = args.id {
        mover.undo_one(id)?
    } else if let Some(prefix) = &args.batch {
        let ids = mover.store().resolve_batch(prefix)?;
        match ids.as_slice() {
            [one] => mover.undo_batch(one)?,
            [] => bail!("no batch matches \"{prefix}\""),
            _ => bail!(
                "batch prefix \"{prefix}\" is ambiguous ({} matches)",
                ids.len()
            ),
        }
    } else {
        let _ = args.last; // default
        match mover.undo_last()? {
            Some(r) => r,
            None => {
                println!("nothing to undo");
                return Ok(());
            }
        }
    };
    print_undo(&report)
}

fn print_undo(r: &UndoReport) -> Result<()> {
    if let Some(b) = &r.batch_id {
        println!("batch {}", short(b));
    }
    for (from, to) in &r.restored {
        println!("restored  {} -> {}", from.display(), to.display());
    }
    for c in &r.conflicts {
        println!(
            "conflict  #{} {} -> {}: {}",
            c.log_id,
            c.dst_path.display(),
            c.src_path.display(),
            c.reason
        );
    }
    if !r.conflicts.is_empty() {
        bail!(
            "{} move(s) could not be undone (left as they are)",
            r.conflicts.len()
        );
    }
    Ok(())
}

fn cmd_log(config: Option<PathBuf>, args: LogArgs) -> Result<()> {
    let cfg = load_config(config)?;
    let store = Store::open(&cfg.store.db_path())?;
    let moves = store.recent_moves(args.limit)?;
    if moves.is_empty() {
        println!("no moves recorded ({})", cfg.store.db_path().display());
    }
    for m in moves {
        println!(
            "#{:<5} {}  batch {}  {:<11} {} -> {}{}",
            m.id,
            fmt_time(m.moved_at),
            short(&m.batch_id),
            format!(
                "[{}]",
                m.rule_name.as_deref().unwrap_or(m.decided_by.as_str())
            ),
            m.src_path.display(),
            m.dst_path.display(),
            m.undone_at
                .map(|t| format!("  (undone {})", fmt_time(t)))
                .unwrap_or_default()
        );
    }
    Ok(())
}

fn cmd_pending(config: Option<PathBuf>) -> Result<()> {
    let cfg = load_config(config)?;
    let store = Store::open(&cfg.store.db_path())?;
    let items = store.list_pending()?;
    if items.is_empty() {
        println!("no pending files");
    }
    for p in items {
        println!(
            "{}  {} -> {}  [{}]{}",
            fmt_time(p.created_at),
            p.path.display(),
            p.suggested_dest
                .as_deref()
                .map(|d| d.display().to_string())
                .unwrap_or_default(),
            p.reason.as_deref().unwrap_or(""),
            if p.path.exists() { "" } else { "  (file gone)" }
        );
    }
    Ok(())
}

fn cmd_classify(config: Option<PathBuf>, args: ClassifyArgs) -> Result<()> {
    let cfg = load_config(config)?;
    let rules = load_rules(&cfg, args.rules)?;
    for p in &args.paths {
        let p = tidy_core::config::expand_home(p);
        if !p.is_file() {
            continue;
        }
        let facts = FileFacts::from_path(&p).with_context(|| format!("reading {}", p.display()))?;
        match rules.classify(&facts) {
            Some(c) => println!(
                "{} -> {}  [{}]{}",
                p.display(),
                c.dest_dir.display(),
                c.rule_name,
                if c.confirm { " confirm=true" } else { "" }
            ),
            None => println!("{} -> (no rule)", p.display()),
        }
    }
    Ok(())
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
