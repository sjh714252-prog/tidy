import { create } from "zustand";
import type { Activity } from "../ipc/types";

const MAX = 300;

interface ActivityState {
  items: Activity[];
  push: (a: Activity) => void;
  clear: () => void;
}

/** Live feed of what the watcher did (kept in memory, newest first). */
export const useActivity = create<ActivityState>((set) => ({
  items: [],
  push: (a) => set((s) => ({ items: [a, ...s.items].slice(0, MAX) })),
  clear: () => set({ items: [] }),
}));
