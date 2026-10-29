import { create } from "zustand";
import * as ipc from "../ipc/commands";
import type { ApplyResult, CommandResult } from "../ipc/types";

/** (M16, SPEC §14.13) 홈 "말로 시키기": 명령 → 미리보기 → [옮기기] → 되돌리기. */
interface CommandState {
  text: string;
  busy: boolean;
  error: string | null;
  result: CommandResult | null;
  /** Ticked plan move ids. */
  selected: Set<number>;
  applied: ApplyResult | null;
  undone: string | null;
  setText: (t: string) => void;
  run: (rulesOnly?: boolean) => Promise<void>;
  toggle: (id: number) => void;
  setAll: (on: boolean) => void;
  apply: () => Promise<void>;
  undo: () => Promise<void>;
  reset: () => void;
}

export const useCommand = create<CommandState>((set, get) => ({
  text: "",
  busy: false,
  error: null,
  result: null,
  selected: new Set(),
  applied: null,
  undone: null,
  setText: (text) => set({ text }),
  run: async (rulesOnly = false) => {
    const text = get().text.trim();
    if (!text || get().busy) return;
    set({ busy: true, error: null, result: null, applied: null, undone: null });
    try {
      const result = await ipc.runCommand(text, rulesOnly);
      set({ result, selected: new Set(result.plan?.moves.map((m) => m.id) ?? []) });
    } catch (e) {
      set({ error: String(e).replace(/^Error:\s*/, "") });
    } finally {
      set({ busy: false });
    }
  },
  toggle: (id) => {
    const s = new Set(get().selected);
    if (s.has(id)) s.delete(id);
    else s.add(id);
    set({ selected: s });
  },
  setAll: (on) => set({ selected: new Set(on ? (get().result?.plan?.moves.map((m) => m.id) ?? []) : []) }),
  apply: async () => {
    const plan = get().result?.plan;
    if (!plan || get().busy || !get().selected.size) return;
    set({ busy: true, error: null });
    try {
      set({ applied: await ipc.applyPlan(plan.id, [...get().selected], []) });
    } catch (e) {
      set({ error: String(e).replace(/^Error:\s*/, "") });
    } finally {
      set({ busy: false });
    }
  },
  undo: async () => {
    const a = get().applied;
    if (!a || get().busy) return;
    set({ busy: true });
    try {
      const u = await ipc.undoBatch(a.batchId);
      set({
        undone:
          `${u.restored.length}개를 원래 자리로 되돌렸습니다` +
          (u.conflicts.length ? ` · 되돌리지 못함 ${u.conflicts.length}개: ${u.conflicts.join("; ")}` : ""),
      });
    } catch (e) {
      set({ error: String(e).replace(/^Error:\s*/, "") });
    } finally {
      set({ busy: false });
    }
  },
  reset: () => set({ result: null, applied: null, undone: null, error: null, selected: new Set() }),
}));
