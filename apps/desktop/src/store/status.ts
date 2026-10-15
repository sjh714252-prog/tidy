import { create } from "zustand";
import * as ipc from "../ipc/commands";
import type { Status } from "../ipc/types";

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
