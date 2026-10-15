// Backend → frontend events (src-tauri/src/watch.rs, windows.rs).
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { Activity, MainTab } from "./types";

export const EV_ACTIVITY = "tidy://activity";
export const EV_PENDING = "tidy://pending";
export const EV_STATUS = "tidy://status";
export const EV_NAVIGATE = "tidy://navigate";
export const EV_FOCUS_SEARCH = "tidy://focus-search";

export const onActivity = (cb: (a: Activity) => void): Promise<UnlistenFn> =>
  listen<Activity>(EV_ACTIVITY, (e) => cb(e.payload));
/** Pending queue changed (payload: new count, or null from the popup trigger). */
export const onPending = (cb: () => void): Promise<UnlistenFn> => listen(EV_PENDING, () => cb());
export const onStatus = (cb: () => void): Promise<UnlistenFn> => listen(EV_STATUS, () => cb());
export const onNavigate = (cb: (tab: MainTab) => void): Promise<UnlistenFn> =>
  listen<MainTab>(EV_NAVIGATE, (e) => cb(e.payload));
export const onFocusSearch = (cb: () => void): Promise<UnlistenFn> =>
  listen(EV_FOCUS_SEARCH, () => cb());
