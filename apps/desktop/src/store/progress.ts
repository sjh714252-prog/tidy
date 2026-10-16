import { create } from "zustand";
import type { TransferProgress } from "../ipc/types";

interface ProgressState {
  /** The copy-verify move running now (null when none). */
  current: TransferProgress | null;
  update: (p: TransferProgress) => void;
}

let clearTimer: ReturnType<typeof setTimeout> | undefined;

/** Live NAS copy progress (only long copies report, every ~250 ms). */
export const useProgress = create<ProgressState>((set) => ({
  current: null,
  update: (p) => {
    clearTimeout(clearTimer);
    set({ current: p });
    // "done" stays visible briefly; a stalled copy clears after a minute.
    clearTimer = setTimeout(() => set({ current: null }), p.phase === "done" ? 1500 : 60_000);
  },
}));
