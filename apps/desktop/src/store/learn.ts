import { create } from "zustand";
import * as ipc from "../ipc/commands";
import type { LearnSuggestion } from "../ipc/types";

interface LearnState {
  items: LearnSuggestion[];
  message: string | null;
  error: string | null;
  refresh: () => Promise<void>;
  accept: (s: LearnSuggestion) => Promise<void>;
  dismiss: (s: LearnSuggestion) => Promise<void>;
}

/** Learner rule suggestions (repeated identical decisions → a rule). */
export const useLearn = create<LearnState>((set, get) => ({
  items: [],
  message: null,
  error: null,
  refresh: async () => {
    try {
      set({ items: await ipc.listLearn(), error: null });
    } catch (e) {
      set({ error: String(e) });
    }
  },
  accept: async (s) => {
    try {
      await ipc.acceptLearn(s.key);
      set({ message: `규칙 추가: ${s.ruleName}`, error: null });
    } catch (e) {
      set({ error: String(e) });
    }
    await get().refresh();
  },
  dismiss: async (s) => {
    try {
      await ipc.dismissLearn(s.key);
      set({ message: `다시 제안하지 않음: ${s.ruleName}`, error: null });
    } catch (e) {
      set({ error: String(e) });
    }
    await get().refresh();
  },
}));
