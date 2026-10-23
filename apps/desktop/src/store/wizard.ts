import { create } from "zustand";
import * as ipc from "../ipc/commands";
import type { FinishResult, PreviewReport, RuleCard, WatchFolder, WizardInfo } from "../ipc/types";
import { emptyCard } from "../ipc/types";

export interface WizardFolder extends WatchFolder {
  label: string;
  checked: boolean;
}

interface WizardState {
  step: number;
  info: WizardInfo | null;
  folders: WizardFolder[];
  starters: Set<string>;
  custom: RuleCard[];
  /** Independent counts per starter (and custom row) over the checked folders. */
  counts: PreviewReport | null;
  finishing: boolean;
  result: FinishResult | null;
  error: string | null;
  load: () => Promise<void>;
  go: (step: number) => void;
  setFolder: (i: number, p: Partial<WizardFolder>) => void;
  addFolder: (path: string) => Promise<void>;
  toggleStarter: (id: string) => void;
  setCustom: (i: number, p: Partial<RuleCard>) => void;
  addCustom: () => void;
  removeCustom: (i: number) => void;
  refreshCounts: () => Promise<void>;
  finish: () => Promise<void>;
}

const folder = (path: string, label: string, checked: boolean, network = false): WizardFolder => ({
  path,
  label,
  checked,
  kind: network ? "network" : "local",
  recursive: false,
  indexContent: !network,
  pollIntervalS: null,
  pollOnly: false,
  contentIndexWindow: null,
  mode: "in_place",
  organizeRoot: null,
  exclude: [],
});

let countTimer: ReturnType<typeof setTimeout> | undefined;

/**
 * First-run wizard state (SPEC §14.1). Everything stays here until
 * "감시 시작": no config or rules file is written before that, and the
 * app does not watch (or move) anything until setup is finished.
 */
export const useWizard = create<WizardState>((set, get) => ({
  step: 1,
  info: null,
  folders: [],
  starters: new Set(["screenshots", "installers", "documents"]),
  custom: [],
  counts: null,
  finishing: false,
  result: null,
  error: null,
  load: async () => {
    try {
      const info = await ipc.wizardInfo();
      const folders: WizardFolder[] = info.watches.map((w) => ({
        ...w,
        label: w.path.split(/[\\/]/).filter(Boolean).pop() ?? w.path,
        checked: true,
      }));
      for (const s of info.suggested) {
        if (!folders.some((f) => f.path === s.path)) {
          folders.push(folder(s.path, s.label, s.checked && !info.watches.length));
        }
      }
      set({ info, folders, step: 1, result: null, error: null });
      void get().refreshCounts();
    } catch (e) {
      set({ error: String(e) });
    }
  },
  go: (step) => {
    set({ step, error: null });
    if (step === 2) void get().refreshCounts();
  },
  setFolder: (i, p) => {
    set({ folders: get().folders.map((f, j) => (j === i ? { ...f, ...p } : f)) });
    clearTimeout(countTimer);
    countTimer = setTimeout(() => void get().refreshCounts(), 300);
  },
  addFolder: async (path) => {
    if (get().folders.some((f) => f.path === path)) return;
    const network = await ipc.isNetworkPath(path).catch(() => path.startsWith("\\\\"));
    const label = path.split(/[\\/]/).filter(Boolean).pop() ?? path;
    set({ folders: [...get().folders, folder(path, label, true, network)] });
    void get().refreshCounts();
  },
  toggleStarter: (id) => {
    const s = new Set(get().starters);
    if (s.has(id)) s.delete(id);
    else s.add(id);
    set({ starters: s });
  },
  setCustom: (i, p) => {
    set({ custom: get().custom.map((c, j) => (j === i ? { ...c, ...p } : c)) });
    clearTimeout(countTimer);
    countTimer = setTimeout(() => void get().refreshCounts(), 300);
  },
  addCustom: () => set({ custom: [...get().custom, { ...emptyCard(), name: "", dest: "" }] }),
  removeCustom: (i) => set({ custom: get().custom.filter((_, j) => j !== i) }),
  refreshCounts: async () => {
    const { info, folders, custom } = get();
    if (!info) return;
    const watches = folders.filter((f) => f.checked);
    const cards = [
      ...info.starters.map((s) => s.card),
      ...custom.filter((c) => c.name.trim() && c.dest.trim() && (c.ext.length || c.nameContains.trim())),
    ];
    try {
      set({ counts: await ipc.previewRules(watches, cards, false) });
    } catch {
      set({ counts: null });
    }
  },
  finish: async () => {
    const { folders, starters, custom } = get();
    set({ finishing: true, error: null });
    try {
      const result = await ipc.finishOnboarding({
        watches: folders.filter((f) => f.checked),
        starters: [...starters],
        custom: custom.filter((c) => c.name.trim() && c.dest.trim()),
      });
      set({ result, step: 5 });
    } catch (e) {
      set({ error: String(e) });
    } finally {
      set({ finishing: false });
    }
  },
}));
