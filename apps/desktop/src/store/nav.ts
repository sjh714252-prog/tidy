import { create } from "zustand";
import type { MainTab, SettingsKey } from "../ipc/types";

/**
 * Main window tab + settings page, kept in the URL hash (SPEC §14.8):
 * `#/main`, `#/main/pending`, `#/main/report`, `#/main/history`,
 * `#/main/settings/<key>`.
 */
export const SETTINGS_KEYS: SettingsKey[] = ["watch", "rules", "context", "ai", "security", "nas", "advanced"];
const TABS: MainTab[] = ["activity", "pending", "report", "history", "settings"];

interface NavState {
  tab: MainTab;
  section: SettingsKey;
  setTab: (t: MainTab) => void;
  /** Settings page (also "AI 연결에서 켜기"). */
  goSettings: (section: string) => void;
}

function parse(hash: string): { tab: MainTab; section: SettingsKey } {
  const parts = hash.replace(/^#\/?/, "").split("?")[0]?.split("/") ?? [];
  // parts[0] === "main"
  const t = (parts[1] ?? "") as MainTab;
  const tab: MainTab = TABS.includes(t) ? t : t === ("home" as MainTab) ? "activity" : "activity";
  const s = (parts[2] ?? "") as SettingsKey;
  return { tab, section: SETTINGS_KEYS.includes(s) ? s : "watch" };
}

function hashOf(tab: MainTab, section: SettingsKey): string {
  if (tab === "activity") return "#/main";
  if (tab === "settings") return `#/main/settings/${section}`;
  return `#/main/${tab}`;
}

function write(tab: MainTab, section: SettingsKey) {
  const h = hashOf(tab, section);
  if (window.location.hash !== h) window.history.replaceState(null, "", h);
}

const start = parse(window.location.hash);

export const useNav = create<NavState>((set, get) => ({
  tab: start.tab,
  section: start.section,
  setTab: (tab) => {
    set({ tab });
    write(tab, get().section);
  },
  goSettings: (key) => {
    const section = SETTINGS_KEYS.includes(key as SettingsKey) ? (key as SettingsKey) : get().section;
    set({ tab: "settings", section });
    write("settings", section);
  },
}));

// Back/forward, links and typed addresses.
window.addEventListener("hashchange", () => {
  if (!window.location.hash.startsWith("#/main")) return;
  const p = parse(window.location.hash);
  const cur = useNav.getState();
  if (p.tab !== cur.tab || p.section !== cur.section) useNav.setState(p);
});
