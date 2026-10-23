import { create } from "zustand";
import * as ipc from "../ipc/commands";
import type { EditorState, RuleCard, RulesCheck, Settings, WatchFolder } from "../ipc/types";
import { emptyCard } from "../ipc/types";

interface SettingsState {
  loaded: Settings | null;
  draft: Settings | null;
  rulesCheck: RulesCheck | null;
  /** Visual editor (M8): cards of rulesText + first-match counts. */
  editor: EditorState | null;
  cards: RuleCard[] | null;
  /** Cards that came from "AI로 규칙 만들기" (badge; nothing saved yet). */
  aiMarked: Set<number>;
  drafting: boolean;
  draftNote: string | null;
  saving: boolean;
  message: string | null;
  error: string | null;
  load: () => Promise<void>;
  patch: (p: Partial<Settings>) => void;
  setWatch: (i: number, p: Partial<WatchFolder>) => void;
  addWatch: (path: string) => Promise<void>;
  removeWatch: (i: number) => void;
  setRules: (text: string) => void;
  setCards: (cards: RuleCard[]) => void;
  addCard: () => void;
  aiDraft: () => Promise<void>;
  save: () => Promise<void>;
  dirty: () => boolean;
}

let checkTimer: ReturnType<typeof setTimeout> | undefined;
let cardsTimer: ReturnType<typeof setTimeout> | undefined;
let seq = 0;
/** Resolves when the last card edit reached rulesText (save waits for it). */
let pendingSync: Promise<void> = Promise.resolve();

const checkOf = (e: EditorState): RulesCheck => ({
  ok: e.ok,
  rules: (e.cards ?? []).filter((c) => c.enabled).map((c) => c.name),
  error: e.error,
});

export const useSettings = create<SettingsState>((set, get) => ({
  loaded: null,
  draft: null,
  rulesCheck: null,
  editor: null,
  cards: null,
  aiMarked: new Set(),
  drafting: false,
  draftNote: null,
  saving: false,
  message: null,
  error: null,
  load: async () => {
    try {
      const s = await ipc.getSettings();
      set({ loaded: s, draft: structuredClone(s), error: null, message: null, aiMarked: new Set(), draftNote: null });
      get().setRules(s.rulesText);
    } catch (e) {
      set({ error: String(e) });
    }
  },
  patch: (p) => {
    const d = get().draft;
    if (d) set({ draft: { ...d, ...p }, message: null });
  },
  setWatch: (i, p) => {
    const d = get().draft;
    if (!d) return;
    const watches = d.watches.map((w, j) => (j === i ? { ...w, ...p } : w));
    set({ draft: { ...d, watches }, message: null });
  },
  addWatch: async (path) => {
    if (get().draft?.watches.some((w) => w.path === path)) return;
    // NAS folders: polling + copy-verify moves, metadata-only index (SPEC §5.9).
    const network = await ipc.isNetworkPath(path).catch(() => path.startsWith("\\\\"));
    const d = get().draft;
    if (!d) return;
    const w: WatchFolder = {
      path,
      kind: network ? "network" : "local",
      recursive: false,
      indexContent: !network,
      pollIntervalS: null,
      pollOnly: false,
      contentIndexWindow: null,
      mode: "in_place",
      organizeRoot: null,
      exclude: [],
    };
    set({ draft: { ...d, watches: [...d.watches, w] }, message: null });
  },
  removeWatch: (i) => {
    const d = get().draft;
    if (d) set({ draft: { ...d, watches: d.watches.filter((_, j) => j !== i) }, message: null });
  },
  // TOML ("고급") → cards. A file that does not parse leaves the cards
  // read-only with the error shown.
  setRules: (text) => {
    get().patch({ rulesText: text });
    clearTimeout(checkTimer);
    const mine = ++seq;
    checkTimer = setTimeout(async () => {
      try {
        const e = await ipc.rulesEditor(text, true);
        if (mine !== seq) return;
        set({ editor: e, rulesCheck: checkOf(e), cards: e.cards ?? get().cards });
      } catch (err) {
        if (mine === seq) set({ rulesCheck: { ok: false, rules: [], error: String(err) } });
      }
    }, 250);
  },
  // Cards → TOML, regenerated on top of the current text so comments and
  // keys the cards do not know survive. Card values stay as typed; only
  // their positions (origin) follow the regenerated file.
  setCards: (cards) => {
    set({ cards });
    clearTimeout(cardsTimer);
    const mine = ++seq;
    pendingSync = new Promise((resolve) => {
      cardsTimer = setTimeout(async () => {
        const base = get().draft?.rulesText ?? "";
        try {
          const e = await ipc.rulesFromCards(base, cards, true);
          if (mine === seq) {
            get().patch({ rulesText: e.text });
            set({
              editor: e,
              rulesCheck: checkOf(e),
              cards: cards.map((c, i) => ({ ...c, origin: i })),
            });
          }
        } catch (err) {
          if (mine === seq) {
            const msg = String(err);
            set({ rulesCheck: { ok: false, rules: [], error: msg }, editor: { ...(get().editor as EditorState), error: msg, ok: false } });
          }
        } finally {
          resolve();
        }
      }, 300);
    });
  },
  addCard: () => {
    const cards = get().cards ?? [];
    get().setCards([...cards, { ...emptyCard(), name: `새 규칙 ${cards.length + 1}`, dest: "정리/{yyyy}-{mm}" }]);
  },
  aiDraft: async () => {
    set({ drafting: true, draftNote: null });
    try {
      const d = await ipc.aiRuleDrafts();
      const cards = get().cards ?? [];
      const marked = new Set(get().aiMarked);
      d.drafts.forEach((_, k) => marked.add(cards.length + k));
      set({
        aiMarked: marked,
        draftNote:
          `${d.summary} — 제안 ${d.drafts.length}개를 아래에 추가했습니다(저장 전까지 아무것도 쓰지 않음)` +
          (d.rejected.length ? `, 거부 ${d.rejected.length}개: ${d.rejected.map((r) => `${r.name}(${r.reason})`).join(", ")}` : ""),
      });
      get().setCards([...cards, ...d.drafts.map((x) => ({ ...x.card, origin: null }))]);
    } catch (e) {
      set({ draftNote: `AI 규칙 제안 실패: ${String(e)}` });
    } finally {
      set({ drafting: false });
    }
  },
  save: async () => {
    await pendingSync;
    const d = get().draft;
    if (!d) return;
    set({ saving: true, error: null, message: null });
    try {
      await ipc.saveSettings({
        watches: d.watches,
        dbPathCustom: d.dbPathCustom && d.dbPathCustom.trim() ? d.dbPathCustom.trim() : null,
        quietMode: d.quietMode,
        hotkey: d.hotkey,
        rulesText: d.rulesText,
        startOnLogin: d.autostart.wanted,
        checkUpdates: d.checkUpdates,
      });
      await get().load();
      set({ message: "저장했습니다. 감시를 다시 시작하고 인덱스를 갱신합니다." });
    } catch (e) {
      set({ error: String(e) });
    } finally {
      set({ saving: false });
    }
  },
  dirty: () => {
    const { loaded, draft } = get();
    return !!loaded && !!draft && JSON.stringify(loaded) !== JSON.stringify(draft);
  },
}));
