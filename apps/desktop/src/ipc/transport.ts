// IPC transport: Tauri when running inside the app window, otherwise (dev
// only) the HTTP test bridge of the running app (src-tauri/src/devbridge.rs).
//
// The bridge exists because WebView2 ignores synthetic input from automation
// tools: the same bundle opened in a normal browser at the Vite URL can be
// clicked and inspected, while every command and event still goes to the
// real backend.
import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { listen as tauriListen, type UnlistenFn } from "@tauri-apps/api/event";

export const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

const BRIDGE = "http://127.0.0.1:1421";

type Listener = (payload: unknown) => void;
const listeners = new Map<string, Set<Listener>>();
let polling = false;

async function poll() {
  if (polling) return;
  polling = true;
  let since = 0;
  // Start from "now": events emitted before the page opened are history.
  try {
    const r = await fetch(`${BRIDGE}/events?since=1000000000`);
    since = ((await r.json()) as { next: number }).next;
  } catch {
    /* bridge not up yet; the loop below keeps retrying */
  }
  for (;;) {
    try {
      const r = await fetch(`${BRIDGE}/events?since=${since}`);
      const data = (await r.json()) as { next: number; events: { name: string; payload: unknown }[] };
      since = data.next;
      for (const ev of data.events) {
        listeners.get(ev.name)?.forEach((cb) => cb(ev.payload));
      }
    } catch {
      await new Promise((res) => setTimeout(res, 2000));
    }
  }
}

async function bridgeInvoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const r = await fetch(`${BRIDGE}/invoke/${cmd}`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(args ?? {}),
  });
  const data = (await r.json()) as { ok: boolean; value?: T; error?: string };
  if (!data.ok) throw new Error(data.error ?? "bridge error");
  return data.value as T;
}

async function bridgeListen<T>(name: string, cb: (e: { payload: T }) => void): Promise<UnlistenFn> {
  const fn: Listener = (p) => cb({ payload: p as T });
  let set = listeners.get(name);
  if (!set) listeners.set(name, (set = new Set()));
  set.add(fn);
  void poll();
  return () => {
    set?.delete(fn);
  };
}

function noBridge(): never {
  throw new Error("Tidy 앱 밖에서는 동작하지 않습니다 (개발 빌드의 테스트 브리지 전용)");
}

export const invoke: <T>(cmd: string, args?: Record<string, unknown>) => Promise<T> = inTauri
  ? tauriInvoke
  : import.meta.env.DEV
    ? bridgeInvoke
    : noBridge;

export const listen: <T>(name: string, cb: (e: { payload: T }) => void) => Promise<UnlistenFn> = inTauri
  ? (name, cb) => tauriListen(name, cb)
  : import.meta.env.DEV
    ? bridgeListen
    : noBridge;

export type { UnlistenFn };

/** Hide the current window; a no-op in the browser (bridge) where there is none. */
export async function hideCurrentWindow(): Promise<void> {
  if (!inTauri) return;
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  await getCurrentWindow().hide();
}

/** This page is the main window (not search / confirm / wizard). */
export function inMainWindow(): boolean {
  const h = typeof window === "undefined" ? "" : window.location.hash;
  return !h || h === "#/" || h.startsWith("#/main");
}
