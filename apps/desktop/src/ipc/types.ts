// IPC types — mirror of apps/desktop/src-tauri/src/commands.rs (camelCase).
// Keep both sides in sync when changing a command.

export interface Status {
  configPath: string;
  configExists: boolean;
  rulesPath: string;
  rulesExists: boolean;
  dbPath: string;
  /** Folders currently being watched (empty while paused / not configured). */
  watching: string[];
  paused: boolean;
  indexing: boolean;
  pendingCount: number;
  filesIndexed: number;
  lastError: string | null;
  hotkey: string;
  /** AI step: off (not enabled) | ready | unavailable (key/network…) | starting. */
  aiState: "off" | "ready" | "unavailable" | "starting";
  /** "provider / model" when ready, the reason otherwise. */
  aiDetail: string | null;
  /** Network watch folders unreachable right now (paused, retried every 60 s). */
  offline: string[];
}

export type MatchedIn = "name" | "content" | "both" | "filter";

export interface SearchHit {
  path: string;
  name: string;
  dir: string;
  ext: string | null;
  size: number;
  /** Unix seconds. */
  mtime: number;
  matchedIn: MatchedIn;
  /** File name with matches wrapped in MARK_START / MARK_END. */
  nameHighlight: string;
  /** Body excerpt with matches wrapped in MARK_START / MARK_END. */
  snippet: string;
  textStatus: "none" | "ok" | "unsupported" | "failed" | "skipped";
  exists: boolean;
}

/** Highlight markers used by the backend (Unicode private use area). */
export const MARK_START = "";
export const MARK_END = "";

export interface PendingItem {
  id: number;
  path: string;
  name: string;
  dir: string;
  suggestedDest: string | null;
  /** Rule that asked for confirmation (source "rule"). */
  rule: string | null;
  source: "rule" | "ai";
  /** AI confidence 0..1 (source "ai"). */
  confidence: number | null;
  /** AI's one-line reason (source "ai"). */
  reason: string | null;
  /** AI-suggested new file name. */
  suggestedName: string | null;
  /** Unix seconds. */
  createdAt: number;
  exists: boolean;
  size: number | null;
}

/** Confirm popup actions (SPEC §5.3). */
export type Decision =
  // destDir null = suggested folder; newName = "이름 바꿔 이동"
  | { kind: "move"; destDir: string | null; newName?: string | null }
  | { kind: "skip" }
  | { kind: "ignore" };

export interface Resolution {
  kind: "moved" | "duplicate" | "already_in_place" | "skipped" | "ignored" | "gone";
  path: string;
  dst: string | null;
  batchId: string | null;
}

export interface UndoResult {
  /** [from, to] pairs. */
  restored: [string, string][];
  conflicts: string[];
}

export interface WatchFolder {
  path: string;
  kind: "local" | "network";
  recursive: boolean;
  indexContent: boolean;
  /** Seconds; null = default (network 120 s, local: no polling). */
  pollIntervalS: number | null;
  /** Network only: polling only, no OS change events. */
  pollOnly: boolean;
  /** Network only: "HH:MM-HH:MM" window for background body-text indexing. */
  contentIndexWindow: string | null;
}

export interface Settings {
  configPath: string;
  watches: WatchFolder[];
  rulesPath: string;
  rulesText: string;
  rulesExists: boolean;
  /** Effective database path. */
  dbPath: string;
  /** `[store] path` from config.toml, null = default location. */
  dbPathCustom: string | null;
  dbPathDefault: string;
  quietMode: boolean;
  hotkey: string;
}

export interface SaveSettingsArgs {
  watches: WatchFolder[];
  dbPathCustom: string | null;
  quietMode: boolean;
  hotkey: string;
  rulesText: string;
}

export interface RulesCheck {
  ok: boolean;
  rules: string[];
  error: string | null;
}

export type ActivityKind =
  | "moved"
  | "pending"
  | "duplicate"
  | "no_rule"
  | "skipped"
  | "error"
  | "warning"
  | "info";

export interface Activity {
  kind: ActivityKind;
  path: string;
  detail: string;
  /** Unix seconds. */
  at: number;
}

export type MainTab = "activity" | "pending" | "report" | "history" | "settings";

export type AiProvider = "anthropic" | "openai" | "ollama" | "";

/** `[ai]` + the active profile's classify model. The API key is never sent back. */
export interface AiSettings {
  enabled: boolean;
  activeProfile: string;
  profiles: string[];
  provider: AiProvider;
  model: string;
  baseUrl: string;
  /** Keychain entry name, e.g. "tidy/anthropic". */
  keyRef: string;
  /** Anthropic effort ("low" | "medium" | …) or "". */
  effort: string;
  autoThreshold: number;
  sendContent: boolean;
  /** A key is stored in the OS keychain under keyRef. */
  keyStored: boolean;
}

/** Learner rule suggestion (SPEC §5.2). */
export interface LearnSuggestion {
  key: string;
  ruleName: string;
  count: number;
  examples: string[];
  dest: string;
  ruleToml: string;
}

// ── M5: report / restructure / history ──────────────────────────────────
// These mirror the core's own JSON (snake_case), sent as-is by
// src-tauri/src/report_cmds.rs.

export interface TypeStat { ext: string; count: number; bytes: number }
export interface FolderStat { path: string; bytes: number; files: number }
export interface StaleFile { path: string; bytes: number; last_used: number }
export interface Stats {
  total_files: number;
  total_dirs: number;
  total_bytes: number;
  root_level_files: number;
  by_type: TypeStat[];
  largest_files: FolderStat[];
  largest_dirs: FolderStat[];
  empty_dirs: string[];
  stale_files: number;
  stale: StaleFile[];
}
export interface DupGroup { hash: string; size: number; paths: string[]; wasted: number }

export interface Report {
  root: string;
  generatedAt: number;
  staleDays: number;
  /** Folder tree text, depth 3. */
  treeText: string;
  stats: Stats;
  /** Paths are relative to root, "/"-separated. */
  duplicates: DupGroup[];
  errors: string[];
}

export interface PlannedMove {
  id: number;
  from: string;
  to: string;
  is_dir: boolean;
  files: number;
  bytes: number;
  reason: string;
  warnings: string[];
}
export interface Plan {
  id: number;
  root: string;
  source: "rules" | "ai";
  created_at: number;
  summary: string;
  folders: { path: string; reason: string }[];
  moves: PlannedMove[];
  rejected: { from: string; to: string; reason: string }[];
  applied_batch: string | null;
}
export interface Preview {
  before: string;
  after: string;
  /** [from, to] pairs leaving the folder. */
  outside: [string, string][];
  files_moved: number;
  /** Folders left empty (kept — nothing is deleted). */
  emptied: string[];
}
export interface PlanResult { plan: Plan; preview: Preview }
export interface ApplyResult { batchId: string; moved: number; skipped: string[]; errors: string[] }
export interface StoredPlan {
  id: number;
  root: string;
  source: string;
  created_at: number;
  applied_batch: string | null;
  applied_at: number | null;
}

export interface BatchSummary {
  batch_id: string;
  first_at: number;
  last_at: number;
  moves: number;
  undone: number;
  decided_by: "rule" | "ai" | "user" | "restructure";
  label: string | null;
  example: string;
}
export interface MoveLogEntry {
  id: number;
  batch_id: string;
  src_path: string;
  dst_path: string;
  decided_by: string;
  rule_name: string | null;
  ai_confidence: number | null;
  moved_at: number;
  undone_at: number | null;
  /** (M6) Logged but the transfer has not finished (long NAS copy, or interrupted). */
  in_progress: boolean;
}

/** Progress of a long copy-verify move to/from a NAS (core snake_case). */
export interface TransferProgress {
  src: string;
  dst: string;
  copied: number;
  total: number;
  phase: "copy" | "verify" | "done";
}
