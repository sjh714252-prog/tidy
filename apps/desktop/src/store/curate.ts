import { create } from "zustand";
import * as ipc from "../ipc/commands";
import type { ContextCandidates, ContextDict, CuratePatch, CurateSettings } from "../ipc/types";

/** Settings → 업무 맥락 (M10): [curate] + context.toml, saved by the one save button. */
interface CurateState {
  draft: CurateSettings | null;
  loaded: CurateSettings | null;
  busy: boolean;
  extracting: boolean;
  candidates: ContextCandidates | null;
  message: string | null;
  error: string | null;
  load: (force?: boolean) => Promise<void>;
  dirty: () => boolean;
  patch: (p: Partial<CuratePatch>) => void;
  setDict: (d: ContextDict) => void;
  extract: () => Promise<void>;
  /** Put the ticked / renamed candidates into the draft dictionary. */
  takeCandidates: (picked: ContextCandidates) => void;
  dropCandidates: () => void;
  save: () => Promise<boolean>;
}

const editable = (s: CurateSettings) => ({ settings: s.settings, dict: s.dict });

export const useCurate = create<CurateState>((set, get) => ({
  draft: null,
  loaded: null,
  busy: false,
  extracting: false,
  candidates: null,
  message: null,
  error: null,
  load: async (force = false) => {
    try {
      const fresh = await ipc.getCurateSettings();
      if (force || !get().dirty()) set({ draft: fresh, loaded: fresh, error: null });
      else set({ loaded: fresh });
    } catch (e) {
      set({ error: String(e) });
    }
  },
  dirty: () => {
    const { draft, loaded } = get();
    return !!draft && !!loaded && JSON.stringify(editable(draft)) !== JSON.stringify(editable(loaded));
  },
  patch: (p) => {
    const d = get().draft;
    if (d) set({ draft: { ...d, settings: { ...d.settings, ...p } }, message: null });
  },
  setDict: (dict) => {
    const d = get().draft;
    if (d) set({ draft: { ...d, dict }, message: null });
  },
  extract: async () => {
    set({ extracting: true, error: null, candidates: null });
    try {
      set({ candidates: await ipc.extractContext() });
    } catch (e) {
      set({ error: String(e) });
    } finally {
      set({ extracting: false });
    }
  },
  takeCandidates: (picked) => {
    const d = get().draft;
    if (!d) return;
    const dict: ContextDict = structuredClone(d.dict);
    for (const e of picked.entities) {
      const cur = dict.entity.find((x) => x.name === e.name);
      if (cur) cur.aliases = [...new Set([...cur.aliases, ...e.aliases])];
      else dict.entity.push({ name: e.name, aliases: e.aliases });
    }
    for (const i of picked.issue_types)
      if (!dict.issue_type.some((x) => x.name === i.name)) dict.issue_type.push({ name: i.name });
    for (const k of picked.doc_kinds)
      if (!dict.doc_kind.some((x) => x.name === k.name)) dict.doc_kind.push({ name: k.name, ext: k.ext });
    set({ draft: { ...d, dict }, candidates: null, message: "사전에 넣었습니다 — 저장하면 기록됩니다." });
  },
  dropCandidates: () => set({ candidates: null }),
  save: async () => {
    const d = get().draft;
    if (!d) return false;
    set({ busy: true, error: null, message: null });
    try {
      const saved = await ipc.saveCurateSettings(d);
      set({ draft: saved, loaded: saved, message: "업무 맥락 설정과 사전을 저장했습니다." });
      return true;
    } catch (e) {
      set({ error: String(e) });
      return false;
    } finally {
      set({ busy: false });
    }
  },
}));
