import { create } from "zustand";
import * as ipc from "../ipc/commands";
import { useStatus } from "./status";
import type { ApplyResult, ContextSuggestion, CurateEstimate, MoveEdit, Plan, Preview, Report, ReviewResult } from "../ipc/types";

interface ReportState {
  roots: string[];
  /** (M19) Organize roots: choosing one means "정리 위치 점검". */
  reviewRoots: string[];
  /** (M19) What the last review found (counts, AI files waiting). */
  reviewInfo: Omit<ReviewResult, "plan" | "preview"> | null;
  /** (M19) A review running ("ai" = with the AI). */
  reviewing: "rules" | "ai" | "rebuild" | null;
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
  /** (0.11.1) From the home card: files no rule takes and sub-folders —
   * what a rules preview leaves where it is. */
  leftover: { files: number; folders: number } | null;

  loadRoots: () => Promise<void>;
  /** (M19) "정리 위치 점검": look inside the organize root and propose
   * re-sorting it; `useAi` also places customer files outside the
   * customer folders. Nothing moves. */
  review: (useAi?: boolean) => Promise<void>;
  /** (0.16.0) "정리 위치 새로 짜기": re-plan the whole organize root. */
  rebuild: () => Promise<void>;
  setPath: (p: string) => Promise<void>;
  setRecursive: (b: boolean) => void;
  setDups: (b: boolean) => void;
  setProtect: (p: string) => void;
  build: () => Promise<void>;
  makePlan: (kind: "rules" | "ai" | "context", recursive?: boolean) => Promise<void>;
  /** "업무 맥락으로 정리": "모으기" guard → cost notice → confirm → plan. */
  curate: (skipGuard?: boolean) => Promise<void>;
  /** (0.11.3) The one main button: a rules preview first; when no rule
   * moves anything and the AI is on, straight on to the work context
   * (cost notice + confirm first). Nothing moves. */
  start: (recursive?: boolean) => Promise<void>;
  /** (0.12.0) "모든 감시 폴더 훑기": pick every watch folder and start the
   * work-context plan (cost notice + confirm first). Nothing moves. */
  surveyAll: () => Promise<void>;
  /** (0.11.3) Place 미분류 work-context items in one click (nothing moves). */
  settle: (how: "guess" | "by_kind", ids?: number[]) => Promise<void>;
  settling: boolean;
  /** (M12) "파일별로 나누기": re-analyze one folder item's files. */
  splitFolder: (moveId: number) => Promise<void>;
  /** (M10) Change customer / issue / kind of one work-context item. */
  editMove: (moveId: number, edit: MoveEdit) => Promise<void>;
  learn: (picked: ContextSuggestion[]) => Promise<void>;
  dropSuggestions: () => void;
  openPlan: (id: number) => Promise<void>;
  /** (0.11.2) "제안 파일 열기…": pick a plan file → saved plan + preview. */
  importPlan: () => Promise<void>;
  toggle: (id: number) => void;
  setAll: (on: boolean) => void;
  apply: () => Promise<void>;
  closePlan: () => void;
}

/** (0.12.0) The "정리할 폴더" choice meaning every watch folder at once
 * (work context only: one plan, laid out under the organize root). */
export const ALL_WATCHES = "*all-watches*";

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
  reviewRoots: [],
  reviewInfo: null,
  reviewing: null,
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
  leftover: null,
  settling: false,

  loadRoots: async () => {
    try {
      const roots = await ipc.reportRoots();
      const reviewRoots = await ipc.reviewRoots().catch(() => [] as string[]);
      set({ roots, reviewRoots });
      if (!get().path && roots[0]) await get().setPath(roots[0]);
    } catch (e) {
      set({ error: String(e) });
    }
  },
  setPath: async (path) => {
    set({
      path,
      report: null,
      plan: null,
      preview: null,
      applied: null,
      target: null,
      suggestCurate: false,
      leftover: null,
      reviewInfo: null,
    });
    if (!path || path === ALL_WATCHES) return;
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
    if (!path || path === ALL_WATCHES) return;
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
    if (path === ALL_WATCHES && kind !== "context") {
      set({ error: "모든 감시 폴더는 업무 맥락으로만 한 번에 정리합니다. 규칙·AI 구조 제안은 폴더를 하나 고르세요." });
      return;
    }
    set({ planning: kind, error: null, applied: null, suggestions: [], learnt: null });
    try {
      const r =
        kind === "ai"
          ? await ipc.planAi(path, protectList(protect))
          : kind === "context"
            ? path === ALL_WATCHES
              ? await ipc.planContextAll(protectList(protect))
              : await ipc.planContext(path, protectList(protect))
            : await ipc.planRules(path, recursive, protectList(protect));
      set({ plan: r.plan, preview: r.preview, selected: defaultSelected(r.plan) });
    } catch (e) {
      set({ error: String(e) });
    } finally {
      set({ planning: null });
    }
  },
  curate: async (skipGuard = false) => {
    const { path, target } = get();
    if (!path || (!skipGuard && !guardTarget(target))) return;
    try {
      const est = path === ALL_WATCHES ? await ipc.curateEstimateAll() : await ipc.curateEstimate(path);
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
  review: async (useAi = false) => {
    const { path, protect } = get();
    if (!path || path === ALL_WATCHES) return;
    set({ reviewing: useAi ? "ai" : "rules", error: null, applied: null, suggestions: [], learnt: null });
    try {
      const { plan, preview, ...info } = await ipc.planReview(path, useAi, protectList(protect));
      set({
        plan,
        preview,
        selected: plan ? defaultSelected(plan) : new Set(),
        reviewInfo: info,
      });
    } catch (e) {
      set({ error: String(e) });
    } finally {
      set({ reviewing: null });
    }
  },
  rebuild: async () => {
    const { path, protect } = get();
    if (!path || path === ALL_WATCHES) return;
    set({ reviewing: "rebuild", error: null, applied: null, suggestions: [], learnt: null });
    try {
      const { plan, preview, ...info } = await ipc.planRebuild(path, protectList(protect));
      set({
        plan,
        preview,
        selected: plan ? defaultSelected(plan) : new Set(),
        reviewInfo: info,
      });
    } catch (e) {
      set({ error: String(e) });
    } finally {
      set({ reviewing: null });
    }
  },
  start: async (recursive) => {
    const st = get();
    if (st.reviewRoots.includes(st.path)) {
      await get().review(false);
      return;
    }
    if (st.path === ALL_WATCHES) {
      const ai = useStatus.getState().status?.aiState;
      if (ai === "ready" || ai === "starting") await get().curate(true);
      else set({ error: "모든 감시 폴더를 한 번에 훑는 정리는 AI가 필요합니다 — 설정 → AI 연결에서 켜 주세요." });
      return;
    }
    if (!st.path || !guardTarget(st.target)) return;
    await get().makePlan("rules", recursive ?? st.recursive);
    const after = get();
    const ai = useStatus.getState().status?.aiState;
    const aiOn = ai === "ready" || ai === "starting";
    if (aiOn && after.plan?.source === "rules" && after.plan.moves.length === 0 && !after.error) {
      // target already confirmed above: skip the second guard.
      await get().curate(true);
    }
  },
  surveyAll: async () => {
    await get().setPath(ALL_WATCHES);
    await get().curate(true);
  },
  settle: async (how, ids) => {
    const { plan, selected } = get();
    if (!plan || plan.applied_batch) return;
    set({ settling: true, error: null });
    try {
      const r = await ipc.settleContextItems(plan.id, how, ids ?? null, [...selected]);
      const sel = new Set([...selected].filter((id) => r.plan.moves.some((x) => x.id === id && x.from !== x.to)));
      const before = new Map(plan.moves.map((m) => [m.id, m.from !== m.to]));
      for (const m of r.plan.moves) if (m.from !== m.to && !before.get(m.id)) sel.add(m.id);
      set({ plan: r.plan, preview: r.preview, selected: sel });
    } catch (e) {
      set({ error: String(e) });
    } finally {
      set({ settling: false });
    }
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
  importPlan: async () => {
    const file = await ipc.pickPlanFile();
    if (!file) return;
    set({ planning: "rules", error: null, applied: null, suggestions: [] });
    try {
      const r = await ipc.importPlan(file);
      set({ path: r.plan.root, plan: r.plan, preview: r.preview, selected: defaultSelected(r.plan), leftover: null });
    } catch (e) {
      set({ error: String(e) });
    } finally {
      set({ planning: null });
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
  closePlan: () => set({ plan: null, preview: null, applied: null, suggestions: [], learnt: null, reviewInfo: null }),
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
