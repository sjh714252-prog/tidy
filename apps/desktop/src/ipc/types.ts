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
  /** Rule that asked for confirmation. */
  rule: string | null;
  /** Unix seconds. */
  createdAt: number;
  exists: boolean;
  size: number | null;
}

/** Confirm popup actions (SPEC §5.3). */
export type Decision =
  | { kind: "move"; destDir: string | null } // null = suggested folder
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
  pollIntervalS: number | null;
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
  | "info";

export interface Activity {
  kind: ActivityKind;
  path: string;
  detail: string;
  /** Unix seconds. */
  at: number;
}

export type MainTab = "activity" | "pending" | "settings";
