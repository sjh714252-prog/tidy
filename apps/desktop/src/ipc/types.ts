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
  /** First-run wizard finished; nothing is watched or moved before that (M8). */
  onboarded: boolean;
  /** Semantic search: off | ready | unavailable (repeated failures, words only). */
  semanticState: "off" | "ready" | "unavailable";
  /** Embedding model when on, the reason otherwise. */
  semanticDetail: string | null;
  /** (M11) Configured watch folders (also while paused). */
  watchTotal: number;
  watchPaths: string[];
  /** (M11) Watch folders organized in place, and distinct "모으기" roots. */
  organizeInPlace: number;
  organizeRoots: string[];
  /** (M11) Newer release found by the last update check. */
  update: UpdateInfo | null;
}

/** (M11) Latest release vs. this version (core `update::UpdateInfo`). */
export interface UpdateInfo {
  current: string;
  latest: string;
  newer: boolean;
  /** Installer for this OS, or the release page. */
  url: string;
  page: string;
}

export interface UpdateResult {
  current: string;
  info: UpdateInfo | null;
  error: string | null;
  checkedAt: number;
}

export interface AboutInfo {
  version: string;
  gitSha: string;
  repo: string;
  license: string;
  os: string;
  configPath: string;
  configDir: string;
  dbPath: string;
  dbDir: string;
  logDir: string;
  checkUpdates: boolean;
}

/** (M11) 홈 탭: 감시 현황 + 기존 파일 미정리 수. */
export interface Backlog {
  toMove: number;
  noRule: number;
  total: number;
  capped: boolean;
  /** (M12) Sub-folders to organize as whole items ("새 폴더 N개"). */
  newFolders: number;
  /** (M12) Sub-folders still being copied (not offered yet). */
  settling: number;
}
export interface FolderOverview {
  path: string;
  mode: "in_place" | "collect";
  organizeRoot: string | null;
  network: boolean;
  recursive: boolean;
  state: "ok" | "offline" | "paused" | "idle";
  exclude: number;
  /** null while counting, or for network folders. */
  backlog: Backlog | null;
}
export interface Overview {
  folders: FolderOverview[];
  computing: boolean;
  total: number;
  capped: boolean;
  /** AI on + customers in the context dictionary. */
  canCurate: boolean;
  cap: number;
  /** (M12) New sub-folders over all watch folders. */
  newFolders: number;
}

/** (M12) "고객사별 보기". */
export interface CustomerStat {
  name: string;
  /** Existing customer folders (one per watch folder anchor). */
  dirs: string[];
  /** Issue / date folders directly in them. */
  issues: number;
  files: number;
  /** Newest file's mtime (Unix seconds). */
  last: number | null;
  capped: boolean;
}

/** "semantic" = found by meaning only (no word matched). */
export type MatchedIn = "name" | "content" | "both" | "filter" | "semantic";

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
  /** Cosine similarity to the query (0..1) when semantic search ranked it. */
  similarity: number | null;
  /** (M9) Encrypted vault zip (🔒); opening decrypts a temporary copy. */
  vault: boolean;
  /** (M9) "주민번호 2 · 이름+휴대전화 15" when judged personal. */
  pii: string | null;
}

/** Words + meaning search (search_semantic). */
export interface SearchResult {
  hits: SearchHit[];
  /** fts = words only; hybrid = words + meaning. */
  mode: "fts" | "hybrid";
  /** The query read like a question/sentence (meaning weighted up). */
  natural: boolean;
  /** Why meaning did not take part. */
  note: string | null;
}

export interface SemanticStatus {
  state: "off" | "ready" | "unavailable";
  detail: string | null;
  /** Files with a current vector / indexed files. */
  embedded: number;
  files: number;
}

/** Start at login (tray only). */
export interface Autostart {
  /** general.start_on_login */
  wanted: boolean;
  /** Registered with the OS now; null in dev builds (not applied). */
  registered: boolean | null;
  error: string | null;
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
  /** (M9) Approving locks the file into an encrypted zip. */
  encrypt: boolean;
  /** (M9) "주민번호 2 · 이름+휴대전화 15". */
  pii: string | null;
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

/** serde `Result<T, String>`: `{ Ok: T }` or `{ Err: string }`. */
export type BatchResult<T> = { Ok: T } | { Err: string };

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
  /** M8: in_place = sub-folders inside this folder; collect = under organizeRoot. */
  mode: OrganizeMode;
  /** collect: folder the category folders go under. */
  organizeRoot: string | null;
  /** Sub-folders Tidy leaves alone (absolute paths inside this folder). */
  exclude: string[];
}

export type OrganizeMode = "in_place" | "collect";

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
  autostart: Autostart;
  /** (M11) `[general] check_updates`. */
  checkUpdates: boolean;
}

export interface SaveSettingsArgs {
  watches: WatchFolder[];
  dbPathCustom: string | null;
  quietMode: boolean;
  hotkey: string;
  rulesText: string;
  startOnLogin: boolean;
  checkUpdates: boolean;
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

// ── M8: rule cards, wizard, folder tree ─────────────────────────────────

/** One [[rule]] of rules.toml as the card editor shows it ("" / null = not set). */
export interface RuleCard {
  /** Index of the [[rule]] table it came from; null = new. */
  origin: number | null;
  name: string;
  enabled: boolean;
  confirm: boolean;
  ext: string[];
  nameContains: string;
  nameRegex: string;
  /** Bytes ("1048576") or "10MB". */
  minSize: string;
  maxSize: string;
  olderThanDays: number | null;
  newerThanDays: number | null;
  fromDir: string;
  dest: string;
  /** (M9) "" (never) | "pii" | "always". */
  encrypt: string;
  /** Keys the card does not edit (kept as they are, e.g. ai_auto). */
  otherKeys: string[];
}

/** Core JSON (snake_case). */
export interface RulePreview {
  index: number;
  name: string;
  count: number;
  examples: string[];
  sample_dest: string | null;
}

export interface PreviewReport {
  rules: RulePreview[];
  scanned: number;
  unmatched: number;
  truncated: boolean;
  skipped: string[];
  missing: string[];
}

export interface EditorState {
  text: string;
  ok: boolean;
  error: string | null;
  cards: RuleCard[] | null;
  preview: PreviewReport | null;
}

export interface Starter {
  id: string;
  title: string;
  summary: string;
  card: RuleCard;
}

export interface SuggestedFolder {
  label: string;
  path: string;
  exists: boolean;
  checked: boolean;
}

export interface WizardInfo {
  onboarded: boolean;
  watches: WatchFolder[];
  suggested: SuggestedFolder[];
  starters: Starter[];
  existingRules: string[];
}

export interface FinishResult {
  status: Status;
  existingFiles: number;
  matchingFiles: number;
  folders: string[];
}

export interface FsRoot {
  label: string;
  path: string;
  kind: "drive" | "home" | "favorite" | "watch" | "anchor";
}

export interface DirEntry {
  name: string;
  path: string;
}

/** Core JSON (snake_case). */
export interface RuleDrafts {
  summary: string;
  drafts: { card: RuleCard; reason: string }[];
  rejected: { name: string; dest: string; reason: string }[];
}

export const emptyCard = (): RuleCard => ({
  origin: null,
  name: "",
  enabled: true,
  confirm: false,
  ext: [],
  nameContains: "",
  nameRegex: "",
  minSize: "",
  maxSize: "",
  olderThanDays: null,
  newerThanDays: null,
  fromDir: "",
  dest: "",
  encrypt: "",
  otherKeys: [],
});

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
  /** Embedding model for semantic search ("" = none; Anthropic has no embeddings). */
  embedProvider: "" | "openai" | "ollama";
  embedModel: string;
  embedBaseUrl: string;
  embedKeyRef: string;
  embedKeyStored: boolean;
  /** Where keys are stored on this machine. */
  keyStore: string;
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
  /** (M9) "개인정보 의심 파일 N개(미암호화 M개)" (search index). */
  pii: string | null;
  piiSuspected: number;
  piiUnencrypted: number;
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
  /** (M10) The AI's work-context judgement (source "context"). */
  context?: ContextNote | null;
  /** (M10) Locked into an encrypted zip (personal file, vault mode). */
  encrypt?: boolean;
}
/** (M10) Core JSON (snake_case). */
export interface ContextNote {
  entity: string | null;
  issue: string;
  /** Folder date after merging, YYYY-MM-DD. */
  date: string;
  own_date: string;
  doc_kind: string;
  topic: string | null;
  confidence: number;
  reason: string;
  ai_entity: string | null;
  ai_issue: string;
  unclassified: boolean;
  edited: boolean;
  key: string;
  /** (M12) A folder item's inner tree (a few levels). */
  tree?: string | null;
}
export interface Plan {
  id: number;
  root: string;
  source: "rules" | "ai" | "context";
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

/** (M9) [security.pii] — what counts as personal information. */
export interface PiiSettings {
  enabled: boolean;
  rrn: boolean;
  account: boolean;
  namePhone: boolean;
  nameEmail: boolean;
  minRrn: number;
  minAccount: number;
  minNamePhone: number;
  minNameEmail: number;
  exclude: string[];
  aiCheck: boolean;
}

/** (M9) Settings → 보안(개인정보). Never contains the password. */
export interface SecuritySettings {
  /** rules = 규칙 옵션으로만 (B) · vault = 보안 폴더로 모으기 (A) */
  mode: "rules" | "vault";
  vaultDir: string;
  approval: "confirm" | "auto";
  pii: PiiSettings;
  // read-only
  hasPassword: boolean;
  previousPasswords: number;
  keyStore: string;
  keyError: string | null;
  suspected: number;
  unencrypted: number;
  locked: number;
}

/** (M10) context.toml (core JSON field names). */
export interface ContextDict {
  entity: { name: string; aliases: string[]; notes?: string }[];
  issue_type: { name: string }[];
  doc_kind: { name: string; ext: string[] }[];
}

/** (M10) [curate] fields the settings edit. */
export interface CuratePatch {
  enabled: boolean;
  prefer: boolean;
  template: string;
  other_template: string;
  min_confidence: number;
}

export interface CurateSettings {
  settings: CuratePatch;
  dict: ContextDict;
  contextPath: string;
  contextExists: boolean;
  analysed: number;
}

/** (M10) "파일 N개, 예상 토큰/비용". */
export interface CurateEstimate {
  files: number;
  cached: number;
  to_analyze: number;
  requests: number;
  input_tokens: number;
  output_tokens: number;
  cost_usd: number | null;
  provider: string;
  model: string;
  /** (M12) Items that are whole folders. */
  folders: number;
  /** (M12) Sub-folders still changing (left out this time). */
  unsettled: number;
}

export type ContextSuggestion =
  | { kind: "add_entity"; name: string }
  | { kind: "add_alias"; entity: string; alias: string }
  | { kind: "add_issue_type"; name: string };

export interface ContextEdit {
  plan: Plan;
  preview: Preview;
  suggestions: { suggestion: ContextSuggestion; label: string }[];
}

export interface MoveEdit {
  entity: string | null;
  issue?: string | null;
  docKind?: string | null;
  topic?: string | null;
  /** (M12) The item's own date, YYYY-MM-DD. */
  date?: string | null;
}

/** (M10) "바탕화면에서 뽑기" candidates. */
export interface ContextCandidates {
  entities: { name: string; aliases: string[]; files: number; reason: string; known: boolean }[];
  issue_types: { name: string; known: boolean }[];
  doc_kinds: { name: string; ext: string[]; known: boolean }[];
  files: number;
}
