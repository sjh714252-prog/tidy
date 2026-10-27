// (M13, SPEC §14.8) Watch folder aliases and middle-ellipsis path forms.

const KNOWN: { names: string[]; alias: string }[] = [
  { names: ["desktop", "바탕 화면", "바탕화면"], alias: "바탕화면" },
  { names: ["downloads", "다운로드"], alias: "다운로드" },
  { names: ["documents", "문서"], alias: "문서" },
  { names: ["pictures", "사진"], alias: "사진" },
];

export function baseName(p: string): string {
  return p.split(/[\\/]/).filter(Boolean).pop() ?? p;
}

/** OS name of the well-known folders, else the last folder name. */
export function defaultAlias(path: string): string {
  const last = baseName(path);
  const k = KNOWN.find((x) => x.names.includes(last.toLowerCase()));
  return k ? k.alias : last;
}

/** Is this one of the well-known folders (blue icon in settings)? */
export function knownFolder(path: string): string | null {
  const last = baseName(path).toLowerCase();
  return KNOWN.find((x) => x.names.includes(last))?.alias ?? null;
}

export function aliasOf(path: string, alias?: string | null): string {
  return alias?.trim() ? alias.trim() : defaultAlias(path);
}

export interface WatchName {
  path: string;
  alias: string;
}

const norm = (p: string) => p.replace(/\\/g, "/").replace(/\/+$/, "");

/** Watch folder `p` is in (longest match), if any. */
export function watchOf(p: string, watches: WatchName[]): WatchName | null {
  const n = norm(p);
  let best: WatchName | null = null;
  for (const w of watches) {
    const r = norm(w.path);
    if ((n === r || n.startsWith(r + "/")) && (!best || r.length > norm(best.path).length)) best = w;
  }
  return best;
}

/** "바탕화면 › 가온누리원 로그" for a path inside a watch folder, else null. */
export function aliasPath(p: string, alias: string, root: string): string | null {
  const n = norm(p);
  const r = norm(root);
  if (n === r) return alias;
  if (!n.startsWith(r + "/")) return null;
  const rest = n.slice(r.length + 1).split("/").filter(Boolean);
  return [alias, ...rest].join(" › ");
}

/** Split into the root prefix ("C:\\", "\\\\srv\\share\\", "/", "~/") and segments. */
function splitPath(p: string): { root: string; segs: string[]; sep: string } {
  const sep = p.includes("\\") && !p.startsWith("/") ? "\\" : "/";
  let root = "";
  let rest = p;
  const unc = p.match(/^(\\\\[^\\]+\\[^\\]+\\?)/);
  const drive = p.match(/^([a-zA-Z]:[\\/]?)/);
  if (unc?.[1]) root = unc[1];
  else if (drive?.[1]) root = drive[1];
  else if (p.startsWith("~/") || p.startsWith("~\\")) root = p.slice(0, 2);
  else if (p.startsWith("/")) root = "/";
  rest = p.slice(root.length);
  if (root && !/[\\/]$/.test(root)) root += sep;
  return { root, segs: rest.split(/[\\/]/).filter(Boolean), sep };
}

/**
 * Shortened forms of `p`, longest first: the full path, then head (root +
 * first 1–2 parts) + "…" + tail (last 2–1 parts), then "…" + last part.
 * A form is only offered when it hides at least one part.
 */
export function pathForms(p: string): string[] {
  const { root, segs, sep } = splitPath(p);
  const out = new Set<string>([p]);
  const forms: string[] = [];
  for (const h of [2, 1]) {
    for (const t of [2, 1]) {
      if (segs.length - h - t < 1) continue;
      forms.push(root + [...segs.slice(0, h), "…", ...segs.slice(segs.length - t)].join(sep));
    }
  }
  if (segs.length >= 2) forms.push(["…", segs[segs.length - 1]].join(sep));
  forms.sort((a, b) => b.length - a.length).forEach((f) => out.add(f));
  return [...out];
}

/** Longest form of `p` with at most `max` characters (the shortest if none fit). */
export function shortenTo(p: string, max: number): string {
  const forms = pathForms(p);
  return forms.find((f) => f.length <= max) ?? forms[forms.length - 1] ?? p;
}
