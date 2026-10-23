import { create } from "zustand";
import * as ipc from "../ipc/commands";
import type { FolderOverview, Overview } from "../ipc/types";
import { useNav } from "./nav";
import { useReport } from "./report";

/** (M11, SPEC §14.6) 홈 탭: 감시 현황 패널, 정리 현황, 정보 창. */
interface HomeState {
  overview: Overview | null;
  error: string | null;
  /** 감시 현황 패널 (status bar summary toggles it). */
  panelOpen: boolean;
  aboutOpen: boolean;
  /** Run an update check when the about window opens. */
  checkOnOpen: boolean;
  load: (refresh?: boolean) => Promise<void>;
  togglePanel: () => void;
  openAbout: (check?: boolean) => void;
  closeAbout: () => void;
}

export const useHome = create<HomeState>((set) => ({
  overview: null,
  error: null,
  panelOpen: false,
  aboutOpen: false,
  checkOnOpen: false,
  load: async (refresh = false) => {
    try {
      set({ overview: await ipc.watchOverview(refresh), error: null });
    } catch (e) {
      set({ error: String(e) });
    }
  },
  togglePanel: () => {
    const open = !useHome.getState().panelOpen;
    set({ panelOpen: open });
    if (open) useNav.getState().setTab("activity");
  },
  openAbout: (check = false) => set({ aboutOpen: true, checkOnOpen: check }),
  closeAbout: () => set({ aboutOpen: false, checkOnOpen: false }),
}));

/**
 * "지금 정리": go to 정리하기 with this folder and run the rules preview
 * (nothing moves until the user applies). The work-context button is
 * highlighted when it can be used, never started on its own (it costs).
 */
export async function organizeNow(f: FolderOverview, canCurate: boolean) {
  useNav.getState().setTab("report");
  const r = useReport.getState();
  await r.setPath(f.path);
  useReport.setState({ suggestCurate: canCurate, recursive: f.recursive });
  await useReport.getState().makePlan("rules", f.recursive);
}

/** Last two path parts ("…\\바탕 화면"); the full path goes in a title. */
export function shortPath(p: string): string {
  const parts = p.split(/[\\/]/).filter(Boolean);
  if (parts.length <= 2) return p;
  const sep = p.includes("\\") ? "\\" : "/";
  return `…${sep}${parts.slice(-2).join(sep)}`;
}

export function baseName(p: string): string {
  return p.split(/[\\/]/).filter(Boolean).pop() ?? p;
}
