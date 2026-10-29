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
use tidy_core::mover::{DupTrash, ExpectedMoves, Mover, ProgressFn, TransferProgress, UndoReport};
use tidy_core::pending::{resolve, Decision, Resolution};
use tidy_core::report::{self, render_tree, restructure, scan, ScanOptions, Stats, TreeOptions};
use tidy_core::search::{
    hybrid_search, EmbedReport, MatchedIn, Query, SearchMode, SearchOptions, SemanticSearch,
};
use tidy_core::service::{run_embed_loop, run_watch, WatchEvent, WatchRunOptions};
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
    /// Search the index: words (FTS) plus meaning when an embed model is set.
    Search(SearchArgs),
    /// Compute embeddings for semantic search (`tidy index` does this too).
    Embed(EmbedArgs),
    /// (M15) Read the text in images and scanned PDFs so search finds them.
    Ocr(OcrArgs),
    /// (M16) Describe pictures with the AI's vision model (search + sorting).
    Vision(VisionArgs),
    /// AI status, API keys (OS keychain), test call, call log.
    #[command(subcommand)]
    Ai(AiCmd),
    /// Rules: how many files in the watch folders each rule takes now;
    /// the wizard's starter rules.
    #[command(subcommand)]
    Rules(RulesCmd),
    /// Rule suggestions learned from your decisions (--accept / --dismiss).
    Learn(LearnArgs),
    /// Folder report: tree, statistics, duplicates, long-unused files.
    Report(ReportArgs),
    /// Propose a reorganisation (rules or AI), show the diff, apply on approval.
    Restructure(RestructureArgs),
    /// (M14) Identical files (content, any name): list groups with the copy
    /// kept by default; --apply sends the other copies to the OS trash.
    Dedupe(DedupeArgs),
    /// (M10) Work-context curation: customer / dated issue / kind folders
    /// proposed by the AI, applied only on approval.
    #[command(subcommand)]
    Curate(CurateCmd),
    /// (M10) The work-context dictionary (customers, aliases, issue types).
    #[command(subcommand)]
    Context(ContextCmd),
    /// (M9) Find personal information in a folder (reads only, moves nothing).
    #[command(subcommand)]
    Pii(PiiCmd),
    /// (M9) Encrypted vault: lock a file into an AES-256 zip, unlock it,
    /// show the state, set the password (OS keychain only).
    #[command(subcommand)]
    Vault(VaultCmd),
    /// (M18) Run as the NAS agent: watch, organize and index the folders on
    /// this machine and answer the PC app over HTTP (Docker: see docs).
    Serve(ServeArgs),
    /// (M18) Talk to a NAS agent from this PC ([[nas]] in config.toml).
    #[command(subcommand)]
    Nas(NasCmd),
}

#[derive(Args)]
struct ServeArgs {
    /// Address to listen on.
    #[arg(long, env = "TIDY_LISTEN", default_value = "0.0.0.0:7841")]
    listen: String,
    /// The folder the PC may pick watch folders from (Docker: /data).
    #[arg(long, env = "TIDY_DATA_ROOT", default_value = "/data")]
    data_root: PathBuf,
    /// Print the connection password (made on first start) and exit.
    #[arg(long)]
    print_token: bool,
}

#[derive(Subcommand)]
enum NasCmd {
    /// Connection, folders, queue and today's moves.
    Status(NasTarget),
    /// Search the NAS index.
    Search {
        #[command(flatten)]
        target: NasTarget,
        query: String,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Files waiting for confirmation on the NAS.
    Pending(NasTarget),
    /// Recent move batches on the NAS.
    Log {
        #[command(flatten)]
        target: NasTarget,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
}

#[derive(Args)]
struct NasTarget {
    /// Which [[nas]] (default: the first).
    #[arg(long)]
    name: Option<String>,
    /// Address instead of a configured NAS (password from TIDY_TOKEN).
    #[arg(long)]
    url: Option<String>,
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
    /// Scan only N folder levels below the root (0 = direct children);
    /// for huge folders. Default: `report.scan_depth`, else unlimited.
    #[arg(long, value_name = "N")]
    scan_depth: Option<usize>,
}

#[derive(Args)]
struct DedupeArgs {
    /// Folder (default: the first [[watch]] folder). Its watch folder's
    /// organize_root is searched for copies too.
    path: Option<PathBuf>,
    /// Send the copies that are not kept to the OS trash (logged, one batch).
    #[arg(long)]
    apply: bool,
    /// Comma-separated group numbers for --apply, e.g. 1,3.
    #[arg(long, value_delimiter = ',', value_name = "N,..", requires = "apply")]
    only: Vec<usize>,
    /// Smallest file size to consider, in bytes.
    #[arg(long, default_value_t = 1024)]
    min_size: u64,
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
enum RulesCmd {
    /// Per rule: files in the watch folders it would take now (moves nothing).
    Check {
        /// Rules file (default: classify.rules from the config).
        rules: Option<PathBuf>,
        /// Count every rule a file matches, not only the first.
        #[arg(long)]
        each: bool,
        #[arg(short, long, value_enum, default_value_t = Format::Text)]
        format: Format,
    },
    /// List the wizard's starter rules (or print them as TOML with --toml).
    Starters {
        /// Print these starters (comma-separated ids) as rules.toml text.
        #[arg(long, value_delimiter = ',')]
        toml: Vec<String>,
    },
}

#[derive(Subcommand)]
enum CurateCmd {
    /// Analyse FOLDER (AI) and save a plan: 고객사/날짜_이슈/종류. Moves nothing.
    Scan {
        /// Default: the first [[watch]] folder.
        folder: Option<PathBuf>,
        /// Only show the cost estimate (files, tokens, price); no AI call.
        #[arg(long)]
        dry_run: bool,
    },
    /// Apply a saved work-context plan as one undoable batch.
    Apply {
        plan: i64,
        /// Comma-separated item numbers (default: every classified item).
        #[arg(long, value_delimiter = ',', value_name = "N,..")]
        only: Vec<u32>,
    },
}

#[derive(Subcommand)]
enum ContextCmd {
    /// Show customers (with aliases), issue types and kinds of material.
    List,
    /// Add a customer/project (or with --issue an issue type, --kind a kind).
    Add {
        name: String,
        /// Aliases for a customer, comma-separated (e.g. GARA,가람).
        #[arg(long, value_delimiter = ',')]
        alias: Vec<String>,
        #[arg(long, conflicts_with = "kind")]
        issue: bool,
        #[arg(long)]
        kind: bool,
        /// --kind: extensions that usually mean it (comma-separated).
        #[arg(long, value_delimiter = ',', requires = "kind")]
        ext: Vec<String>,
    },
    /// Add an alias to a customer: `tidy context alias 가람정보 GARA`.
    Alias { entity: String, alias: String },
}

#[derive(Subcommand)]
enum PiiCmd {
    /// Judge every file under FOLDER: 주민번호, 계좌번호, 이름+휴대전화,
    /// 이름+이메일 ([security.pii] settings). Nothing is written.
    Scan {
        folder: PathBuf,
        /// Only the folder itself, not its subfolders.
        #[arg(long)]
        no_recursive: bool,
        /// Also list files without personal information.
        #[arg(long)]
        all: bool,
        #[arg(short, long, value_enum, default_value_t = Format::Text)]
        format: Format,
    },
}

#[derive(Subcommand)]
enum VaultCmd {
    /// Lock FILE into `<name>.zip` (AES-256) in --to, the vault folder, or
    /// next to it; logged and undoable like any move.
    Lock {
        file: PathBuf,
        #[arg(long)]
        to: Option<PathBuf>,
    },
    /// Decrypt a zip Tidy locked back to where the file came from (undo).
    Unlock { zip: PathBuf },
    /// Password, mode, vault folder, locked files, personal files in the index.
    Status,
    /// Set or change the vault password (old ones are kept for old zips).
    Password,
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
    /// Do not compute embeddings afterwards (semantic search).
    #[arg(long)]
    no_embed: bool,
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
    /// Words only: no semantic ranking even if an embed model is configured.
    #[arg(long)]
    fts: bool,
    /// Output format.
    #[arg(short, long, value_enum, default_value_t = Format::Text)]
    format: Format,
}

#[derive(Args)]
struct EmbedArgs {
    /// Drop every stored vector first and embed everything again.
    #[arg(long)]
    rebuild: bool,
    /// Show how many files have a current vector and exit.
    #[arg(long)]
    status: bool,
    /// Embed at most this many files in this run.
    #[arg(long, value_name = "N")]
    limit: Option<usize>,
}

#[derive(Args)]
struct OcrArgs {
    /// Recognize this file only and print the text (the index is not changed).
    file: Option<PathBuf>,
    /// Show the engine and how many files wait / are done, and exit.
    #[arg(long, conflicts_with = "file")]
    status: bool,
    /// Put files that failed back in the queue first.
    #[arg(long, conflicts_with = "file")]
    retry: bool,
    /// Recognize at most this many files in this run.
    #[arg(long, value_name = "N", conflicts_with = "file")]
    limit: Option<usize>,
}

#[derive(Args)]
struct VisionArgs {
    /// Describe this picture only and print the answer (the index is not changed).
    file: Option<PathBuf>,
    /// Show the model and how many pictures wait / are done, and exit.
    #[arg(long, conflicts_with = "file")]
    status: bool,
    /// Put pictures that failed back in the queue first.
    #[arg(long, conflicts_with = "file")]
    retry: bool,
    /// Describe at most this many pictures in this run (the daily limit still applies).
    #[arg(long, value_name = "N", conflicts_with = "file")]
    limit: Option<usize>,
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
    /// (M14) If an identical file already has the name at the destination:
    /// skip | rename | replace (default: [mover] duplicate_policy).
    #[arg(long, requires = "move", value_name = "POLICY", value_parser = parse_dup_policy)]
    on_duplicate: Option<tidy_core::config::DuplicatePolicy>,
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
    // The agent logs what it does by default (docker logs).
    let serving = matches!(cli.cmd, Cmd::Serve(_));
    init_logging(if serving {
        cli.verbose.max(1)
    } else {
        cli.verbose
    });

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
        Cmd::Embed(args) => cmd_embed(cli.config, args),
        Cmd::Ocr(args) => cmd_ocr(cli.config, args),
        Cmd::Vision(args) => cmd_vision(cli.config, args),
        Cmd::Ai(cmd) => cmd_ai(cli.config, cmd),
        Cmd::Rules(cmd) => cmd_rules(cli.config, cmd),
        Cmd::Learn(args) => cmd_learn(cli.config, args),
        Cmd::Report(args) => cmd_report(cli.config, args),
        Cmd::Dedupe(args) => cmd_dedupe(cli.config, args),
        Cmd::Restructure(args) => cmd_restructure(cli.config, args),
        Cmd::Pii(cmd) => cmd_pii(cli.config, cmd),
        Cmd::Curate(cmd) => cmd_curate(cli.config, cmd),
        Cmd::Context(cmd) => cmd_context(cli.config, cmd),
        Cmd::Vault(cmd) => cmd_vault(cli.config, cmd),
        Cmd::Serve(args) => cmd_serve(cli.config, args),
        Cmd::Nas(cmd) => cmd_nas(cli.config, cmd),
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
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
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
    if !args.no_ai && !args.no_index && !args.dry_run {
        spawn_embed_thread(&cfg);
    }
    if !args.no_index && !args.dry_run {
        spawn_ocr_thread(&cfg);
    }
    if !args.no_ai && !args.no_index && !args.dry_run {
        spawn_vision_thread(&cfg);
    }
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
        WatchEvent::AiHealth(ai::AiAvailability::Unavailable, e) => eprintln!(
            "# ai: unavailable after repeated failures — rules only, retrying later ({})",
            e.unwrap_or_default()
        ),
        WatchEvent::AiHealth(ai::AiAvailability::Ready, _) => {
            eprintln!("# ai: reachable again")
        }
        WatchEvent::Handled(path, res) => report_action(res, path),
    })?;
    Ok(())
}

/// Background embedding next to `tidy watch` (when an embed model is set).
fn spawn_embed_thread(cfg: &Config) {
    let sem = match SemanticSearch::from_config(cfg) {
        Ok(s) => s,
        Err(ai::AiError::Disabled | ai::AiError::NoEmbedModel(_)) => return,
        Err(e) => {
            eprintln!("# semantic search off: {e}");
            return;
        }
    };
    eprintln!("# embeddings: {} (background)", sem.describe());
    let cfg = cfg.clone();
    std::thread::spawn(move || {
        let stop = AtomicBool::new(false); // Ctrl+C ends the process
        let res = run_embed_loop(&cfg, &sem, &stop, Duration::from_secs(60), |r| {
            if let Some(e) = &r.error {
                eprintln!("# embeddings: {e} (words-only search until it works)");
            } else {
                eprintln!(
                    "# embeddings: {} new, {} reused, {} waiting",
                    r.embedded, r.reused, r.remaining
                );
            }
        });
        if let Err(e) = res {
            eprintln!("# embeddings stopped: {e}");
        }
    });
}

fn report_action(res: &Result<Action, tidy_core::engine::EngineError>, path: &Path) {
    match res {
        Ok(Action::Moved {
            src,
            dst,
            rule,
            batch_id,
            confidence,
            encrypted,
            ..
        }) => println!(
            "{} {} -> {}  [{}{}] batch {}",
            if *encrypted { "locked   " } else { "moved    " },
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
            encrypt,
            pii,
            ..
        }) => println!(
            "pending   {} (suggested {}{})  [{}{}] {}{}",
            path.display(),
            suggested.display(),
            if *encrypt { ", encrypt" } else { "" },
            rule,
            pct(*confidence),
            reason.as_deref().unwrap_or("confirm"),
            pii.as_ref()
                .map(|f| format!("  개인정보: {}", f.summary()))
                .unwrap_or_default()
        ),
        Ok(Action::WouldMove {
            path,
            rule,
            dest_dir,
            confirm,
            encrypt,
        }) => println!(
            "would {}  {} -> {}{}  [{}]",
            if *confirm { "ask " } else { "move" },
            path.display(),
            dest_dir.display(),
            if *encrypt { " (encrypted .zip)" } else { "" },
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
            if m.op == tidy_core::store::MoveOp::Trash {
                format!("trash (kept {})", m.dst_path.display())
            } else {
                m.dst_path.display().to_string()
            },
            match (m.undone_at, m.in_progress) {
                (Some(t), _) => format!("  (undone {})", fmt_time(t)),
                (None, true) => "  (in progress / interrupted)".into(),
                (None, false) => String::new(),
            }
        );
    }
    Ok(())
}

fn parse_dup_policy(s: &str) -> std::result::Result<tidy_core::config::DuplicatePolicy, String> {
    use tidy_core::config::DuplicatePolicy::*;
    match s {
        "skip" => Ok(Skip),
        "rename" => Ok(Rename),
        "replace" => Ok(Replace),
        _ => Err("skip | rename | replace".into()),
    }
}

fn cmd_pending(config: Option<PathBuf>, args: PendingArgs) -> Result<()> {
    let cfg = load_config(config)?;
    let decision = if let Some(id) = args.r#move {
        Some((
            id,
            Decision::Move {
                dest_dir: args.to.map(|p| tidy_core::config::expand_home(&p)),
                new_name: args.rename,
                on_duplicate: args.on_duplicate,
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
    let anchors = tidy_core::classify::Anchors::from_config(&cfg);
    for p in &args.paths {
        let p = tidy_core::config::expand_home(p);
        if !p.is_file() {
            continue;
        }
        let p = dunce::canonicalize(&p).unwrap_or(p);
        let facts = FileFacts::from_path(&p).with_context(|| format!("reading {}", p.display()))?;
        match rules.classify_in(&facts, anchors.for_path(&p)) {
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
    if !args.no_embed {
        // Semantic search is optional: no model or a failing one only
        // leaves a note, the index itself is done (CLAUDE.md rule 4).
        match SemanticSearch::from_config(&cfg) {
            Ok(sem) => {
                let r = embed_with_progress(&sem, &store, usize::MAX)?;
                print_embed_report(&sem, &r);
            }
            Err(ai::AiError::Disabled | ai::AiError::NoEmbedModel(_)) => {}
            Err(e) => eprintln!("# embeddings skipped: {e}"),
        }
    }
    Ok(())
}

fn embed_with_progress(sem: &SemanticSearch, store: &Store, limit: usize) -> Result<EmbedReport> {
    let total = store.embed_backlog(sem.model_key())?;
    if total == 0 {
        return Ok(EmbedReport::default());
    }
    eprintln!("# embedding {total} files with {}", sem.describe());
    let mut last = 0;
    Ok(sem.embed_pending(store, limit, |done| {
        if done >= last + 200 {
            last = done;
            eprintln!("# embedded {done} / {total}");
        }
    })?)
}

fn print_embed_report(sem: &SemanticSearch, r: &EmbedReport) {
    if r.embedded + r.reused > 0 || r.error.is_some() {
        println!(
            "embedded  {} new, {} reused  ({} waiting)  [{}]",
            r.embedded,
            r.reused,
            r.remaining,
            sem.describe()
        );
    }
    if let Some(e) = &r.error {
        eprintln!("# embedding stopped: {e} — search works with words only until it succeeds");
    }
}

fn cmd_embed(config: Option<PathBuf>, args: EmbedArgs) -> Result<()> {
    let cfg = load_config(config)?;
    let store = open_store(&cfg)?;
    let sem = SemanticSearch::from_config(&cfg)
        .context("semantic search is not configured ([ai] enabled + profile `embed` model)")?;
    if args.status {
        let st = store.vector_stats(sem.model_key())?;
        println!(
            "model: {}   files with a current vector: {} / {}   stored vectors: {}",
            sem.describe(),
            st.embedded,
            st.files,
            st.vectors
        );
        return Ok(());
    }
    if args.rebuild {
        store.clear_vectors()?;
        eprintln!("# embeddings cleared");
    }
    let r = embed_with_progress(&sem, &store, args.limit.unwrap_or(usize::MAX))?;
    print_embed_report(&sem, &r);
    if r.embedded + r.reused == 0 && r.error.is_none() {
        println!("nothing to embed ({} waiting)", r.remaining);
    }
    if r.error.is_some() {
        std::process::exit(1);
    }
    Ok(())
}

fn cmd_ocr(config: Option<PathBuf>, args: OcrArgs) -> Result<()> {
    use tidy_core::index::ocr;
    let cfg = load_config(config)?;
    if let Some(file) = &args.file {
        let engine = ocr::build_engine(&cfg.ocr)?;
        eprintln!("# {}", engine.describe());
        let text = ocr::recognize(engine.as_ref(), file, &cfg.ocr)?;
        if text.trim().is_empty() {
            eprintln!("# no text found");
        } else {
            println!("{text}");
        }
        return Ok(());
    }
    let store = open_store(&cfg)?;
    if args.status {
        let st = ocr::status(&cfg, &store)?;
        println!(
            "ocr: {}   engine: {}",
            if st.enabled {
                "on"
            } else {
                "off ([ocr] enabled = false)"
            },
            st.engine.as_deref().unwrap_or("none")
        );
        if let Some(n) = &st.note {
            println!("note: {n}");
        }
        let c = &st.counts;
        println!(
            "waiting: {}   done: {}   no text: {}   failed: {}",
            c.pending, c.ok, c.empty, c.failed
        );
        return Ok(());
    }
    if args.retry {
        let n = store.ocr_retry_failed()?;
        eprintln!("# {n} failed file(s) queued again");
    }
    let engine = ocr::build_engine(&cfg.ocr)?;
    eprintln!("# {}", engine.describe());
    let r = ocr::run_pass(
        &cfg,
        &store,
        engine.as_ref(),
        args.limit.unwrap_or(usize::MAX),
        |p| eprintln!("  {}", p.display()),
    )?;
    println!(
        "recognized {}   no text {}   failed {}   still waiting {}",
        r.recognized, r.empty, r.failed, r.remaining
    );
    Ok(())
}

/// Background text recognition next to `tidy watch` (`[ocr] enabled`).
fn spawn_ocr_thread(cfg: &Config) {
    if !cfg.ocr.enabled {
        return;
    }
    let cfg = cfg.clone();
    std::thread::spawn(move || {
        let stop = AtomicBool::new(false); // Ctrl+C ends the process
        let res =
            tidy_core::service::run_ocr_loop(&cfg, &stop, Duration::from_secs(60), |r| match r {
                Ok(r) => eprintln!(
                    "# ocr: {} recognized, {} no text, {} failed, {} waiting",
                    r.recognized, r.empty, r.failed, r.remaining
                ),
                Err(e) => eprintln!("# ocr: {e}"),
            });
        if let Err(e) = res {
            eprintln!("# ocr stopped: {e}");
        }
    });
}

fn cmd_vision(config: Option<PathBuf>, args: VisionArgs) -> Result<()> {
    use tidy_core::index::vision;
    let cfg = load_config(config)?;
    let store = open_store(&cfg)?;
    if let Some(file) = &args.file {
        let d = vision::VisionDescriber::from_config(&cfg)?;
        eprintln!("# {} / {}", d.provider_name(), d.model());
        let name = file
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let c = d.describe(&store, file, &name)?;
        if c.is_empty() {
            eprintln!("# nothing to describe");
        } else {
            println!("{}", c.line());
            if let Some(k) = &c.kind {
                eprintln!("# kind: {k}");
            }
        }
        return Ok(());
    }
    if args.status {
        let st = vision::status(&cfg, &store)?;
        println!(
            "vision: {}   model: {}{}",
            if st.enabled {
                "on"
            } else {
                "off ([vision] enabled = false)"
            },
            st.model.as_deref().unwrap_or("none"),
            if st.local { " (local)" } else { "" }
        );
        if let Some(n) = &st.note {
            println!("note: {n}");
        }
        let c = &st.counts;
        println!(
            "waiting: {}   described: {}   nothing to describe: {}   failed: {}   personal (not sent): {}",
            c.pending, c.ok, c.empty, c.failed, c.skipped
        );
        println!(
            "today: {} / {}   total sent: {}   est. cost: ${:.4}",
            st.today, st.daily_limit, st.total, st.cost_usd
        );
        return Ok(());
    }
    if args.retry {
        let n = store.vision_retry_failed()?;
        eprintln!("# {n} failed picture(s) queued again");
    }
    let d = vision::VisionDescriber::from_config(&cfg)?;
    eprintln!("# {} / {}", d.provider_name(), d.model());
    let wait_for_ocr = cfg.ocr.enabled && tidy_core::index::ocr::build_engine(&cfg.ocr).is_ok();
    let r = vision::run_pass(
        &cfg,
        &store,
        &d,
        args.limit.unwrap_or(usize::MAX),
        wait_for_ocr,
        |p| eprintln!("  {}", p.display()),
    )?;
    println!(
        "described {}   nothing to describe {}   failed {}   personal (not sent) {}   still waiting {}",
        r.described, r.empty, r.failed, r.skipped, r.remaining
    );
    if r.limited {
        println!(
            "daily limit reached ([vision] daily_limit = {})",
            cfg.vision.daily_limit
        );
    }
    if let Some(e) = &r.stopped {
        println!("stopped: {e}");
    }
    Ok(())
}

/// Background picture descriptions next to `tidy watch` (`[vision] enabled`).
fn spawn_vision_thread(cfg: &Config) {
    if !cfg.vision.enabled {
        return;
    }
    let cfg = cfg.clone();
    std::thread::spawn(move || {
        let stop = AtomicBool::new(false); // Ctrl+C ends the process
        let res = tidy_core::service::run_vision_loop(&cfg, &stop, Duration::from_secs(60), |r| {
            match r {
                Ok(r) => eprintln!(
                    "# vision: {} described, {} nothing to describe, {} failed, {} personal, {} waiting",
                    r.described, r.empty, r.failed, r.skipped, r.remaining
                ),
                Err(e) => eprintln!("# vision: {e}"),
            }
        });
        if let Err(e) = res {
            eprintln!("# vision stopped: {e}");
        }
    });
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
    let sem = if args.fts {
        None
    } else {
        match SemanticSearch::from_config(&cfg) {
            Ok(s) => Some(s),
            Err(ai::AiError::Disabled | ai::AiError::NoEmbedModel(_)) => None,
            Err(e) => {
                eprintln!("# semantic search off: {e} (words only)");
                None
            }
        }
    };
    let out = hybrid_search(&store, sem.as_ref(), &query, &opts)?;
    if let Some(n) = &out.note {
        eprintln!("# {n}");
    }
    let hits = &out.hits;
    match args.format {
        Format::Json => println!("{}", serde_json::to_string_pretty(&out)?),
        Format::Text => {
            if let (SearchMode::Hybrid, Some(s)) = (out.mode, &sem) {
                eprintln!(
                    "# words + meaning ({}){}",
                    s.describe(),
                    if out.natural {
                        ", question-like query"
                    } else {
                        ""
                    }
                );
            }
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
                    MatchedIn::Semantic => "meaning",
                };
                let tag = match h.similarity {
                    Some(sim) => format!("{tag} {:.0}%", sim * 100.0),
                    None => tag.to_string(),
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

fn cmd_rules(config: Option<PathBuf>, cmd: RulesCmd) -> Result<()> {
    use tidy_core::classify::{preview, starter};
    match cmd {
        RulesCmd::Check {
            rules,
            each,
            format,
        } => {
            let cfg = load_config(config)?;
            let rules = load_rules(&cfg, rules)?;
            if cfg.watches.is_empty() {
                bail!("no [[watch]] folders configured");
            }
            let r = preview::preview(&rules, &cfg.watches, &cfg.ignore_filter()?, !each);
            match format {
                Format::Json => println!("{}", serde_json::to_string_pretty(&r)?),
                Format::Text => {
                    for p in &r.rules {
                        println!(
                            "{:>5}  {}{}",
                            p.count,
                            p.name,
                            p.sample_dest
                                .as_ref()
                                .map(|d| format!("  → {}", d.display()))
                                .unwrap_or_default()
                        );
                        if !p.examples.is_empty() {
                            println!("       예: {}", p.examples.join(", "));
                        }
                    }
                    println!(
                        "files: {}{}   no rule: {}",
                        r.scanned,
                        if r.truncated { "+ (stopped)" } else { "" },
                        if each {
                            "-".to_string()
                        } else {
                            r.unmatched.to_string()
                        }
                    );
                    for p in &r.skipped {
                        println!("skipped (network folder): {}", p.display());
                    }
                    for p in &r.missing {
                        println!("missing: {}", p.display());
                    }
                }
            }
            Ok(())
        }
        RulesCmd::Starters { toml } => {
            if toml.is_empty() {
                for s in starter::starters() {
                    println!("{:<12} {:<8} {}", s.id, s.title, s.summary);
                }
            } else {
                print!(
                    "{}",
                    tidy_core::classify::apply_cards("", &starter::starter_cards(&toml))?
                );
            }
            Ok(())
        }
    }
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
            println!("stored in {}: {key_ref}", ai::keys::store_name());
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
                            "key {r}: {}  (store: {})",
                            if ai::keys::has_key(r) {
                                "stored"
                            } else {
                                "MISSING"
                            },
                            ai::keys::store_name()
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
            match SemanticSearch::from_config(&cfg) {
                Ok(sem) => {
                    let st = Store::open(&cfg.store.db_path())
                        .ok()
                        .and_then(|s| s.vector_stats(sem.model_key()).ok())
                        .unwrap_or_default();
                    println!(
                        "embed: {}   files with vectors: {} / {}",
                        sem.describe(),
                        st.embedded,
                        st.files
                    );
                }
                Err(e) => println!("embed: off — {e} (search uses words only)"),
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

fn cmd_dedupe(config: Option<PathBuf>, args: DedupeArgs) -> Result<()> {
    let cfg = load_config(config)?;
    let root = default_folder(&cfg, args.path)?;
    let store = Store::open(&cfg.store.db_path())?;
    let res = report::scan(
        &root,
        &ScanOptions {
            max_depth: cfg.report.scan_depth,
            ..Default::default()
        },
    )
    .with_context(|| format!("scanning {}", root.display()))?;
    let opts = report::DupOptions {
        min_size: args.min_size,
        ..report::DupOptions::for_root(&root, &cfg.watches)
    };
    let groups = report::find_duplicates(&res.root, &res.tree, &opts, Some(&store));
    if groups.is_empty() {
        println!("no identical files in {}", root.display());
        return Ok(());
    }
    let picked = |i: usize| args.only.is_empty() || args.only.contains(&i);
    let mut wasted = 0;
    for (i, g) in groups.iter().enumerate() {
        let n = i + 1;
        if !picked(n) {
            continue;
        }
        wasted += g.wasted;
        println!(
            "#{n}  {} × {}  (saves {})",
            format_size(g.size, BINARY),
            g.files.len(),
            format_size(g.wasted, BINARY)
        );
        for (k, f) in g.files.iter().enumerate() {
            println!(
                "    {} {}{}",
                if k == g.keep { "keep " } else { "trash" },
                f.path.display(),
                if f.in_organize_root {
                    "  [정리 위치]"
                } else {
                    ""
                }
            );
        }
    }
    if !args.apply {
        println!(
            "\n{} group(s), {} could be freed. Nothing was moved; \
             `tidy dedupe --apply [--only 1,3]` sends the \"trash\" copies to the OS trash.",
            groups.len(),
            format_size(wasted, BINARY)
        );
        return Ok(());
    }
    let req: Vec<DupTrash> = groups
        .iter()
        .enumerate()
        .filter(|(i, _)| picked(i + 1))
        .map(|(_, g)| DupTrash {
            hash: g.hash.clone(),
            keep: g.files[g.keep].path.clone(),
            trash: g
                .files
                .iter()
                .enumerate()
                .filter(|(k, _)| *k != g.keep)
                .map(|(_, f)| f.path.clone())
                .collect(),
        })
        .collect();
    let mover = Mover::new(store, cfg.mover.duplicate_policy);
    let batch = new_batch_id();
    let r = mover.trash_duplicates(&req, &batch);
    println!(
        "sent {} file(s) to the trash ({} freed), batch {}",
        r.trashed.len(),
        format_size(r.bytes(), BINARY),
        short(&batch)
    );
    for (p, why) in &r.failed {
        println!("  left {}: {why}", p.display());
    }
    println!("(undo cannot restore from the OS trash: restore them there if needed)");
    Ok(())
}

fn cmd_report(config: Option<PathBuf>, args: ReportArgs) -> Result<()> {
    let cfg = load_config(config)?;
    let root = default_folder(&cfg, args.path)?;
    let store = Store::open(&cfg.store.db_path()).ok();
    let opts = report::ReportOptions {
        top_n: cfg.report.top_n,
        stale_days: args.stale_days.unwrap_or(cfg.report.stale_days),
        duplicates: (!args.no_dups).then(|| report::DupOptions::for_root(&root, &cfg.watches)),
        scan: ScanOptions {
            max_depth: args.scan_depth.or(cfg.report.scan_depth),
            ..Default::default()
        },
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
            if let Some(p) = &r.pii {
                println!("🔒 {} (검색 인덱스 기준)", p.label());
            }
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
        if let Some(n) = &m.context {
            let who = n.entity.clone().unwrap_or_else(|| {
                format!("(고객사 없음: {})", n.topic.clone().unwrap_or_default())
            });
            println!(
                "{:>3}. {}  {}",
                m.id,
                rel(&m.from),
                if n.unclassified {
                    format!(
                        "미분류 — 그대로 둠 ({:.0}%: {})",
                        n.confidence * 100.0,
                        n.reason
                    )
                } else {
                    format!(
                        "-> {}{}\n       {who} · {} · {} · {} · {:.0}% — {}",
                        rel(&m.to),
                        if m.encrypt { " 🔒" } else { "" },
                        n.issue,
                        n.date,
                        n.doc_kind,
                        n.confidence * 100.0,
                        n.reason
                    )
                }
            );
            continue;
        }
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
        let anchors = tidy_core::classify::Anchors::from_config(&cfg);
        let canon = dunce::canonicalize(&root).unwrap_or_else(|_| root.clone());
        restructure::plan_from_rules_in(
            &root,
            &rules,
            args.recursive,
            &cfg.ignore_filter()?,
            &protected,
            Some(anchors.for_path(&canon).unwrap_or(&canon)),
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

// ── M9: personal information + vault ───────────────────────────────────

fn cmd_pii(config: Option<PathBuf>, cmd: PiiCmd) -> Result<()> {
    let PiiCmd::Scan {
        folder,
        no_recursive,
        all,
        format,
    } = cmd;
    let cfg = load_config(config)?;
    let detector = tidy_core::pii::Detector::lenient(&cfg.security.pii);
    if !detector.enabled() {
        bail!("[security.pii] enabled = false — 개인정보 탐지가 꺼져 있습니다");
    }
    let filter = tidy_core::watcher::IgnoreFilter::new(&cfg.classify.ignore_globs)?;
    let max_bytes = cfg.index.max_content_size_mb * 1024 * 1024;
    let mut rows = Vec::new();
    let (mut pii, mut clean, mut unknown) = (0u64, 0u64, 0u64);
    let walker = walkdir::WalkDir::new(&folder)
        .min_depth(1)
        .max_depth(if no_recursive { 1 } else { usize::MAX })
        .into_iter()
        .filter_entry(|e| {
            let n = e.file_name().to_string_lossy();
            !n.starts_with('.') && !n.contains(tidy_core::mover::TMP_MARKER)
        });
    for e in walker.filter_map(Result::ok) {
        if !e.file_type().is_file() || filter.is_ignored(e.path()) {
            continue;
        }
        let f = detector.judge_path(e.path(), max_bytes, cfg.index.max_text_chars);
        match f.status {
            tidy_core::pii::PiiStatus::Pii => pii += 1,
            tidy_core::pii::PiiStatus::Clean => clean += 1,
            tidy_core::pii::PiiStatus::Unknown => unknown += 1,
        }
        if all || f.is_pii() {
            rows.push((e.path().to_path_buf(), f));
        }
    }
    match format {
        Format::Json => {
            let items: Vec<_> = rows
                .iter()
                .map(|(p, f)| {
                    serde_json::json!({
                        "path": p, "status": f.status, "flags": f.flags,
                        "counts": f.counts, "summary": f.summary(),
                    })
                })
                .collect();
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "personal": pii, "clean": clean, "unknown": unknown, "files": items,
                }))?
            );
        }
        _ => {
            for (p, f) in &rows {
                let tag = match f.status {
                    tidy_core::pii::PiiStatus::Pii => "🔒 개인정보",
                    tidy_core::pii::PiiStatus::Clean => "   없음    ",
                    tidy_core::pii::PiiStatus::Unknown => "   판정 불가",
                };
                println!("{tag}  {}  {}", p.display(), f.summary());
            }
            println!(
                "개인정보 의심 {pii}개 · 없음 {clean}개 · 판정 불가(본문 없음) {unknown}개 — 파일은 그대로입니다"
            );
        }
    }
    Ok(())
}

fn read_password(prompt: &str) -> Result<String> {
    if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        Ok(rpassword::prompt_password(prompt)?)
    } else {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        Ok(line.trim_end_matches(['\r', '\n']).to_string())
    }
}

fn cmd_vault(config: Option<PathBuf>, cmd: VaultCmd) -> Result<()> {
    use tidy_core::security;
    let cfg = load_config(config)?;
    match cmd {
        VaultCmd::Password => {
            let pw = read_password("새 보관 비밀번호: ")?;
            if std::io::IsTerminal::is_terminal(&std::io::stdin())
                && read_password("한 번 더: ")? != pw
            {
                bail!("두 비밀번호가 다릅니다");
            }
            security::set_vault_password(&pw)?;
            let st = security::vault_key_status();
            if !st.has_password {
                bail!("the keychain did not keep the vault password");
            }
            println!(
                "stored in {}: {} (이전 비밀번호 {}개 보관)",
                st.store,
                security::vault_key_ref(),
                st.previous
            );
            Ok(())
        }
        VaultCmd::Status => {
            let st = security::vault_key_status();
            let s = &cfg.security;
            println!(
                "password: {}  (store: {}{})",
                if st.has_password { "set" } else { "NOT SET" },
                st.store,
                if st.previous > 0 {
                    format!(", 이전 비밀번호 {}개", st.previous)
                } else {
                    String::new()
                }
            );
            if let Some(e) = st.error {
                println!("keychain error: {e}");
            }
            println!(
                "mode: {}   approval: {}   vault_dir: {}",
                s.mode.as_str(),
                s.approval.as_str(),
                s.vault_dir
                    .as_deref()
                    .map(|d| d.display().to_string())
                    .unwrap_or_else(|| "(없음)".into())
            );
            let mover = open_mover(&cfg)?;
            let store = mover.store();
            let (n, open) = store.pii_totals(None)?;
            println!(
                "locked zips: {}   개인정보 의심 파일(인덱스): {n}개 (미암호화 {open}개)",
                store.active_encrypted()?
            );
            for w in cfg.warnings() {
                println!("# warning: {w}");
            }
            Ok(())
        }
        VaultCmd::Lock { file, to } => {
            let file = dunce::canonicalize(&file)
                .with_context(|| format!("no such file: {}", file.display()))?;
            let dest = to
                .or_else(|| cfg.security.collects().map(Path::to_path_buf))
                .or_else(|| file.parent().map(Path::to_path_buf))
                .context("no destination folder")?;
            let mover = open_mover(&cfg)?;
            let opts = IndexOptions::from_config(&cfg);
            tidy_core::engine::prepare_vault_index(mover.store(), &file, &opts);
            let batch = new_batch_id();
            let out = mover.move_file(&tidy_core::mover::MoveRequest {
                src: file.clone(),
                dest_dir: dest,
                batch_id: batch.clone(),
                decided_by: tidy_core::store::DecidedBy::User,
                rule_name: Some("vault lock".into()),
                ai_confidence: None,
                new_name: None,
                encrypt: true,
                on_duplicate: None,
            })?;
            match out {
                tidy_core::mover::MoveOutcome::Moved { log_id, src, dst } => {
                    tidy_core::engine::reindex_for(
                        mover.store(),
                        &Action::Moved {
                            src: src.clone(),
                            dst: dst.clone(),
                            rule: "vault lock".into(),
                            batch_id: batch.clone(),
                            log_id,
                            by: tidy_core::store::DecidedBy::User,
                            confidence: None,
                            encrypted: true,
                        },
                        &opts,
                    );
                    println!(
                        "locked    {} -> {}  batch {} (tidy undo --batch {} / tidy vault unlock)",
                        src.display(),
                        dst.display(),
                        short(&batch),
                        short(&batch)
                    );
                }
                other => println!("{other:?}"),
            }
            Ok(())
        }
        VaultCmd::Unlock { zip } => {
            let zip = dunce::canonicalize(&zip)
                .with_context(|| format!("no such file: {}", zip.display()))?;
            let mover = open_mover(&cfg)?;
            let Some(e) = mover.store().active_move_to(&zip)? else {
                bail!(
                    "{}: Tidy가 잠근 기록이 없습니다 — 반디집·7-Zip 같은 AES 지원 압축 프로그램으로 여세요",
                    zip.display()
                );
            };
            if e.op != tidy_core::store::MoveOp::Encrypt {
                bail!("{}: 암호화 기록이 아닙니다", zip.display());
            }
            let r = mover.undo_one(e.id)?;
            reindex_undo(mover.store(), &r, &IndexOptions::from_config(&cfg));
            for (from, to) in &r.restored {
                println!("unlocked  {} -> {}", from.display(), to.display());
            }
            for c in &r.conflicts {
                println!("conflict  {}: {}", c.dst_path.display(), c.reason);
            }
            if !r.conflicts.is_empty() {
                std::process::exit(1);
            }
            Ok(())
        }
    }
}

// ── M10: work context ───────────────────────────────────────────────────

fn cmd_context(config: Option<PathBuf>, cmd: ContextCmd) -> Result<()> {
    use tidy_core::curate::ContextDict;
    let cfg = load_config(config)?;
    let path = &cfg.curate.context;
    let mut dict = ContextDict::load(path)?;
    match cmd {
        ContextCmd::List => {
            println!(
                "# {}{}",
                path.display(),
                if path.exists() {
                    ""
                } else {
                    " (아직 없음: 기본값)"
                }
            );
            println!("고객사·프로젝트 ({}):", dict.entities.len());
            for e in &dict.entities {
                println!(
                    "  {}{}",
                    e.name,
                    if e.aliases.is_empty() {
                        String::new()
                    } else {
                        format!("  (별칭: {})", e.aliases.join(", "))
                    }
                );
            }
            println!(
                "이슈 유형: {}",
                dict.issue_types
                    .iter()
                    .map(|i| i.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            println!("자료 종류:");
            for k in &dict.doc_kinds {
                println!(
                    "  {}{}",
                    k.name,
                    if k.ext.is_empty() {
                        String::new()
                    } else {
                        format!("  ({})", k.ext.join(", "))
                    }
                );
            }
            return Ok(());
        }
        ContextCmd::Add {
            name,
            alias,
            issue,
            kind,
            ext,
        } => {
            let changed = if issue {
                dict.add_issue_type(&name)?
            } else if kind {
                dict.add_doc_kind(&name, &ext)?
            } else {
                dict.add_entity(&name, &alias)?
            };
            println!(
                "{}: {name}",
                if changed { "added" } else { "already there" }
            );
        }
        ContextCmd::Alias { entity, alias } => {
            let changed = dict.add_alias(&entity, &alias)?;
            println!(
                "{}: {entity} ← {alias}",
                if changed { "added" } else { "already there" }
            );
        }
    }
    dict.save(path)?;
    Ok(())
}

fn cmd_curate(config: Option<PathBuf>, cmd: CurateCmd) -> Result<()> {
    use tidy_core::curate::Curator;
    let cfg = load_config(config)?;
    let store = Store::open(&cfg.store.db_path())?;
    match cmd {
        CurateCmd::Apply { plan, only } => {
            let plan = restructure::load(&store, plan)?;
            if plan.source != restructure::PlanSource::Context {
                bail!(
                    "plan #{} is not a work-context plan (use tidy restructure --apply)",
                    plan.id
                );
            }
            // Default: every classified item (미분류 items stay anyway).
            let only = (!only.is_empty()).then_some(only.as_slice());
            let mover = Mover::new(store, cfg.mover.duplicate_policy);
            let r = restructure::apply(
                &mover,
                &plan,
                only,
                &protected_dirs(&cfg, &[]),
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
            Ok(())
        }
        CurateCmd::Scan { folder, dry_run } => {
            let root = default_folder(&cfg, folder)?;
            let root = dunce::canonicalize(&root)
                .with_context(|| format!("no such folder: {}", root.display()))?;
            let cur = Curator::from_config(&cfg)?;
            let protected = protected_dirs(&cfg, &[]);
            let rules = RuleSet::load(&cfg.classify.rules).unwrap_or_default();
            let anchors = tidy_core::classify::Anchors::from_config(&cfg);
            let anchor = anchors.for_path(&root).unwrap_or(&root).to_path_buf();
            let skip = tidy_core::curate::skip_dirs(&cfg, &rules, &anchor);
            // (M12) Files directly in the folder + its sub-folders as whole items.
            let items = cur.items_in(
                &root,
                &anchor,
                &cfg.ignore_filter()?,
                &protected,
                &skip,
                &store,
            );
            if !items.unsettled.is_empty() {
                eprintln!(
                    "# 아직 바뀌는 중인 폴더 {}개는 뺐습니다: {}",
                    items.unsettled.len(),
                    items
                        .unsettled
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            let folders = items.folders();
            let inputs = items.inputs;
            let (provider, model) = cfg
                .ai
                .active()
                .and_then(|p| p.classify.as_ref())
                .map(|p| (p.provider.as_str().to_string(), p.model.clone()))
                .unwrap_or_default();
            let est = cur.estimate(&store, &inputs, &provider, &model);
            println!(
                "항목 {}개(폴더 {folders}개 통째) (이미 분석 {}개) → 새로 분석 {}개, 요청 {}번, 예상 토큰 입력 {} · 출력 {}{}{}",
                est.files,
                est.cached,
                est.to_analyze,
                est.requests,
                est.input_tokens,
                est.output_tokens,
                est.cost_usd.map(|c| format!(", 약 ${c:.4}")).unwrap_or_else(|| ", 비용 알 수 없음(모델 단가 미등록)".into()),
                if cfg.ai.send_content() { "" } else { " — 본문은 보내지 않음(send_content = false)" },
            );
            if dry_run {
                return Ok(());
            }
            let ai = AiClassifier::from_config(&cfg, &rules)
                .context("업무 맥락 정리는 AI가 필요합니다 — [ai]를 켜고 분류 모델을 정하세요 (tidy ai status)")?;
            eprintln!("# asking {} / {} …", ai.provider_name(), ai.model());
            let plan = cur.plan(
                &ai,
                &store,
                &root,
                &anchor,
                &cfg.ignore_filter()?,
                &protected,
                &skip,
            )?;
            let plan = restructure::save(&store, plan)?;
            print_plan(&plan, None)?;
            println!(
                "\n적용: tidy curate apply {} [--only 1,3]  (승인 전에는 아무것도 옮기지 않음)",
                plan.id
            );
            Ok(())
        }
    }
}

// ── M18 NAS agent ───────────────────────────────────────────────────────

fn cmd_serve(config: Option<PathBuf>, args: ServeArgs) -> Result<()> {
    use tidy_core::remote::server;
    let config_path = config
        .or_else(|| std::env::var_os("TIDY_CONFIG").map(PathBuf::from))
        .or_else(Config::default_path)
        .context("no config path (use --config or TIDY_CONFIG)")?;
    let token = server::load_or_create_token(&config_path, std::env::var("TIDY_TOKEN").ok())?;
    if args.print_token {
        // Only on request, only to stdout: never into the log.
        println!("{token}");
        return Ok(());
    }
    eprintln!("# config: {}", config_path.display());
    eprintln!("# data root: {}", args.data_root.display());
    if std::env::var("TIDY_TOKEN")
        .map(|t| t.trim().is_empty())
        .unwrap_or(true)
    {
        eprintln!(
            "# password: {} (`tidy serve --print-token` shows it)",
            server::token_path(&config_path).display()
        );
    }
    let agent = server::start(server::AgentOptions {
        config_path,
        data_root: args.data_root,
        listen: args.listen,
        token,
        watch_options: None,
        reindex_every: Duration::from_secs(3600),
    })?;
    eprintln!("# listening on {} — Ctrl+C to stop", agent.addr());
    agent.wait();
    Ok(())
}

fn nas_client(config: Option<PathBuf>, t: &NasTarget) -> Result<tidy_core::remote::NasClient> {
    use tidy_core::remote::NasClient;
    if let Some(url) = &t.url {
        let token = std::env::var("TIDY_TOKEN").context("set TIDY_TOKEN to the NAS password")?;
        return Ok(NasClient::new(url, &token)?);
    }
    let cfg = load_config(config)?;
    let nas = match &t.name {
        Some(n) => cfg.nas.iter().find(|x| &x.name == n),
        None => cfg.nas.first(),
    }
    .context("no [[nas]] configured (or use --url)")?;
    let token = ai::keys::get_key(&nas.token_ref())?
        .with_context(|| format!("no password in the keychain under {}", nas.token_ref()))?;
    Ok(NasClient::new(&nas.url, &token)?)
}

fn cmd_nas(config: Option<PathBuf>, cmd: NasCmd) -> Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    match cmd {
        NasCmd::Status(t) => {
            let c = nas_client(config, &t)?;
            let s = rt.block_on(c.status())?;
            println!(
                "{}  Tidy {}  data root {}",
                c.base(),
                s.version,
                s.data_root
            );
            println!(
                "watching: {}{}",
                if s.watching { "yes" } else { "no" },
                s.error.map(|e| format!(" ({e})")).unwrap_or_default()
            );
            for w in &s.watches {
                println!(
                    "  {} {} [{}{}]",
                    if w.online { "●" } else { "○" },
                    w.path,
                    w.mode,
                    if w.polling { ", polling" } else { "" }
                );
            }
            println!(
                "pending {}  indexed {}  ocr waiting {}  moved today {}",
                s.pending, s.files, s.ocr_waiting, s.today_moved
            );
        }
        NasCmd::Search {
            target,
            query,
            limit,
        } => {
            let c = nas_client(config, &target)?;
            let r = rt.block_on(c.search(&query, limit))?;
            if r.hits.is_empty() {
                println!("no results");
            }
            for h in r.hits {
                println!("{}  [{}]", h.path, h.matched_in);
                if !h.snippet.is_empty() {
                    println!("    {}", h.snippet.replace(['\u{E000}', '\u{E001}'], "*"));
                }
            }
        }
        NasCmd::Pending(t) => {
            let c = nas_client(config, &t)?;
            let items = rt.block_on(c.pending())?;
            if items.is_empty() {
                println!("no pending files");
            }
            for p in items {
                println!(
                    "#{:<4} {} -> {}  [{}]",
                    p.id,
                    p.path,
                    p.suggested_dest.unwrap_or_default(),
                    p.rule.or(p.reason).unwrap_or_default()
                );
            }
        }
        NasCmd::Log { target, limit } => {
            let c = nas_client(config, &target)?;
            for b in rt.block_on(c.batches(limit))? {
                println!(
                    "{}  {}  {} files{}  {}  e.g. {}",
                    short(&b.batch_id),
                    fmt_time(b.last_at),
                    b.moves,
                    if b.undone > 0 {
                        format!(" ({} undone)", b.undone)
                    } else {
                        String::new()
                    },
                    b.label.unwrap_or(b.decided_by),
                    b.example.display()
                );
            }
        }
    }
    Ok(())
}
