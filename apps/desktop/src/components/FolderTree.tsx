import { useEffect, useState } from "react";
import * as ipc from "../ipc/commands";
import type { DirEntry, FsRoot } from "../ipc/types";

interface NodeState {
  open: boolean;
  loading: boolean;
  children: DirEntry[] | null;
  error: string | null;
}

/**
 * Explorer-style folder picker (SPEC §14.1). Starts from drives, home and
 * the usual folders, the watch folders and their collect roots. Children
 * are read only when a node is opened (network drives are slow); a share
 * that does not answer shows "응답 없음" on that node only. "새 폴더"
 * creates the folder right away through the mover.
 */
export function FolderTree({
  selected,
  onSelect,
  extraRoots = [],
  source,
}: {
  selected: string | null;
  onSelect: (path: string) => void;
  /** Folders to show on top (e.g. a collect root not saved yet). */
  extraRoots?: FsRoot[];
  /** (M16) Another machine's folders (a NAS): its roots and listing; no
   * "새 폴더" (the NAS makes folders itself when it moves files). */
  source?: { roots: FsRoot[]; list: (path: string) => Promise<DirEntry[]> };
}) {
  const [roots, setRoots] = useState<FsRoot[]>([]);
  const [nodes, setNodes] = useState<Record<string, NodeState>>({});
  const [error, setError] = useState<string | null>(null);
  // The tree keeps its own selection (the parent may only want the result).
  const [sel, setSel] = useState<string | null>(selected);
  const pick = (p: string) => {
    setSel(p);
    onSelect(p);
  };

  useEffect(() => {
    if (source) setRoots(source.roots);
    else ipc.fsRoots().then(setRoots, (e) => setError(String(e)));
  }, [source]);

  const load = async (path: string) => {
    setNodes((n) => ({ ...n, [path]: { open: true, loading: true, children: n[path]?.children ?? null, error: null } }));
    try {
      const children = await (source ? source.list(path) : ipc.listDirs(path));
      setNodes((n) => ({ ...n, [path]: { open: true, loading: false, children, error: null } }));
    } catch (e) {
      setNodes((n) => ({ ...n, [path]: { open: true, loading: false, children: [], error: String(e) } }));
    }
  };

  const toggle = (path: string) => {
    const st = nodes[path];
    if (st?.open) setNodes((n) => ({ ...n, [path]: { ...st, open: false } }));
    else void load(path);
  };

  const newFolder = async () => {
    if (!sel) return;
    const name = window.prompt("새 폴더 이름", "새 폴더")?.trim();
    if (!name) return;
    try {
      const made = await ipc.makeDir(sel, name);
      await load(sel);
      pick(made);
    } catch (e) {
      setError(String(e));
    }
  };

  const row = (e: { name: string; path: string }, depth: number, label?: string) => {
    const st = nodes[e.path];
    return (
      <li key={e.path}>
        <div
          className={sel === e.path ? "tree-row sel" : "tree-row"}
          style={{ paddingLeft: 6 + depth * 14 }}
          data-path={e.path}
        >
          <button className="twisty" onClick={() => toggle(e.path)} aria-label={st?.open ? "접기" : "펼치기"}>
            {st?.loading ? "…" : st?.open ? "▾" : "▸"}
          </button>
          <button className="tree-name" onClick={() => pick(e.path)} title={e.path}>
            📁 {label ?? e.name}
          </button>
        </div>
        {st?.open && (
          <ul>
            {st.error && (
              <li className="warn small" style={{ paddingLeft: 26 + depth * 14 }}>
                {st.error}
              </li>
            )}
            {st.children?.map((c) => row(c, depth + 1))}
            {st.children && !st.children.length && !st.error && (
              <li className="muted small" style={{ paddingLeft: 26 + depth * 14 }}>
                (하위 폴더 없음)
              </li>
            )}
          </ul>
        )}
      </li>
    );
  };

  const all = [...extraRoots.filter((r) => !roots.some((x) => x.path === r.path)), ...roots];
  return (
    <div className="folder-tree">
      <ul>{all.map((r) => row({ name: r.label, path: r.path }, 0, r.label))}</ul>
      <div className="tree-actions">
        <span className="muted small path">{sel ?? "폴더를 고르세요"}</span>
        {!source && (
          <button disabled={!sel} onClick={() => void newFolder()}>
            새 폴더
          </button>
        )}
      </div>
      {error && <p className="error small">{error}</p>}
    </div>
  );
}
