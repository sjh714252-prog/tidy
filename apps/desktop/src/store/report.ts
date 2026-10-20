import { create } from "zustand";
import * as ipc from "../ipc/commands";
import type { ApplyResult, ContextSuggestion, MoveEdit, Plan, Preview, Report } from "../ipc/types";

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
  planning: "rules" | "ai" | "context" | null;
  /** (M10) Dictionary changes learnt from edits in a work-context plan (confirm to save). */
  suggestions: { suggestion: ContextSuggestion; label: string }[];
  editing: number | null;
  learnt: string | null;
  applying: boolean;
  applied: ApplyResult | null;

  loadRoots: () => Promise<void>;
  setPath: (p: string) => void;
  setDups: (b: boolean) => void;
  setProtect: (p: string) => void;
  build: () => Promise<void>;
  makePlan: (kind: "rules" | "ai" | "context", recursive?: boolean) => Promise<void>;
  /** (M10) Change customer / issue / kind of one work-context item. */
  editMove: (moveId: number, edit: MoveEdit) => Promise<void>;
  learn: (picked: ContextSuggestion[]) => Promise<void>;
  dropSuggestions: () => void;
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

/** What a plan ticks by default: everything, except work-context items that
 * stay where they are (미분류 / already in place). */
const defaultSelected = (plan: Plan) =>
  new Set(plan.moves.filter((m) => plan.source !== "context" || m.from !== m.to).map((m) => m.id));

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
  suggestions: [],
  editing: null,
  learnt: null,
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
    set({ planning: kind, error: null, applied: null, suggestions: [], learnt: null });
    try {
      const r =
        kind === "ai"
          ? await ipc.planAi(path, protectList(protect))
          : kind === "context"
            ? await ipc.planContext(path, recursive, protectList(protect))
            : await ipc.planRules(path, recursive, protectList(protect));
      set({ plan: r.plan, preview: r.preview, selected: defaultSelected(r.plan) });
    } catch (e) {
      set({ error: String(e) });
    } finally {
      set({ planning: null });
    }
  },
  openPlan: async (id) => {
    try {
      const r = await ipc.openPlan(id);
      set({ plan: r.plan, preview: r.preview, selected: defaultSelected(r.plan), applied: null, suggestions: [] });
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
    set({ selected: on && plan ? defaultSelected(plan) : new Set() });
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
  editMove: async (moveId, edit) => {
    const { plan, selected, suggestions } = get();
    if (!plan || plan.applied_batch) return;
    set({ editing: moveId, error: null, learnt: null });
    try {
      const r = await ipc.editContextMove(plan.id, moveId, edit, [...selected]);
      const m = r.plan.moves.find((x) => x.id === moveId);
      const sel = new Set([...selected].filter((id) => r.plan.moves.some((x) => x.id === id && x.from !== x.to)));
      if (m && m.from !== m.to) sel.add(moveId);
      const seen = new Set(suggestions.map((x) => x.label));
      set({
        plan: r.plan,
        preview: r.preview,
        selected: sel,
        suggestions: [...suggestions, ...r.suggestions.filter((x) => !seen.has(x.label))],
      });
    } catch (e) {
      set({ error: String(e) });
    } finally {
      set({ editing: null });
    }
  },
  learn: async (picked) => {
    if (!picked.length) return set({ suggestions: [] });
    try {
      await ipc.applyContextSuggestions(picked);
      set({ suggestions: [], learnt: `맥락 사전에 ${picked.length}개를 넣었습니다 — 다음 분석부터 반영됩니다.` });
    } catch (e) {
      set({ error: String(e) });
    }
  },
  dropSuggestions: () => set({ suggestions: [] }),
  closePlan: () => set({ plan: null, preview: null, applied: null, suggestions: [], learnt: null }),
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
