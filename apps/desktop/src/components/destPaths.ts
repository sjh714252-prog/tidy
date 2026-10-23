// (M11, SPEC §14.6) Rule destinations that ignore "지정 위치로 모으기".
// Same rules as core `classify::rules_doc::{is_absolute_dest, relative_suggestion}`.

/** `C:\…`, `D:/…`, `\\server\…`, `/…`, `~/…` — not relative, not `{root}`. */
export function isAbsoluteDest(dest: string): boolean {
  const d = dest.trim();
  return /^[a-zA-Z]:/.test(d) || d.startsWith("/") || d.startsWith("\\") || d === "~" || /^~[\\/]/.test(d);
}

/** Last component, or the last two when the last holds a `{variable}`. */
export function relativeSuggestion(dest: string): string {
  const parts = dest
    .split(/[\\/]/)
    .map((p) => p.trim())
    .filter((p) => p && p !== "~" && !/^[a-zA-Z]:$/.test(p));
  if (!parts.length) return "";
  const last = parts[parts.length - 1] ?? "";
  const take = last.includes("{") && parts.length >= 2 ? 2 : 1;
  return parts.slice(parts.length - take).join("/");
}

/** The folder part of a destination (up to the first `{variable}`). */
export function destFolder(dest: string): string {
  const i = dest.indexOf("{");
  return (i >= 0 ? dest.slice(0, i) : dest).replace(/[\\/]+$/, "");
}
