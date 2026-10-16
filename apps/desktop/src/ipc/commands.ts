// Typed wrappers for every Tauri command (src-tauri/src/commands.rs).
import { inTauri, invoke } from "./transport";
import type {
  BatchResult,
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
/** One batch for the whole selection, so a single undo restores all of it. */
export const resolvePendingBatch = (ids: number[], decision: Decision) =>
  invoke<BatchResult<Resolution>[]>("resolve_pending_batch", { ids, decision });
export const undoLast = () => invoke<UndoResult | null>("undo_last");
export const pickFolder = (start?: string | null) =>
  inTauri
    ? invoke<string | null>("pick_folder", { start: start ?? null })
    : // Dev bridge (browser): no native dialog, so ask for the path as text.
      Promise.resolve(window.prompt("폴더 경로를 입력하세요", start ?? "")?.trim() || null);

export const getSettings = () => invoke<Settings>("get_settings");
export const checkRules = (text: string) => invoke<RulesCheck>("check_rules", { text });
export const saveSettings = (args: SaveSettingsArgs) => invoke<Status>("save_settings", { args });
