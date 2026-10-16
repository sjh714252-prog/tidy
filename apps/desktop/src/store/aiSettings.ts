import { create } from "zustand";
import * as ipc from "../ipc/commands";
import type { AiProvider, AiSettings } from "../ipc/types";

interface AiSettingsState {
  draft: AiSettings | null;
  /** Typed into the key field; sent once on save, then cleared. Never loaded back. */
  apiKey: string;
  busy: boolean;
  message: string | null;
  error: string | null;
  load: () => Promise<void>;
  patch: (p: Partial<AiSettings>) => void;
  setProvider: (p: AiProvider) => void;
  setApiKey: (k: string) => void;
  save: () => Promise<boolean>;
  deleteKey: () => Promise<void>;
  test: () => Promise<void>;
}

/** Sensible defaults per provider (model names are only suggestions). */
const DEFAULTS: Record<Exclude<AiProvider, "">, Partial<AiSettings>> = {
  anthropic: { model: "claude-haiku-5-5", baseUrl: "", keyRef: "tidy/anthropic", effort: "low" },
  openai: { model: "", baseUrl: "https://api.openai.com/v1", keyRef: "tidy/openai", effort: "" },
  ollama: { model: "qwen3:8b", baseUrl: "http://localhost:11434", keyRef: "", effort: "" },
};

export const useAiSettings = create<AiSettingsState>((set, get) => ({
  draft: null,
  apiKey: "",
  busy: false,
  message: null,
  error: null,
  load: async () => {
    try {
      set({ draft: await ipc.getAiSettings(), error: null });
    } catch (e) {
      set({ error: String(e) });
    }
  },
  patch: (p) => {
    const d = get().draft;
    if (d) set({ draft: { ...d, ...p }, message: null });
  },
  setProvider: (provider) => {
    const d = get().draft;
    if (!d) return;
    const defaults = provider ? DEFAULTS[provider] : {};
    set({ draft: { ...d, ...defaults, provider, keyStored: false }, message: null });
  },
  setApiKey: (apiKey) => set({ apiKey, message: null }),
  save: async () => {
    const d = get().draft;
    if (!d) return false;
    set({ busy: true, error: null, message: null });
    try {
      const saved = await ipc.saveAiSettings(d, get().apiKey.trim() || null);
      set({ draft: saved, apiKey: "", message: "AI 설정을 저장했습니다. 감시를 다시 시작합니다." });
      return true;
    } catch (e) {
      set({ error: String(e) });
      return false;
    } finally {
      set({ busy: false });
    }
  },
  deleteKey: async () => {
    const d = get().draft;
    if (!d?.keyRef) return;
    try {
      const s = await ipc.deleteApiKey(d.keyRef);
      set({ draft: { ...d, keyStored: s.keyStored }, message: "키체인에서 키를 지웠습니다." });
    } catch (e) {
      set({ error: String(e) });
    }
  },
  test: async () => {
    set({ busy: true, error: null, message: null });
    try {
      set({ message: await ipc.testAi() });
    } catch (e) {
      set({ error: String(e) });
    } finally {
      set({ busy: false });
    }
  },
}));
