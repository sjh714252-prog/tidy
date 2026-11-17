import { create } from "zustand";
import * as ipc from "../ipc/commands";
import type { JobInfo } from "../ipc/types";

interface JobState {
  /** (0.18.1) The long job running now (apply / undo…), or null. */
  job: JobInfo | null;
  set: (job: JobInfo | null) => void;
  load: () => Promise<void>;
}

export const useJob = create<JobState>((set) => ({
  job: null,
  set: (job) => set({ job }),
  load: async () => {
    try {
      set({ job: await ipc.currentJob() });
    } catch {
      // No answer: no bar.
    }
  },
}));
