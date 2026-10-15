// Typed wrappers for every Tauri command (src-tauri/src/commands.rs).
import { invoke } from "@tauri-apps/api/core";
import type {
  Decision,
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
