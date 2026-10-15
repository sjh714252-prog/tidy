import { create } from "zustand";
import * as ipc from "../ipc/commands";
import type { SearchHit } from "../ipc/types";

interface SearchState {
  query: string;
  hits: SearchHit[];
  selected: number;
  loading: boolean;
  error: string | null;
  setQuery: (q: string) => void;
  move: (delta: number) => void;
  select: (i: number) => void;
}

let seq = 0;
let timer: ReturnType<typeof setTimeout> | undefined;

export const useSearch = create<SearchState>((set, get) => ({
  query: "",
  hits: [],
  selected: 0,
  loading: false,
  error: null,
  setQuery: (query) => {
    set({ query });
    clearTimeout(timer);
    if (!query.trim()) {
      set({ hits: [], selected: 0, loading: false, error: null });
      return;
    }
    // Debounce typing; drop responses that arrive out of order.
    timer = setTimeout(async () => {
      const mine = ++seq;
      set({ loading: true });
      try {
        const hits = await ipc.search(query, 50);
        if (mine === seq) set({ hits, selected: 0, loading: false, error: null });
      } catch (e) {
        if (mine === seq) set({ hits: [], loading: false, error: String(e) });
      }
    }, 120);
  },
  move: (delta) => {
    const { hits, selected } = get();
    if (!hits.length) return;
    set({ selected: (selected + delta + hits.length) % hits.length });
  },
  select: (i) => set({ selected: i }),
}));
