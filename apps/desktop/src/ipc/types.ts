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
