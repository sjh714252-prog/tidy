import { create } from "zustand";
import * as ipc from "../ipc/commands";
import type { ApplyResult, Plan, Preview, Report } from "../ipc/types";

interface ReportState {
  roots: string[];
  path: string;
  dups: boolean;
  report: Report | null;
  loading: boolean;
  error: string | null;

  /** "건드리지 말 폴더" for this request (one per line). */
  protect: string;
  plan: Plan | null;
  preview: Preview | null;
  selected: Set<number>;
  planning: "rules" | "ai" | null;
  applying: boolean;
  applied: ApplyResult | null;

  loadRoots: () => Promise<void>;
  setPath: (p: string) => void;
  setDups: (b: boolean) => void;
  setProtect: (p: string) => void;
  build: () => Promise<void>;
  makePlan: (kind: "rules" | "ai", recursive?: boolean) => Promise<void>;
  openPlan: (id: number) => Promise<void>;
  toggle: (id: number) => void;
  setAll: (on: boolean) => void;
  apply: () => Promise<void>;
  closePlan: () => void;
}

const protectList = (s: string) =>
  s
    .split("\n")
    .map((l) => l.trim())
    .filter(Boolean);

let previewTimer: ReturnType<typeof setTimeout> | undefined;

export const useReport = create<ReportState>((set, get) => ({
  roots: [],
  path: "",
  dups: true,
  report: null,
  loading: false,
  error: null,
  protect: "",
  plan: null,
  preview: null,
  selected: new Set(),
  planning: null,
  applying: false,
  applied: null,

  loadRoots: async () => {
    try {
      const roots = await ipc.reportRoots();
      set({ roots, path: get().path || roots[0] || "" });
    } catch (e) {
      set({ error: String(e) });
    }
  },
  setPath: (path) => set({ path, report: null, plan: null, preview: null, applied: null }),
  setDups: (dups) => set({ dups }),
  setProtect: (protect) => set({ protect }),
  build: async () => {
    const { path, dups } = get();
    if (!path) return;
    set({ loading: true, error: null });
    try {
      set({ report: await ipc.buildReport(path, dups) });
    } catch (e) {
      set({ error: String(e) });
    } finally {
      set({ loading: false });
    }
  },
  makePlan: async (kind, recursive = false) => {
    const { path, protect } = get();
    if (!path) return;
    set({ planning: kind, error: null, applied: null });
    try {
      const r =
        kind === "ai"
          ? await ipc.planAi(path, protectList(protect))
          : await ipc.planRules(path, recursive, protectList(protect));
      set({ plan: r.plan, preview: r.preview, selected: new Set(r.plan.moves.map((m) => m.id)) });
    } catch (e) {
      set({ error: String(e) });
    } finally {
      set({ planning: null });
    }
  },
  openPlan: async (id) => {
    try {
      const r = await ipc.openPlan(id);
      set({ plan: r.plan, preview: r.preview, selected: new Set(r.plan.moves.map((m) => m.id)), applied: null });
    } catch (e) {
      set({ error: String(e) });
    }
  },
  toggle: (id) => {
    const selected = new Set(get().selected);
    if (selected.has(id)) selected.delete(id);
    else selected.add(id);
    set({ selected });
    refreshPreview();
  },
  setAll: (on) => {
    const plan = get().plan;
    set({ selected: on && plan ? new Set(plan.moves.map((m) => m.id)) : new Set() });
    refreshPreview();
  },
  apply: async () => {
    const { plan, selected, protect } = get();
    if (!plan || !selected.size) return;
    set({ applying: true, error: null });
    try {
      const applied = await ipc.applyPlan(plan.id, [...selected], protectList(protect));
      set({ applied, plan: { ...plan, applied_batch: applied.batchId } });
    } catch (e) {
      set({ error: String(e) });
    } finally {
      set({ applying: false });
    }
  },
  closePlan: () => set({ plan: null, preview: null, applied: null }),
}));

/** Re-render the before/after trees after the selection settles. */
function refreshPreview() {
  clearTimeout(previewTimer);
  previewTimer = setTimeout(async () => {
    const { plan, selected } = useReport.getState();
    if (!plan) return;
    try {
      useReport.setState({ preview: await ipc.previewPlan(plan.id, [...selected]) });
    } catch (e) {
      useReport.setState({ error: String(e) });
    }
  }, 200);
}
