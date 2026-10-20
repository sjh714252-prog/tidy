import { create } from "zustand";
import type { MainTab } from "../ipc/types";

/** Main window tab + which settings section to open/scroll to (M10). */
interface NavState {
  tab: MainTab;
  /** Settings section to open and scroll to once (e.g. "ai"). */
  focus: string | null;
  setTab: (t: MainTab) => void;
  /** "AI 연결에서 켜기": jump to a settings section. */
  goSettings: (section: string) => void;
  clearFocus: () => void;
}

export const useNav = create<NavState>((set) => ({
  tab: "activity",
  focus: null,
  setTab: (tab) => set({ tab }),
  goSettings: (focus) => set({ tab: "settings", focus }),
  clearFocus: () => set({ focus: null }),
}));
