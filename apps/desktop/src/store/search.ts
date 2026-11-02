import { create } from "zustand";
import * as ipc from "../ipc/commands";
import type { NasHit, SearchHit, SemanticStatus } from "../ipc/types";

interface SearchState {
  query: string;
  hits: SearchHit[];
  selected: number;
  loading: boolean;
  /** Waiting for the words + meaning results (after the word results). */
  semanticLoading: boolean;
  /** Mode of the results shown: words only, or words + meaning. */
  mode: "fts" | "hybrid" | null;
  natural: boolean;
  /** Why meaning did not take part. */
  note: string | null;
  semantic: SemanticStatus | null;
  error: string | null;
  /** (M18) Results from connected NAS agents, listed after this PC's. */
  nasHits: NasHit[];
  nasLoading: boolean;
  /** NAS that did not answer ("사무실 NAS: 연결할 수 없습니다"). */
  nasErrors: string[];
  /** (M12) Only this customer's folders ("" = everywhere). */
  customer: string;
  customers: string[];
  setCustomer: (c: string) => void;
  loadCustomers: () => Promise<void>;
  setQuery: (q: string) => void;
  move: (delta: number) => void;
  select: (i: number) => void;
  refreshSemantic: () => Promise<void>;
}

let generation = 0;
let ftsTimer: ReturnType<typeof setTimeout> | undefined;
let semTimer: ReturnType<typeof setTimeout> | undefined;
let nasTimer: ReturnType<typeof setTimeout> | undefined;
/** NAS answers over the network: asked once typing pauses. */
const NAS_DELAY_MS = 300;

/** This PC's results, then the NAS ones (selection runs over both). */
export function allHits(s: { hits: SearchHit[]; nasHits: NasHit[] }): (SearchHit | NasHit)[] {
  return s.nasHits.length ? [...s.hits, ...s.nasHits] : s.hits;
}

export function isNasHit(h: SearchHit | NasHit): h is NasHit {
  return "nas" in h;
}

/** Words answer at once; meaning (an embedding request) once typing pauses. */
const FTS_DELAY_MS = 120;
const SEMANTIC_DELAY_MS = 450;

export const useSearch = create<SearchState>((set, get) => ({
  query: "",
  hits: [],
  selected: 0,
  loading: false,
  semanticLoading: false,
  mode: null,
  natural: false,
  note: null,
  semantic: null,
  error: null,
  nasHits: [],
  nasLoading: false,
  nasErrors: [],
  customer: "",
  customers: [],
  setCustomer: (customer) => {
    set({ customer });
    get().setQuery(get().query);
  },
  loadCustomers: async () => {
    try {
      const customers = await ipc.customerNames();
      set({ customers });
      if (get().customer && !customers.includes(get().customer)) get().setCustomer("");
    } catch {
      // No dictionary / store: no filter offered.
    }
  },
  setQuery: (query) => {
    const customer = get().customer || null;
    set({ query });
    clearTimeout(ftsTimer);
    clearTimeout(semTimer);
    clearTimeout(nasTimer);
    const gen = ++generation;
    if (!query.trim()) {
      set({ hits: [], selected: 0, loading: false, semanticLoading: false, error: null, mode: null, note: null, nasHits: [], nasLoading: false, nasErrors: [] });
      return;
    }
    // Customer folders are this PC's; with that filter the NAS is not asked.
    if (customer) set({ nasHits: [], nasErrors: [], nasLoading: false });
    else
      nasTimer = setTimeout(async () => {
        set({ nasLoading: true });
        try {
          const r = await ipc.nasSearch(query, 30);
          if (gen === generation) set({ nasHits: r.hits, nasErrors: r.errors, nasLoading: false });
        } catch {
          if (gen === generation) set({ nasHits: [], nasErrors: [], nasLoading: false });
        }
      }, NAS_DELAY_MS);
    // Responses of an older query (or a word answer arriving after the
    // meaning answer) are dropped.
    let merged = false;
    ftsTimer = setTimeout(async () => {
      set({ loading: true });
      try {
        const hits = await ipc.search(query, 50, customer);
        if (gen === generation && !merged) set({ hits, selected: 0, loading: false, error: null, mode: "fts" });
      } catch (e) {
        if (gen === generation && !merged) set({ hits: [], loading: false, error: String(e) });
      }
    }, FTS_DELAY_MS);
    if (get().semantic?.state === "off") return;
    semTimer = setTimeout(async () => {
      set({ semanticLoading: true });
      try {
        const r = await ipc.searchSemantic(query, 50, customer);
        if (gen !== generation) return;
        merged = true;
        const keep = get().hits[get().selected]?.path;
        const at = keep ? r.hits.findIndex((h) => h.path === keep) : -1;
        set({
          hits: r.hits,
          selected: at >= 0 ? at : 0,
          mode: r.mode,
          natural: r.natural,
          note: r.note,
          loading: false,
          semanticLoading: false,
          error: null,
        });
      } catch (e) {
        // Word results stay; meaning just did not take part.
        if (gen === generation) set({ semanticLoading: false, note: String(e) });
      }
    }, SEMANTIC_DELAY_MS);
  },
  move: (delta) => {
    const { selected } = get();
    const n = allHits(get()).length;
    if (!n) return;
    set({ selected: (selected + delta + n) % n });
  },
  select: (i) => set({ selected: i }),
  refreshSemantic: async () => {
    try {
      set({ semantic: await ipc.semanticStatus() });
    } catch {
      set({ semantic: null });
    }
  },
}));
