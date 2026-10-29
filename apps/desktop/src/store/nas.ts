import { create } from "zustand";
import * as ipc from "../ipc/commands";
import type { NasState } from "../ipc/types";

/**
 * (M16) Connected NAS agents and their live state. Loaded on demand (home,
 * settings, search window focus); a NAS that does not answer is just shown
 * as "연결 안 됨" — nothing else waits on it.
 */
interface NasStore {
  list: NasState[];
  loaded: boolean;
  loading: boolean;
  error: string | null;
  load: () => Promise<void>;
  /** Replace one entry after connect / refresh. */
  put: (n: NasState) => void;
  drop: (name: string) => void;
}

export const useNas = create<NasStore>((set, get) => ({
  list: [],
  loaded: false,
  loading: false,
  error: null,
  load: async () => {
    if (get().loading) return;
    set({ loading: true });
    try {
      set({ list: await ipc.nasList(), loaded: true, loading: false, error: null });
    } catch (e) {
      set({ loaded: true, loading: false, error: String(e) });
    }
  },
  put: (n) =>
    set((s) => ({
      list: s.list.some((x) => x.name === n.name) ? s.list.map((x) => (x.name === n.name ? n : x)) : [...s.list, n],
    })),
  drop: (name) => set((s) => ({ list: s.list.filter((x) => x.name !== name) })),
}));

/** Last folder name of a NAS path ("/data/업무/받은 파일" → "받은 파일"). */
export function nasFolderName(p: string): string {
  const parts = p.split("/").filter(Boolean);
  return parts[parts.length - 1] ?? p;
}
