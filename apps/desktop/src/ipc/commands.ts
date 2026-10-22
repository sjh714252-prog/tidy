// Typed wrappers for every Tauri command (src-tauri/src/commands.rs).
import { inTauri, invoke } from "./transport";
import type {
  AiSettings,
  ApplyResult,
  BatchResult,
  BatchSummary,
  MoveLogEntry,
  PlanResult,
  Preview,
  Report,
  StoredPlan,
  Decision,
  DirEntry,
  EditorState,
  FinishResult,
  FsRoot,
  PreviewReport,
  RuleCard,
  RuleDrafts,
  WatchFolder,
  WizardInfo,
  LearnSuggestion,
  PendingItem,
  Resolution,
  RulesCheck,
  SaveSettingsArgs,
  SearchHit,
  SearchResult,
  SecuritySettings,
  ContextCandidates,
  ContextEdit,
  ContextSuggestion,
  CurateEstimate,
  CurateSettings,
  MoveEdit,
  SemanticStatus,
  Settings,
  Status,
  UndoResult,
} from "./types";

export const getStatus = () => invoke<Status>("get_status");
export const setPaused = (paused: boolean) => invoke<Status>("set_paused", { paused });
export const indexNow = () => invoke<void>("index_now");
/** "search" | "main" | "main:<tab>" | "confirm" */
export const showWindow = (which: string) => invoke<void>("show_window", { which });

export const search = (query: string, limit = 50) =>
  invoke<SearchHit[]>("search", { query, limit });
/** Words + meaning; falls back to words only (with a note) without a model. */
export const searchSemantic = (query: string, limit = 50) =>
  invoke<SearchResult>("search_semantic", { query, limit });
export const semanticStatus = () => invoke<SemanticStatus>("semantic_status");
/** Opens with the default app. For a vault zip (M9) a temporary decrypted
 * copy is opened instead and its path returned. */
export const openPath = (path: string) => invoke<string | null>("open_path", { path });
/** (M9) Decrypt a vault zip to a temporary copy without opening it. */
export const vaultCopy = (path: string) => invoke<string>("vault_copy", { path });
export const getSecuritySettings = () => invoke<SecuritySettings>("get_security_settings");
/** The password goes to the OS keychain only; null = keep the current one. */
export const saveSecuritySettings = (settings: SecuritySettings, password: string | null) =>
  invoke<SecuritySettings>("save_security_settings", { settings, password });
export const revealPath = (path: string) => invoke<void>("reveal_path", { path });

export const listPending = () => invoke<PendingItem[]>("list_pending");
export const resolvePending = (id: number, decision: Decision) =>
  invoke<Resolution>("resolve_pending", { id, decision });
/** One batch for the whole selection, so a single undo restores all of it. */
export const resolvePendingBatch = (ids: number[], decision: Decision) =>
  invoke<BatchResult<Resolution>[]>("resolve_pending_batch", { ids, decision });
export const undoLast = () => invoke<UndoResult | null>("undo_last");
export const pickFolder = (start?: string | null) =>
  inTauri
    ? invoke<string | null>("pick_folder", { start: start ?? null })
    : // Dev bridge (browser): no native dialog, so ask for the path as text.
      Promise.resolve(window.prompt("폴더 경로를 입력하세요", start ?? "")?.trim() || null);
/** UNC path, remote drive or SMB/NFS mount → default kind "network". */
export const isNetworkPath = (path: string) => invoke<boolean>("is_network_path", { path });

export const getSettings = () => invoke<Settings>("get_settings");
export const checkRules = (text: string) => invoke<RulesCheck>("check_rules", { text });
export const saveSettings = (args: SaveSettingsArgs) => invoke<Status>("save_settings", { args });

export const getAiSettings = () => invoke<AiSettings>("get_ai_settings");
/** apiKey / embedApiKey (optional) are written to the OS keychain only. */
export const saveAiSettings = (settings: AiSettings, apiKey: string | null, embedApiKey: string | null = null) =>
  invoke<AiSettings>("save_ai_settings", { settings, apiKey, embedApiKey });
export const deleteApiKey = (keyRef: string) => invoke<AiSettings>("delete_api_key", { keyRef });
export const testAi = () => invoke<string>("test_ai");

export const listLearn = () => invoke<LearnSuggestion[]>("list_learn");
export const acceptLearn = (key: string) => invoke<void>("accept_learn", { key });
export const dismissLearn = (key: string) => invoke<void>("dismiss_learn", { key });

export const reportRoots = () => invoke<string[]>("report_roots");
export const buildReport = (path: string, duplicates: boolean) =>
  invoke<Report>("build_report", { path, duplicates });
export const planRules = (path: string, recursive: boolean, protect: string[]) =>
  invoke<PlanResult>("plan_rules", { path, recursive, protect });
export const planAi = (path: string, protect: string[]) => invoke<PlanResult>("plan_ai", { path, protect });
// ── M10 work context ──
export const getCurateSettings = () => invoke<CurateSettings>("get_curate_settings");
export const saveCurateSettings = (settings: CurateSettings) =>
  invoke<CurateSettings>("save_curate_settings", { settings });
export const curateEstimate = (path: string, recursive: boolean) =>
  invoke<CurateEstimate>("curate_estimate", { path, recursive });
/** AI analysis → saved plan + preview. Nothing moves. */
export const planContext = (path: string, recursive: boolean, protect: string[]) =>
  invoke<PlanResult>("plan_context", { path, recursive, protect });
export const editContextMove = (id: number, moveId: number, edit: MoveEdit, selected: number[]) =>
  invoke<ContextEdit>("edit_context_move", { id, moveId, edit, selected });
export const applyContextSuggestions = (suggestions: ContextSuggestion[]) =>
  invoke<CurateSettings>("apply_context_suggestions", { suggestions });
export const extractContext = () => invoke<ContextCandidates>("extract_context");
export const contextGroups = () => invoke<[string, string][]>("context_groups");
export const previewPlan = (id: number, ids: number[]) => invoke<Preview>("preview_plan", { id, ids });
/** Moves files. Only call from an explicit user confirmation. */
export const applyPlan = (id: number, ids: number[], protect: string[]) =>
  invoke<ApplyResult>("apply_plan", { id, ids, protect });
export const listPlans = () => invoke<StoredPlan[]>("list_plans");
export const openPlan = (id: number) => invoke<PlanResult>("open_plan", { id });

export const listBatches = (limit = 100) => invoke<BatchSummary[]>("list_batches", { limit });
export const batchMoves = (batchId: string) => invoke<MoveLogEntry[]>("batch_moves", { batchId });
export const undoBatch = (batchId: string) => invoke<UndoResult>("undo_batch", { batchId });

// M8: wizard, rule editor, folder tree.
export const wizardInfo = () => invoke<WizardInfo>("wizard_info");
/** watches null = the configured folders. exclusive = first-match counts. */
export const previewRules = (watches: WatchFolder[] | null, cards: RuleCard[], exclusive: boolean) =>
  invoke<PreviewReport>("preview_rules", { watches, cards, exclusive });
/** Writes config.toml + rules.toml and starts watching ("감시 시작"). */
export const finishOnboarding = (args: { watches: WatchFolder[]; starters: string[]; custom: RuleCard[] }) =>
  invoke<FinishResult>("finish_onboarding", { args });
export const rulesEditor = (text: string, preview: boolean) => invoke<EditorState>("rules_editor", { text, preview });
export const rulesFromCards = (text: string, cards: RuleCard[], preview: boolean) =>
  invoke<EditorState>("rules_from_cards", { text, cards, preview });
export const aiRuleDrafts = () => invoke<RuleDrafts>("ai_rule_drafts");
export const fsRoots = () => invoke<FsRoot[]>("fs_roots");
export const listDirs = (path: string) => invoke<DirEntry[]>("list_dirs", { path });
export const makeDir = (parent: string, name: string) => invoke<string>("make_dir", { parent, name });
export const openWizard = () => invoke<void>("open_wizard");
