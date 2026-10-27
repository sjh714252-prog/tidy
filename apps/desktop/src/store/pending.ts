import { create } from "zustand";
import * as ipc from "../ipc/commands";
import type { Decision, PendingItem, Resolution } from "../ipc/types";

interface PendingState {
  items: PendingItem[];
  /** Selected ids in the queue table. */
  checked: Set<number>;
  busy: boolean;
  error: string | null;
  lastResult: string | null;
  /** (M14) The identical file a "그대로 둠" result refers to ("기존 파일 열기"). */
  lastExisting: string | null;
  refresh: () => Promise<void>;
  toggle: (id: number) => void;
  toggleAll: () => void;
  resolve: (id: number, d: Decision) => Promise<Resolution | null>;
  /** Apply one decision to every checked item (queue screen bulk action). */
  resolveChecked: (d: Decision) => Promise<void>;
}

export function describe(r: Resolution): string {
  switch (r.kind) {
    case "moved":
      return `이동함 → ${r.dst}`;
    case "duplicate":
      return r.message ?? `똑같은 파일이 이미 있어 그대로 둠 (${r.dst})`;
    case "already_in_place":
      return "이미 그 폴더에 있음";
    case "skipped":
      return "건너뜀 (이번만)";
    case "ignored":
      return "무시 (앞으로 정리하지 않음)";
    case "gone":
      return "파일이 없어 확인 대기에서 뺌";
  }
}

export const usePending = create<PendingState>((set, get) => ({
  items: [],
  checked: new Set(),
  busy: false,
  error: null,
  lastResult: null,
  lastExisting: null,
  refresh: async () => {
    try {
      const items = await ipc.listPending();
      const ids = new Set(items.map((i) => i.id));
      const checked = new Set([...get().checked].filter((id) => ids.has(id)));
      set({ items, checked });
    } catch (e) {
      set({ error: String(e) });
    }
  },
  toggle: (id) => {
    const checked = new Set(get().checked);
    if (checked.has(id)) checked.delete(id);
    else checked.add(id);
    set({ checked });
  },
  toggleAll: () => {
    const { items, checked } = get();
    set({ checked: checked.size === items.length ? new Set() : new Set(items.map((i) => i.id)) });
  },
  resolve: async (id, d) => {
    set({ busy: true, error: null });
    try {
      const r = await ipc.resolvePending(id, d);
      set({ lastResult: describe(r), lastExisting: r.kind === "duplicate" ? r.dst : null });
      return r;
    } catch (e) {
      set({ error: String(e).replace(/^Error:\s*/, ""), lastResult: null, lastExisting: null });
      return null;
    } finally {
      set({ busy: false });
      await get().refresh();
    }
  },
  resolveChecked: async (d) => {
    const ids = [...get().checked];
    set({ busy: true, error: null });
    const failed: string[] = [];
    let ok = 0;
    try {
      for (const r of await ipc.resolvePendingBatch(ids, d)) {
        if ("Ok" in r) ok++;
        else failed.push(r.Err);
      }
    } catch (e) {
      failed.push(String(e));
    }
    set({
      busy: false,
      checked: new Set(),
      lastResult: `${ok}건 처리${failed.length ? `, ${failed.length}건 실패` : ""}`,
      lastExisting: null,
      error: failed[0] ?? null,
    });
    await get().refresh();
  },
}));
