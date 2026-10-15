import { create } from "zustand";
import * as ipc from "../ipc/commands";
import type { RulesCheck, Settings, WatchFolder } from "../ipc/types";

interface SettingsState {
  loaded: Settings | null;
  draft: Settings | null;
  rulesCheck: RulesCheck | null;
  saving: boolean;
  message: string | null;
  error: string | null;
  load: () => Promise<void>;
  patch: (p: Partial<Settings>) => void;
  setWatch: (i: number, p: Partial<WatchFolder>) => void;
  addWatch: (path: string) => void;
  removeWatch: (i: number) => void;
  setRules: (text: string) => void;
  save: () => Promise<void>;
  dirty: () => boolean;
}

let checkTimer: ReturnType<typeof setTimeout> | undefined;

export const useSettings = create<SettingsState>((set, get) => ({
  loaded: null,
  draft: null,
  rulesCheck: null,
  saving: false,
  message: null,
  error: null,
  load: async () => {
    try {
      const s = await ipc.getSettings();
      set({ loaded: s, draft: structuredClone(s), error: null, message: null });
      get().setRules(s.rulesText);
    } catch (e) {
      set({ error: String(e) });
    }
  },
  patch: (p) => {
    const d = get().draft;
    if (d) set({ draft: { ...d, ...p }, message: null });
  },
  setWatch: (i, p) => {
    const d = get().draft;
    if (!d) return;
    const watches = d.watches.map((w, j) => (j === i ? { ...w, ...p } : w));
    set({ draft: { ...d, watches }, message: null });
  },
  addWatch: (path) => {
    const d = get().draft;
    if (!d || d.watches.some((w) => w.path === path)) return;
    const w: WatchFolder = {
      path,
      kind: path.startsWith("\\\\") ? "network" : "local",
      recursive: false,
      indexContent: true,
      pollIntervalS: null,
    };
    set({ draft: { ...d, watches: [...d.watches, w] }, message: null });
  },
  removeWatch: (i) => {
    const d = get().draft;
    if (d) set({ draft: { ...d, watches: d.watches.filter((_, j) => j !== i) }, message: null });
  },
  setRules: (text) => {
    get().patch({ rulesText: text });
    clearTimeout(checkTimer);
    checkTimer = setTimeout(async () => {
      try {
        set({ rulesCheck: await ipc.checkRules(text) });
      } catch (e) {
        set({ rulesCheck: { ok: false, rules: [], error: String(e) } });
      }
    }, 250);
  },
  save: async () => {
    const d = get().draft;
    if (!d) return;
    set({ saving: true, error: null, message: null });
    try {
      await ipc.saveSettings({
        watches: d.watches,
        dbPathCustom: d.dbPathCustom && d.dbPathCustom.trim() ? d.dbPathCustom.trim() : null,
        quietMode: d.quietMode,
        hotkey: d.hotkey,
        rulesText: d.rulesText,
      });
      await get().load();
      set({ message: "저장했습니다. 감시를 다시 시작하고 인덱스를 갱신합니다." });
    } catch (e) {
      set({ error: String(e) });
    } finally {
      set({ saving: false });
    }
  },
  dirty: () => {
    const { loaded, draft } = get();
    return !!loaded && !!draft && JSON.stringify(loaded) !== JSON.stringify(draft);
  },
}));
