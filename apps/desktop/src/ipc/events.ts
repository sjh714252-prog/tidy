// Backend → frontend events (src-tauri/src/watch.rs, windows.rs).
import { listen, type UnlistenFn } from "./transport";
import type { Activity, MainTab, TransferProgress } from "./types";

export const EV_ACTIVITY = "tidy://activity";
export const EV_PENDING = "tidy://pending";
export const EV_STATUS = "tidy://status";
export const EV_NAVIGATE = "tidy://navigate";
export const EV_PROGRESS = "tidy://progress";
export const EV_FOCUS_SEARCH = "tidy://focus-search";

export const onActivity = (cb: (a: Activity) => void): Promise<UnlistenFn> =>
  listen<Activity>(EV_ACTIVITY, (e) => cb(e.payload));
/** Pending queue changed (payload: new count, or null from the popup trigger). */
export const onPending = (cb: () => void): Promise<UnlistenFn> => listen(EV_PENDING, () => cb());
export const onStatus = (cb: () => void): Promise<UnlistenFn> => listen(EV_STATUS, () => cb());
/** Payload: a tab, optionally with a query ("report?plan=rules&path=…", M8). */
export const onNavigate = (cb: (tab: MainTab, query: URLSearchParams) => void): Promise<UnlistenFn> =>
  listen<string>(EV_NAVIGATE, (e) => {
    const [tab, q] = e.payload.split("?");
    cb(tab as MainTab, new URLSearchParams(q ?? ""));
  });
export const onFocusSearch = (cb: () => void): Promise<UnlistenFn> =>
  listen(EV_FOCUS_SEARCH, () => cb());
export const onProgress = (cb: (p: TransferProgress) => void): Promise<UnlistenFn> =>
  listen<TransferProgress>(EV_PROGRESS, (e) => cb(e.payload));
