import { create } from "zustand";
import * as ipc from "../ipc/commands";
import type { Status } from "../ipc/types";
import { aliasOf, type WatchName } from "../components/paths";

interface StatusState {
  status: Status | null;
  error: string | null;
  refresh: () => Promise<void>;
  setPaused: (paused: boolean) => Promise<void>;
}

export const useStatus = create<StatusState>((set) => ({
  status: null,
  error: null,
  refresh: async () => {
    try {
      set({ status: await ipc.getStatus(), error: null });
    } catch (e) {
      set({ error: String(e) });
    }
  },
  setPaused: async (paused) => {
    try {
      set({ status: await ipc.setPaused(paused), error: null });
    } catch (e) {
      set({ error: String(e) });
    }
  },
}));

const EMPTY: WatchName[] = [];
let lastKey = "";
let lastNames: WatchName[] = EMPTY;

/** (M13) Watch folders with their display names (alias or default). */
export function watchNames(status: Status | null): WatchName[] {
  if (!status) return EMPTY;
  const key = JSON.stringify([status.watchPaths, status.watchAliases]);
  if (key !== lastKey) {
    lastKey = key;
    lastNames = status.watchPaths.map((path, i) => ({ path, alias: aliasOf(path, status.watchAliases?.[i]) }));
  }
  return lastNames;
}

export const useWatchNames = () => useStatus((s) => watchNames(s.status));
