// Typed wrappers for every Tauri command (src-tauri/src/commands.rs).
import { invoke } from "@tauri-apps/api/core";
import type {
  AiSettings,
  Decision,
  LearnSuggestion,
  PendingItem,
  Resolution,
  RulesCheck,
  SaveSettingsArgs,
  SearchHit,
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
export const openPath = (path: string) => invoke<void>("open_path", { path });
export const revealPath = (path: string) => invoke<void>("reveal_path", { path });

export const listPending = () => invoke<PendingItem[]>("list_pending");
export const resolvePending = (id: number, decision: Decision) =>
  invoke<Resolution>("resolve_pending", { id, decision });
export const undoLast = () => invoke<UndoResult | null>("undo_last");
export const pickFolder = (start?: string | null) =>
  invoke<string | null>("pick_folder", { start: start ?? null });

export const getSettings = () => invoke<Settings>("get_settings");
export const checkRules = (text: string) => invoke<RulesCheck>("check_rules", { text });
export const saveSettings = (args: SaveSettingsArgs) => invoke<Status>("save_settings", { args });

export const getAiSettings = () => invoke<AiSettings>("get_ai_settings");
/** apiKey (optional) is written to the OS keychain only. */
export const saveAiSettings = (settings: AiSettings, apiKey: string | null) =>
  invoke<AiSettings>("save_ai_settings", { settings, apiKey });
export const deleteApiKey = (keyRef: string) => invoke<AiSettings>("delete_api_key", { keyRef });
export const testAi = () => invoke<string>("test_ai");

export const listLearn = () => invoke<LearnSuggestion[]>("list_learn");
export const acceptLearn = (key: string) => invoke<void>("accept_learn", { key });
export const dismissLearn = (key: string) => invoke<void>("dismiss_learn", { key });
