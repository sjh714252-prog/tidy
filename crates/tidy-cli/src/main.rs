//! `tidy` CLI. Thin adapter over `tidy-core` (SPEC §4: the core knows
//! nothing about its callers).

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use humansize::{format_size, BINARY};
use tidy_core::ai;
use tidy_core::classify::{learner, AiClassifier, FileFacts, RuleSet};
use tidy_core::config::RootKind;
use tidy_core::engine::{new_batch_id, reindex_undo, Action};
use tidy_core::index::{jobs_from_config, FileOutcome, IndexOptions, IndexReport, Indexer};
use tidy_core::mover::{ExpectedMoves, Mover, ProgressFn, TransferProgress, UndoReport};
use tidy_core::pending::{resolve, Decision, Resolution};
use tidy_core::report::{self, render_tree, restructure, scan, ScanOptions, Stats, TreeOptions};
use tidy_core::search::{search, MatchedIn, Query, SearchOptions};
use tidy_core::service::{run_watch, WatchEvent, WatchRunOptions};
use tidy_core::store::{PendingSource, Store};
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
    /// Watch the configured folders and organise new files by rules.toml.
    Watch(WatchArgs),
    /// Undo moves (default: the most recent batch).
    Undo(UndoArgs),
    /// Resolve moves interrupted by a crash or a lost NAS connection
    /// (`tidy watch` also does this when it starts).
    Recover(RecoverArgs),
    /// Show recent moves from move_log.
    Log(LogArgs),
    /// List files waiting for confirmation (rules with confirm = true), or
    /// resolve one with --move / --skip / --ignore.
    Pending(PendingArgs),
    /// Show which rule would apply to the given files (moves nothing).
    Classify(ClassifyArgs),
    /// Index folders or single files (metadata + body text) for `tidy search`.
    Index(IndexArgs),
    /// Full-text search over the index.
    Search(SearchArgs),
    /// AI status, API keys (OS keychain), test call, call log.
    #[command(subcommand)]
    Ai(AiCmd),
    /// Rule suggestions learned from your decisions (--accept / --dismiss).
    Learn(LearnArgs),
    /// Folder report: tree, statistics, duplicates, long-unused files.
    Report(ReportArgs),
    /// Propose a reorganisation (rules or AI), show the diff, apply on approval.
    Restructure(RestructureArgs),
}

#[derive(Clone, Copy, ValueEnum)]
enum ReportFormat {
    Text,
    Json,
    Html,
}

#[derive(Args)]
struct ReportArgs {
    /// Folder (default: the first [[watch]] folder).
    path: Option<PathBuf>,
    /// text | json | html (HTML goes to stdout: `> report.html`).
    #[arg(short, long, value_enum, default_value_t = ReportFormat::Text)]
    format: ReportFormat,
    /// Tree depth to print.
    #[arg(short, long, default_value_t = 3)]
    depth: usize,
    /// Skip duplicate detection (it reads file contents).
    #[arg(long)]
    no_dups: bool,
    /// Days without use after which a file counts as stale.
    #[arg(long)]
    stale_days: Option<u64>,
}

#[derive(Args)]
struct RestructureArgs {
    /// Folder to reorganise (default: the first [[watch]] folder).
    path: Option<PathBuf>,
    /// Plan from rules.toml (no AI needed).
    #[arg(long, conflicts_with_all = ["ai", "show", "apply", "list"])]
    rules: bool,
    /// Plan proposed by the AI (SPEC §8.2).
    #[arg(long, conflicts_with_all = ["show", "apply", "list"])]
    ai: bool,
    /// --rules: include files in subfolders.
    #[arg(long, requires = "rules")]
    recursive: bool,
    /// Folders never to touch (added to [report] protected).
    #[arg(long, value_name = "DIR")]
    protect: Vec<PathBuf>,
    /// Show a saved plan with its diff.
    #[arg(long, value_name = "PLAN")]
    show: Option<i64>,
    /// Apply a saved plan (all moves, or --only).
    #[arg(long, value_name = "PLAN")]
    apply: Option<i64>,
    /// Comma-separated move numbers for --show / --apply, e.g. 1,3,5.
    #[arg(long, value_delimiter = ',', value_name = "N,..")]
    only: Vec<u32>,
    /// List saved plans.
    #[arg(long)]
    list: bool,
}

#[derive(Subcommand)]
enum AiCmd {
    /// Show whether the AI step is usable and why not.
    Status,
    /// Store or remove an API key in the OS keychain.
    #[command(subcommand)]
    Key(KeyCmd),
    /// Ask the AI where a file belongs (moves nothing; costs one call).
    Test {
        file: PathBuf,
        /// Rules file (default: classify.rules from the config).
        #[arg(long)]
        rules: Option<PathBuf>,
    },
    /// Recent AI calls: tokens, cost, latency, errors (no contents).
    Log {
        #[arg(short = 'n', long, default_value_t = 20)]
        limit: usize,
    },
}

#[derive(Subcommand)]
enum KeyCmd {
    /// Read the key from the terminal (not echoed) or stdin and store it
    /// under KEY_REF (e.g. tidy/anthropic, as in the profile's key_ref).
    Set { key_ref: String },
    /// Remove the stored key.
    Delete { key_ref: String },
}

#[derive(Args)]
#[group(multiple = false)]
struct LearnArgs {
    /// Add suggestion N (as listed) to the top of rules.toml.
    #[arg(long, value_name = "N")]
    accept: Option<usize>,
    /// Never suggest N again.
    #[arg(long, value_name = "N")]
    dismiss: Option<usize>,
}

#[derive(Args)]
struct IndexArgs {
    /// Folders or files to index (default: all [[watch]] and [[index_only]]
    /// roots). A file outside every indexed folder registers its own folder
    /// as a root (that folder is not walked; only the given file is indexed).
    paths: Vec<PathBuf>,
    /// Do not descend into subfolders (folders only).
    #[arg(long)]
    no_recursive: bool,
    /// Metadata only, skip body text (what network roots do by default).
    #[arg(long)]
    no_content: bool,
    /// Drop the WHOLE index (every root) first, then index the given paths.
    /// To re-extract a single changed file, just run `tidy index <file>`.
    #[arg(long)]
    rebuild: bool,
    /// Print index statistics and exit.
    #[arg(long)]
    status: bool,
    /// Forget a folder (and its files) or a single file instead of indexing it.
    /// Only index rows are removed; files on disk are untouched.
    #[arg(long)]
    forget: bool,
    /// Output format.
    #[arg(short, long, value_enum, default_value_t = Format::Text)]
    format: Format,
}

#[derive(Args)]
struct SearchArgs {
    /// Query: words, "phrases" and filters (ext:pdf in:~/Documents after:2026-09
    /// before:2026-10 size:>10mb type:image|doc|video|audio|archive|code).
    #[arg(required = true, num_args = 1..)]
    query: Vec<String>,
    /// Maximum number of results.
    #[arg(short = 'n', long, default_value_t = 20)]
    limit: usize,
    /// Output format.
    #[arg(short, long, value_enum, default_value_t = Format::Text)]
    format: Format,
}

#[derive(Args)]
struct PendingArgs {
    /// Move entry ID to its suggested folder (or --to).
    #[arg(long, value_name = "ID", conflicts_with_all = ["skip", "ignore"])]
    r#move: Option<i64>,
    /// Destination folder for --move instead of the suggestion.
    #[arg(long, requires = "move", value_name = "DIR")]
    to: Option<PathBuf>,
    /// New file name for --move (keeps the extension if none is given).
    #[arg(long, requires = "move", value_name = "NAME")]
    rename: Option<String>,
    /// Drop entry ID from the queue and leave the file (this time only).
    #[arg(long, value_name = "ID", conflicts_with = "ignore")]
    skip: Option<i64>,
    /// Leave the file and never organise this path again.
    #[arg(long, value_name = "ID")]
    ignore: Option<i64>,
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
    /// Do not update the search index for handled/moved files.
    #[arg(long)]
    no_index: bool,
    /// Rules only: never ask the AI, even if [ai] is enabled.
    #[arg(long)]
    no_ai: bool,
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
    /// One line per batch (move history timeline; undo with --batch).
    #[arg(long)]
    batches: bool,
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
        Cmd::Recover(args) => cmd_recover(cli.config, args),
        Cmd::Log(args) => cmd_log(cli.config, args),
        Cmd::Pending(args) => cmd_pending(cli.config, args),
        Cmd::Classify(args) => cmd_classify(cli.config, args),
        Cmd::Index(args) => cmd_index(cli.config, args),
        Cmd::Search(args) => cmd_search(cli.config, args),
        Cmd::Ai(cmd) => cmd_ai(cli.config, cmd),
        Cmd::Learn(args) => cmd_learn(cli.config, args),
        Cmd::Report(args) => cmd_report(cli.config, args),
        Cmd::Restructure(args) => cmd_restructure(cli.config, args),
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
    let rules_path = args.rules.clone().unwrap_or(cfg.classify.rules.clone());
    eprintln!("# rules: {}", rules_path.display());
    eprintln!("# db: {}", cfg.store.db_path().display());
    for w in cfg.watches.iter().filter(|w| w.kind == RootKind::Network) {
        eprintln!(
            "# network: {} (poll every {}s{}, {})",
            w.path.display(),
            w.poll_interval().map(|d| d.as_secs()).unwrap_or_default(),
            if w.poll_only {
                ", polling only"
            } else {
                " + events"
            },
            if w.index_content {
                "content indexed"
            } else {
                "metadata only"
            }
        );
    }
    let progress: ProgressFn = Arc::new(|p: &TransferProgress| {
        let pct = (p.copied * 100).checked_div(p.total).unwrap_or(100);
        eprintln!(
            "# {} {pct:>3}% {} / {}  {}",
            match p.phase {
                "copy" => "copying ",
                "verify" => "verifying",
                _ => "copied  ",
            },
            format_size(p.copied, BINARY),
            format_size(p.total, BINARY),
            p.dst.display()
        );
    });
    let opts = WatchRunOptions {
        rules: args.rules,
        process_existing: args.process_existing,
        dry_run: args.dry_run,
        index: !args.no_index,
        ai: !args.no_ai,
        progress: Some(progress),
        ..Default::default()
    };
    let stop = AtomicBool::new(false); // Ctrl+C ends the process
    run_watch(&cfg, &opts, ExpectedMoves::new(), &stop, |ev| match ev {
        WatchEvent::Warning(w) => eprintln!("# warning: {w}"),
        WatchEvent::Recovered(r) => {
            for p in &r.completed {
                eprintln!("# recovered: interrupted move finished -> {}", p.display());
            }
            for p in &r.rolled_back {
                eprintln!(
                    "# recovered: interrupted move rolled back, file stays at {}",
                    p.display()
                );
            }
            if r.tmp_removed > 0 {
                eprintln!(
                    "# recovered: removed {} leftover scratch copies",
                    r.tmp_removed
                );
            }
            for a in &r.attention {
                eprintln!("# check: {a}");
            }
            if r.unresolved > 0 {
                eprintln!(
                    "# {} interrupted moves wait for an offline share",
                    r.unresolved
                );
            }
        }
        WatchEvent::Root(r) => {
            if r.online {
                eprintln!(
                    "# online: {}{}",
                    r.path.display(),
                    if r.events { "" } else { " (polling only)" }
                );
            } else {
                eprintln!(
                    "# offline: {} — paused, retrying every {}s",
                    r.path.display(),
                    tidy_core::watcher::RECONNECT_INTERVAL.as_secs()
                );
            }
        }
        WatchEvent::Started(roots) => {
            for r in roots {
                eprintln!("# watching: {}", r.display());
            }
            eprintln!(
                "# ready{} — Ctrl+C to stop",
                if args.dry_run { " (dry run)" } else { "" }
            );
        }
        WatchEvent::Ai(Ok(m)) => eprintln!("# ai: {m}"),
        WatchEvent::Ai(Err(e)) => eprintln!("# ai: off — {e} (rules only)"),
        WatchEvent::Handled(path, res) => report_action(res, path),
    })?;
    Ok(())
}

fn report_action(res: &Result<Action, tidy_core::engine::EngineError>, path: &Path) {
    match res {
        Ok(Action::Moved {
            src,
            dst,
            rule,
            batch_id,
            confidence,
            ..
        }) => println!(
            "moved     {} -> {}  [{}{}] batch {}",
            src.display(),
            dst.display(),
            rule,
            pct(*confidence),
            short(batch_id)
        ),
        Ok(Action::Pending {
            path,
            rule,
            suggested,
            confidence,
            reason,
            ..
        }) => println!(
            "pending   {} (suggested {})  [{}{}] {}",
            path.display(),
            suggested.display(),
            rule,
            pct(*confidence),
            reason.as_deref().unwrap_or("confirm=true")
        ),
        Ok(Action::WouldMove {
            path,
            rule,
            dest_dir,
            confirm,
        }) => println!(
            "would {}  {} -> {}  [{}]",
            if *confirm { "ask " } else { "move" },
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
        Ok(Action::AiFailed { path, error }) => {
            println!("no rule   {} (AI unavailable: {error})", path.display())
        }
        Ok(Action::Skipped { path, reason }) => {
            println!("skipped   {} ({reason})", path.display())
        }
        Err(e) => eprintln!("error     {}: {e:#}", path.display()),
    }
}

fn pct(c: Option<f64>) -> String {
    c.map(|c| format!(" {:.0}%", c * 100.0)).unwrap_or_default()
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

#[derive(Args)]
struct RecoverArgs {
    /// Only rows older than this count as interrupted (a long copy may be
    /// running in another Tidy). 0 = all — only when no Tidy is running.
    #[arg(long, default_value_t = tidy_core::mover::DEFAULT_RECOVERY_GRACE.as_secs())]
    grace_secs: u64,
}

fn cmd_recover(config: Option<PathBuf>, args: RecoverArgs) -> Result<()> {
    let cfg = load_config(config)?;
    let mover = open_mover(&cfg)?;
    let r = mover.recover_interrupted(Duration::from_secs(args.grace_secs))?;
    for p in &r.completed {
        println!("finished    {}", p.display());
    }
    for p in &r.rolled_back {
        println!("rolled back {} (file stays here)", p.display());
    }
    for a in &r.attention {
        println!("check       {a}");
    }
    if r.tmp_removed > 0 {
        println!("removed {} leftover scratch copies", r.tmp_removed);
    }
    if r.unresolved > 0 {
        println!(
            "{} moves wait for an offline share; run again once it is back",
            r.unresolved
        );
    }
    if r.is_empty() {
        println!("nothing to recover");
    }
    Ok(())
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
    reindex_undo(mover.store(), &report, &IndexOptions::from_config(&cfg));
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
    if args.batches {
        for b in store.batches(args.limit)? {
            println!(
                "{}  batch {}  {:<11} {:>4} moves{}  {}  e.g. {}",
                fmt_time(b.last_at),
                short(&b.batch_id),
                b.decided_by,
                b.moves,
                match b.undone {
                    0 => String::new(),
                    n if n == b.moves => "  (undone)".into(),
                    n => format!("  ({n} undone)"),
                },
                b.label.as_deref().unwrap_or(""),
                b.example.display()
            );
        }
        return Ok(());
    }
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
            match (m.undone_at, m.in_progress) {
                (Some(t), _) => format!("  (undone {})", fmt_time(t)),
                (None, true) => "  (in progress / interrupted)".into(),
                (None, false) => String::new(),
            }
        );
    }
    Ok(())
}

fn cmd_pending(config: Option<PathBuf>, args: PendingArgs) -> Result<()> {
    let cfg = load_config(config)?;
    let decision = if let Some(id) = args.r#move {
        Some((
            id,
            Decision::Move {
                dest_dir: args.to.map(|p| tidy_core::config::expand_home(&p)),
                new_name: args.rename,
            },
        ))
    } else if let Some(id) = args.skip {
        Some((id, Decision::Skip))
    } else {
        args.ignore.map(|id| (id, Decision::Ignore))
    };
    if let Some((id, decision)) = decision {
        let mover = open_mover(&cfg)?;
        let batch = new_batch_id();
        let r = resolve(
            &mover,
            id,
            &decision,
            &batch,
            Some(&IndexOptions::from_config(&cfg)),
        )?;
        match r {
            Resolution::Moved { src, dst, .. } => println!(
                "moved     {} -> {}  batch {}",
                src.display(),
                dst.display(),
                short(&batch)
            ),
            Resolution::Duplicate { src, existing } => println!(
                "duplicate {} (identical to {}; left in place)",
                src.display(),
                existing.display()
            ),
            Resolution::AlreadyInPlace { path } => println!("in place  {}", path.display()),
            Resolution::Skipped { path } => println!("skipped   {}", path.display()),
            Resolution::Ignored { path } => println!("ignored   {} (permanently)", path.display()),
            Resolution::Gone { path } => {
                println!("gone      {} (dropped from queue)", path.display())
            }
        }
        return Ok(());
    }
    let store = Store::open(&cfg.store.db_path())?;
    let items = store.list_pending()?;
    if items.is_empty() {
        println!("no pending files");
    }
    for p in items {
        let who = match p.source {
            PendingSource::Rule => format!("rule: {}", p.reason.as_deref().unwrap_or("")),
            PendingSource::Ai => format!(
                "AI{}: {}",
                pct(p.confidence),
                p.reason.as_deref().unwrap_or("")
            ),
        };
        println!(
            "#{:<4} {}  {} -> {}{}  [{}]{}",
            p.id,
            fmt_time(p.created_at),
            p.path.display(),
            p.suggested_dest
                .as_deref()
                .map(|d| d.display().to_string())
                .unwrap_or_default(),
            p.suggested_name
                .as_deref()
                .map(|n| format!(" (rename: {n})"))
                .unwrap_or_default(),
            who,
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

fn open_store(cfg: &Config) -> Result<Store> {
    let db = cfg.store.db_path();
    Store::open(&db).with_context(|| format!("opening {}", db.display()))
}

fn cmd_index(config: Option<PathBuf>, args: IndexArgs) -> Result<()> {
    let cfg = load_config(config)?;
    let store = open_store(&cfg)?;
    let verbose = tracing::enabled!(tracing::Level::INFO);

    if args.status {
        return print_index_status(&store, &cfg, args.format);
    }
    if args.forget {
        for p in &args.paths {
            let p = dunce::canonicalize(tidy_core::config::expand_home(p)).unwrap_or(p.clone());
            if p.is_file() {
                if !store.remove_file(&p)? {
                    eprintln!("not indexed: {}", p.display());
                    continue;
                }
            } else {
                store.remove_root(&p)?;
            }
            println!("forgot    {}", p.display());
        }
        return Ok(());
    }
    if args.rebuild {
        store.clear_index()?;
        eprintln!("# index cleared");
    }

    // (path, options) per root: explicit paths use the flags, configured
    // roots use their own settings (network roots → metadata only, small
    // hashes). A manual run ignores `content_index_window`.
    let base = IndexOptions::from_config(&cfg);
    let mut jobs: Vec<(PathBuf, IndexOptions)> = Vec::new();
    if args.paths.is_empty() {
        for (path, mut opts) in jobs_from_config(&cfg, true) {
            opts.index_content &= !args.no_content;
            jobs.push((path, opts));
        }
        if jobs.is_empty() {
            bail!("no folders given and no [[watch]] / [[index_only]] roots configured");
        }
    } else {
        for p in &args.paths {
            jobs.push((
                tidy_core::config::expand_home(p),
                IndexOptions {
                    recursive: !args.no_recursive,
                    index_content: !args.no_content,
                    ..base.clone()
                },
            ));
        }
    }

    eprintln!("# db: {}", cfg.store.db_path().display());
    let indexer = Indexer::new(&store);
    let mut reports = Vec::new();
    for (path, opts) in &jobs {
        if path.is_file() {
            let outcome = indexer
                .index_file(path, opts)
                .with_context(|| format!("indexing {}", path.display()))?;
            println!("{:<22} {}", outcome_tag(&outcome), path.display());
            continue;
        }
        if !path.is_dir() {
            eprintln!("skip      {} (no such file or folder)", path.display());
            continue;
        }
        let report = indexer
            .index_root(path, opts, |p, o| {
                if verbose {
                    eprintln!("{:<22} {}", outcome_tag(o), p.display());
                }
            })
            .with_context(|| format!("indexing {}", path.display()))?;
        if matches!(args.format, Format::Text) {
            print_index_report(&report);
        }
        reports.push(report);
    }
    if matches!(args.format, Format::Json) {
        println!("{}", serde_json::to_string_pretty(&reports)?);
    }
    Ok(())
}

fn outcome_tag(o: &FileOutcome) -> String {
    match o {
        FileOutcome::Added(s) => format!("added ({})", s.as_str()),
        FileOutcome::Updated(s) => format!("updated ({})", s.as_str()),
        FileOutcome::Unchanged => "unchanged".to_string(),
    }
}

fn print_index_report(r: &IndexReport) {
    println!(
        "indexed   {}  files {} (added {}, updated {}, unchanged {}, removed {})  \
         text ok {} / failed {} / unsupported {} / too big {}  in {:.1}s",
        r.root.display(),
        r.scanned,
        r.added,
        r.updated,
        r.unchanged,
        r.removed,
        r.text_ok,
        r.text_failed,
        r.text_unsupported,
        r.text_skipped,
        r.elapsed_ms as f64 / 1000.0
    );
    if !r.errors.is_empty() {
        eprintln!("{} entries could not be indexed:", r.errors.len());
        for e in r.errors.iter().take(10) {
            eprintln!("  {e}");
        }
    }
}

fn print_index_status(store: &Store, cfg: &Config, format: Format) -> Result<()> {
    let st = store.index_stats()?;
    match format {
        Format::Json => println!("{}", serde_json::to_string_pretty(&st)?),
        Format::Text => println!(
            "db: {}\nroots: {}   files: {}   with text: {}   total size: {}",
            cfg.store.db_path().display(),
            st.roots,
            st.files,
            st.with_text,
            format_size(st.total_bytes, BINARY)
        ),
    }
    Ok(())
}

fn cmd_search(config: Option<PathBuf>, args: SearchArgs) -> Result<()> {
    let cfg = load_config(config)?;
    let store = open_store(&cfg)?;
    let query = Query::parse(&args.query.join(" "));
    if query.is_empty() {
        bail!("empty query");
    }
    let opts = SearchOptions {
        limit: args.limit,
        ..Default::default()
    };
    let hits = search(&store, &query, &opts)?;
    match args.format {
        Format::Json => println!("{}", serde_json::to_string_pretty(&hits)?),
        Format::Text => {
            if hits.is_empty() {
                let st = store.index_stats()?;
                println!(
                    "no results ({} files indexed; run `tidy index <folder>` first?)",
                    st.files
                );
            }
            for (i, h) in hits.iter().enumerate() {
                let dir = h
                    .file
                    .path
                    .parent()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
                let tag = match h.matched_in {
                    MatchedIn::Name => "name",
                    MatchedIn::Content => "body",
                    MatchedIn::Both => "name+body",
                    MatchedIn::Filter => "filter",
                };
                println!(
                    "{:>2}. {}  {:>9}  {}  [{}]",
                    i + 1,
                    h.name_highlight,
                    format_size(h.file.size, BINARY),
                    fmt_time(h.file.mtime),
                    tag
                );
                println!("    {dir}");
                if !h.snippet.is_empty() {
                    println!("    {}", h.snippet.replace('\n', " "));
                }
            }
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
    for w in cfg.warnings() {
        eprintln!("# warning: {w}");
    }
    println!("{}", toml::to_string_pretty(&cfg)?);
    Ok(())
}

fn cmd_ai(config: Option<PathBuf>, cmd: AiCmd) -> Result<()> {
    match cmd {
        AiCmd::Key(KeyCmd::Set { key_ref }) => {
            let key = if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
                rpassword::prompt_password(format!("API key for {key_ref}: "))?
            } else {
                let mut line = String::new();
                std::io::stdin().read_line(&mut line)?;
                line
            };
            ai::keys::set_key(&key_ref, &key)?;
            // Read back to confirm without ever printing the key.
            if !ai::keys::has_key(&key_ref) {
                bail!("the keychain did not keep the key for {key_ref}");
            }
            println!("stored in the OS keychain: {key_ref}");
            Ok(())
        }
        AiCmd::Key(KeyCmd::Delete { key_ref }) => {
            ai::keys::delete_key(&key_ref)?;
            println!("removed: {key_ref}");
            Ok(())
        }
        AiCmd::Status => {
            let cfg = load_config(config)?;
            let a = &cfg.ai;
            println!(
                "enabled: {}   profile: {}   auto_threshold: {}   send_content: {}",
                a.enabled,
                a.active_profile,
                a.auto_threshold,
                a.send_content()
            );
            match a.active().and_then(|p| p.classify.as_ref()) {
                Some(pc) => {
                    println!(
                        "classify: {} / {}{}",
                        pc.provider.as_str(),
                        pc.model,
                        pc.base_url
                            .as_deref()
                            .map(|u| format!(" @ {u}"))
                            .unwrap_or_default()
                    );
                    if let Some(r) = &pc.key_ref {
                        println!(
                            "key {r}: {}",
                            if ai::keys::has_key(r) {
                                "stored"
                            } else {
                                "MISSING"
                            }
                        );
                    }
                }
                None => println!("classify: (no model in profile \"{}\")", a.active_profile),
            }
            let rules = RuleSet::load(&cfg.classify.rules).unwrap_or_default();
            match AiClassifier::from_config(&cfg, &rules) {
                Ok(_) => println!("state: ready"),
                Err(e) => println!("state: off — {e} (rules only)"),
            }
            if let Ok(store) = Store::open(&cfg.store.db_path()) {
                let (calls, failed, inp, out, cost) = store.ai_usage()?;
                println!(
                    "usage: {calls} calls ({failed} failed), {inp} in / {out} out tokens, ~${cost:.4}"
                );
            }
            Ok(())
        }
        AiCmd::Test { file, rules } => {
            let cfg = load_config(config)?;
            let rules = load_rules(&cfg, rules)?;
            let ai = AiClassifier::from_config(&cfg, &rules)?;
            let store = Store::open(&cfg.store.db_path())?;
            let path = tidy_core::config::expand_home(&file);
            let facts = FileFacts::from_path(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            let s = ai.suggest(&store, &facts)?;
            println!("{}", serde_json::to_string_pretty(&s)?);
            println!(
                "auto-move allowed: {}",
                if ai.auto_allowed(&s) {
                    "yes"
                } else {
                    "no (would ask)"
                }
            );
            Ok(())
        }
        AiCmd::Log { limit } => {
            let cfg = load_config(config)?;
            let store = Store::open(&cfg.store.db_path())?;
            for e in store.recent_ai_log(limit)? {
                println!(
                    "{}  {:<9} {:<20} {:<8} {:>6}/{:<5} tok {:>6} ms  {}{}",
                    fmt_time(e.called_at),
                    e.provider,
                    e.model,
                    e.purpose,
                    e.input_tokens.map(|v| v.to_string()).unwrap_or("-".into()),
                    e.output_tokens.map(|v| v.to_string()).unwrap_or("-".into()),
                    e.latency_ms.unwrap_or(0),
                    e.cost_usd.map(|c| format!("${c:.5} ")).unwrap_or_default(),
                    if e.ok {
                        e.result.unwrap_or_default()
                    } else {
                        format!("ERROR {}", e.error.unwrap_or_default())
                    }
                );
            }
            Ok(())
        }
    }
}

fn cmd_learn(config: Option<PathBuf>, args: LearnArgs) -> Result<()> {
    let cfg = load_config(config)?;
    let store = Store::open(&cfg.store.db_path())?;
    let rules = RuleSet::load(&cfg.classify.rules).unwrap_or_default();
    let threshold = cfg.classify.learn_threshold;
    let list = learner::suggestions(&store, &rules, threshold)?;
    let pick = |n: usize| {
        list.get(n.wrapping_sub(1))
            .with_context(|| format!("no suggestion #{n} (see `tidy learn`)"))
    };
    if let Some(n) = args.accept {
        let s = pick(n)?;
        learner::accept(&store, &cfg.classify.rules, threshold, &s.key)?;
        println!(
            "added to {}:\n{}",
            cfg.classify.rules.display(),
            s.rule_toml
        );
        println!("restart `tidy watch` (or save settings in the app) to use it");
        return Ok(());
    }
    if let Some(n) = args.dismiss {
        let s = pick(n)?;
        learner::dismiss(&store, &s.key)?;
        println!("dismissed: {}", s.rule_name);
        return Ok(());
    }
    if list.is_empty() {
        println!(
            "no suggestions yet (a pattern needs {threshold} identical decisions in the queue)"
        );
    }
    for (i, s) in list.iter().enumerate() {
        println!(
            "{}. {}  ({}회: {})\n{}",
            i + 1,
            s.rule_name,
            s.count,
            s.examples.join(", "),
            s.rule_toml
        );
    }
    Ok(())
}

fn default_folder(cfg: &Config, path: Option<PathBuf>) -> Result<PathBuf> {
    match path {
        Some(p) => Ok(tidy_core::config::expand_home(&p)),
        None => cfg
            .watches
            .first()
            .map(|w| w.path.clone())
            .context("no folder given and no [[watch]] folder configured"),
    }
}

fn cmd_report(config: Option<PathBuf>, args: ReportArgs) -> Result<()> {
    let cfg = load_config(config)?;
    let root = default_folder(&cfg, args.path)?;
    let store = Store::open(&cfg.store.db_path()).ok();
    let opts = report::ReportOptions {
        top_n: cfg.report.top_n,
        stale_days: args.stale_days.unwrap_or(cfg.report.stale_days),
        duplicates: (!args.no_dups).then(report::DupOptions::default),
        ..Default::default()
    };
    let r = report::build(&root, &opts, store.as_ref())
        .with_context(|| format!("reporting {}", root.display()))?;
    match args.format {
        ReportFormat::Json => println!("{}", serde_json::to_string_pretty(&r)?),
        ReportFormat::Html => print!("{}", report::html::render(&r, Some(args.depth))),
        ReportFormat::Text => {
            print!(
                "{}",
                render_tree(
                    &r.tree,
                    &TreeOptions {
                        max_depth: Some(args.depth),
                        ..Default::default()
                    }
                )
            );
            print_stats(&r.stats, r.stale_days);
            if !r.stats.stale.is_empty() {
                println!("\n── Long unused (>{}d) ───────────────────", r.stale_days);
                for f in &r.stats.stale {
                    println!(
                        "{}  {:>10}  {}",
                        fmt_time(f.last_used),
                        format_size(f.bytes, BINARY),
                        f.path
                    );
                }
            }
            println!(
                "\n── Duplicates ({} groups) ───────────────",
                r.duplicates.len()
            );
            for g in &r.duplicates {
                println!(
                    "{} wasted ({} × {})",
                    format_size(g.wasted, BINARY),
                    g.paths.len(),
                    format_size(g.size, BINARY)
                );
                for p in &g.paths {
                    println!("    {p}");
                }
            }
        }
    }
    Ok(())
}

fn protected_dirs(cfg: &Config, extra: &[PathBuf]) -> Vec<PathBuf> {
    cfg.report
        .protected
        .iter()
        .chain(extra.iter())
        .map(|p| {
            let p = tidy_core::config::expand_home(p);
            dunce::canonicalize(&p).unwrap_or(p)
        })
        .collect()
}

fn print_plan(plan: &restructure::Plan, only: Option<&[u32]>) -> Result<()> {
    println!(
        "plan #{} ({})  {}{}",
        plan.id,
        plan.source.as_str(),
        plan.root.display(),
        plan.applied_batch
            .as_deref()
            .map(|b| format!("  — APPLIED, batch {}", short(b)))
            .unwrap_or_default()
    );
    if !plan.summary.is_empty() {
        println!("{}", plan.summary);
    }
    for f in &plan.folders {
        println!("  folder {}: {}", f.path, f.reason);
    }
    println!(
        "\n── Moves ({}) ───────────────────────────",
        plan.moves.len()
    );
    for m in &plan.moves {
        let rel = |p: &Path| {
            p.strip_prefix(&plan.root)
                .map(|r| r.display().to_string())
                .unwrap_or_else(|_| p.display().to_string())
        };
        println!(
            "{:>3}. {}{} -> {}  [{}]{}",
            m.id,
            rel(&m.from),
            if m.is_dir {
                format!("/ ({} files)", m.files)
            } else {
                String::new()
            },
            rel(&m.to),
            m.reason,
            if m.warnings.is_empty() {
                String::new()
            } else {
                format!("  ⚠ {}", m.warnings.join("; "))
            }
        );
    }
    if !plan.rejected.is_empty() {
        println!(
            "\n── Rejected AI moves ({}) ─────────────────",
            plan.rejected.len()
        );
        for r in &plan.rejected {
            println!("  {} -> {}: {}", r.from, r.to, r.reason);
        }
    }
    let p = restructure::preview(plan, only)?;
    println!("\n── Before ──────────────────────────────\n{}", p.before);
    println!(
        "── After ({} files moved) ───────────────\n{}",
        p.files_moved, p.after
    );
    for (from, to) in &p.outside {
        println!(
            "  leaves this folder: {} -> {}",
            from.display(),
            to.display()
        );
    }
    for d in &p.emptied {
        println!("  becomes empty (left in place, nothing is deleted): {d}/");
    }
    Ok(())
}

fn cmd_restructure(config: Option<PathBuf>, args: RestructureArgs) -> Result<()> {
    let cfg = load_config(config)?;
    let store = Store::open(&cfg.store.db_path())?;
    let protected = protected_dirs(&cfg, &args.protect);
    let only = (!args.only.is_empty()).then_some(args.only.as_slice());

    if args.list {
        for p in store.recent_plans(20)? {
            println!(
                "#{:<4} {}  {:<5} {}{}",
                p.id,
                fmt_time(p.created_at),
                p.source,
                p.root.display(),
                p.applied_batch
                    .map(|b| format!("  applied (batch {})", short(&b)))
                    .unwrap_or_default()
            );
        }
        return Ok(());
    }
    if let Some(id) = args.show {
        return print_plan(&restructure::load(&store, id)?, only);
    }
    if let Some(id) = args.apply {
        let plan = restructure::load(&store, id)?;
        let mover = Mover::new(store, cfg.mover.duplicate_policy);
        let r = restructure::apply(
            &mover,
            &plan,
            only,
            &protected,
            Some(&IndexOptions::from_config(&cfg)),
        )?;
        for (a, b) in &r.moved {
            println!("moved     {} -> {}", a.display(), b.display());
        }
        for (p, why) in &r.skipped {
            println!("skipped   {} ({why})", p.display());
        }
        for (p, e) in &r.errors {
            eprintln!("error     {}: {e}", p.display());
        }
        println!(
            "\n{} moved in batch {} — undo all with: tidy undo --batch {}",
            r.moved.len(),
            short(&r.batch_id),
            short(&r.batch_id)
        );
        return Ok(());
    }

    let root = default_folder(&cfg, args.path)?;
    let plan = if args.ai {
        let rules = RuleSet::load(&cfg.classify.rules).unwrap_or_default();
        let ai = AiClassifier::from_config(&cfg, &rules)
            .context("the AI restructure needs [ai] enabled with a usable profile (try --rules)")?;
        eprintln!("# asking {} / {} …", ai.provider_name(), ai.model());
        restructure::plan_from_ai(
            &root,
            &ai,
            &store,
            &protected,
            Duration::from_secs(cfg.ai.restructure_timeout_secs),
            cfg.report.max_moves,
        )?
    } else if args.rules {
        let rules = load_rules(&cfg, None)?;
        restructure::plan_from_rules(
            &root,
            &rules,
            args.recursive,
            &cfg.classify.ignore_globs,
            &protected,
        )?
    } else {
        bail!("choose --rules or --ai (or --list / --show / --apply)");
    };
    let plan = restructure::save(&store, plan)?;
    print_plan(&plan, None)?;
    println!(
        "\nNothing has moved yet. Apply with: tidy restructure --apply {}{}",
        plan.id,
        if plan.moves.len() > 1 {
            "  [--only 1,2,...]"
        } else {
            ""
        }
    );
    Ok(())
}
