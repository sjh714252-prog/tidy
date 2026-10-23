import { create } from "zustand";
import * as ipc from "../ipc/commands";
import type { ApplyResult, ContextSuggestion, CurateEstimate, MoveEdit, Plan, Preview, Report } from "../ipc/types";

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
  /** (M11) Include sub-folders in a rules plan. (M12) Not for work
   * context: there sub-folders are whole items. */
  recursive: boolean;
  /** (M12) A folder item being split into its files. */
  splitting: number | null;
  /** (M11) The "모으기" root the chosen folder is (or lies in): organizing it re-moves results. */
  target: string | null;
  /** (M11) "지금 정리" with AI + a context dictionary: highlight "업무 맥락으로 정리". */
  suggestCurate: boolean;

  loadRoots: () => Promise<void>;
  setPath: (p: string) => Promise<void>;
  setRecursive: (b: boolean) => void;
  setDups: (b: boolean) => void;
  setProtect: (p: string) => void;
  build: () => Promise<void>;
  makePlan: (kind: "rules" | "ai" | "context", recursive?: boolean) => Promise<void>;
  /** "업무 맥락으로 정리": "모으기" guard → cost notice → confirm → plan. */
  curate: () => Promise<void>;
  /** (M12) "파일별로 나누기": re-analyze one folder item's files. */
  splitFolder: (moveId: number) => Promise<void>;
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
  recursive: false,
  splitting: null,
  target: null,
  suggestCurate: false,

  loadRoots: async () => {
    try {
      const roots = await ipc.reportRoots();
      set({ roots });
      if (!get().path && roots[0]) await get().setPath(roots[0]);
    } catch (e) {
      set({ error: String(e) });
    }
  },
  setPath: async (path) => {
    set({ path, report: null, plan: null, preview: null, applied: null, target: null, suggestCurate: false });
    if (!path) return;
    try {
      const target = await ipc.organizeTarget(path);
      if (get().path === path) set({ target });
    } catch {
      // Only a warning: no answer = no warning.
    }
  },
  setRecursive: (recursive) => set({ recursive }),
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
            ? await ipc.planContext(path, protectList(protect))
            : await ipc.planRules(path, recursive, protectList(protect));
      set({ plan: r.plan, preview: r.preview, selected: defaultSelected(r.plan) });
    } catch (e) {
      set({ error: String(e) });
    } finally {
      set({ planning: null });
    }
  },
  curate: async () => {
    const { path, target } = get();
    if (!path || !guardTarget(target)) return;
    try {
      const est = await ipc.curateEstimate(path);
      if (!est.files) {
        window.alert(est.unsettled ? `${unsettledText(est.unsettled)}\n\n지금 정리할 항목은 없습니다.` : "정리할 파일이 없습니다.");
        return;
      }
      if (!window.confirm(`${estimateText(est)}\n\n분석만 하고 아무것도 옮기지 않습니다. 진행할까요?`)) return;
    } catch (e) {
      window.alert(String(e));
      return;
    }
    await get().makePlan("context");
  },
  splitFolder: async (moveId) => {
    const { plan, selected, suggestions } = get();
    if (!plan || plan.applied_batch) return;
    set({ splitting: moveId, error: null });
    try {
      const r = await ipc.splitContextFolder(plan.id, moveId, [...selected]);
      // Keep what was ticked; the folder's files come in ticked.
      const before = new Set(plan.moves.map((m) => m.id));
      const sel = new Set(
        [...selected].filter((id) => id !== moveId && r.plan.moves.some((x) => x.id === id && x.from !== x.to)),
      );
      for (const m of r.plan.moves) if (!before.has(m.id) && m.from !== m.to) sel.add(m.id);
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
      set({ splitting: null });
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

/** (M11) Organizing a "모으기" root re-moves files that are already sorted:
 * two confirmations before any plan there. */
export function guardTarget(target: string | null): boolean {
  return (
    !target ||
    (window.confirm(
      `이 폴더는 정리 결과가 모이는 곳입니다(${target}). 여기서 정리하면 이미 정리된 파일을 다시 옮깁니다. 그래도 미리보기를 만들까요?`,
    ) &&
      window.confirm("정말 정리 위치를 다시 정리할까요? (미리보기만 만들고, 적용 전에는 아무것도 옮기지 않습니다)"))
  );
}

const unsettledText = (n: number) => `복사 중이거나 방금 바뀐 폴더 ${n}개는 이번에 빼고, 조용해지면 다음에 정리합니다.`;

/** The cost notice shown before a work-context analysis (SPEC §14.3, §14.7). */
export function estimateText(e: CurateEstimate): string {
  if (!e.files) return "정리할 파일이 없습니다.";
  const what = e.folders
    ? `항목 ${e.files}개(폴더 ${e.folders}개는 통째로, 파일 ${e.files - e.folders}개)`
    : `파일 ${e.files}개`;
  const head = `${what} 중 ${e.to_analyze}개를 AI로 분석합니다`;
  const tail = e.unsettled ? `\n${unsettledText(e.unsettled)}` : "";
  if (!e.to_analyze) return `${head} — 모두 이미 분석해 두어 AI를 다시 부르지 않습니다(비용 없음).${tail}`;
  const cost =
    e.cost_usd != null ? `, 약 $${e.cost_usd.toFixed(e.cost_usd < 0.01 ? 4 : 2)}` : " (이 모델의 요금은 알 수 없음)";
  return (
    `${head}${e.cached ? ` (이미 분석한 ${e.cached}개는 다시 묻지 않음)` : ""}.\n` +
    `요청 ${e.requests}번 · 예상 토큰 입력 약 ${e.input_tokens.toLocaleString()} / 출력 약 ${e.output_tokens.toLocaleString()}${cost}` +
    `${e.model ? ` · ${e.model}` : ""}${tail}`
  );
}
