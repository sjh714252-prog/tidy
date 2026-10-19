import { create } from "zustand";
import * as ipc from "../ipc/commands";
import type { PiiSettings, SecuritySettings } from "../ipc/types";

interface SecurityState {
  draft: SecuritySettings | null;
  /** Last saved values; `dirty()` compares against it. */
  loaded: SecuritySettings | null;
  /** Typed into the password fields; sent once on save, never loaded back. */
  password: string;
  password2: string;
  busy: boolean;
  message: string | null;
  error: string | null;
  /** Reload; unsaved edits are kept unless `force` (same rule as #8). */
  load: (force?: boolean) => Promise<void>;
  dirty: () => boolean;
  patch: (p: Partial<SecuritySettings>) => void;
  patchPii: (p: Partial<PiiSettings>) => void;
  setPassword: (a: string, b: string) => void;
  /** Problem that blocks saving (password mismatch / too short, no vault folder). */
  invalid: () => string | null;
  save: () => Promise<boolean>;
}

/** Fields the user edits (read-only state is left out of `dirty`). */
const editable = (s: SecuritySettings) => ({
  mode: s.mode,
  vaultDir: s.vaultDir,
  approval: s.approval,
  pii: s.pii,
});

export const MIN_PASSWORD = 8;

export const useSecurity = create<SecurityState>((set, get) => ({
  draft: null,
  loaded: null,
  password: "",
  password2: "",
  busy: false,
  message: null,
  error: null,
  load: async (force = false) => {
    try {
      const fresh = await ipc.getSecuritySettings();
      if (force || !get().dirty()) {
        set({ draft: fresh, loaded: fresh, password: "", password2: "", error: null });
      } else {
        // Keep the edits, refresh what is read-only.
        const d = get().draft;
        set({ loaded: fresh, draft: d ? { ...fresh, ...editable(d) } : fresh });
      }
    } catch (e) {
      set({ error: String(e) });
    }
  },
  dirty: () => {
    const { loaded, draft, password, password2 } = get();
    if (password || password2) return true;
    return !!loaded && !!draft && JSON.stringify(editable(loaded)) !== JSON.stringify(editable(draft));
  },
  patch: (p) => {
    const d = get().draft;
    if (d) set({ draft: { ...d, ...p }, message: null });
  },
  patchPii: (p) => {
    const d = get().draft;
    if (d) set({ draft: { ...d, pii: { ...d.pii, ...p } }, message: null });
  },
  setPassword: (password, password2) => set({ password, password2, message: null }),
  invalid: () => {
    const { draft, password, password2 } = get();
    if (password || password2) {
      if ([...password].length < MIN_PASSWORD) return `비밀번호는 ${MIN_PASSWORD}자 이상이어야 합니다`;
      if (password !== password2) return "두 비밀번호가 다릅니다";
    }
    if (draft?.mode === "vault" && !draft.vaultDir.trim()) return "보안 폴더로 모으려면 보안 폴더를 고르세요";
    return null;
  },
  save: async () => {
    const d = get().draft;
    if (!d) return false;
    const bad = get().invalid();
    if (bad) {
      set({ error: bad });
      return false;
    }
    set({ busy: true, error: null, message: null });
    try {
      const saved = await ipc.saveSecuritySettings(d, get().password || null);
      const changedPw = !!get().password;
      set({
        draft: saved,
        loaded: saved,
        password: "",
        password2: "",
        message: changedPw
          ? `보안 설정을 저장했습니다. 비밀번호는 ${saved.keyStore}에만 저장했습니다.`
          : "보안 설정을 저장했습니다.",
      });
      return true;
    } catch (e) {
      set({ error: String(e) });
      return false;
    } finally {
      set({ busy: false });
    }
  },
}));
