import { useEffect, useRef, useState } from "react";
import * as ipc from "../ipc/commands";
import type { FoundMove, SearchHit } from "../ipc/types";
import { formatTime } from "./format";
import { PathText } from "./PathText";

const BY: Record<string, string> = { rule: "규칙", ai: "AI", user: "확인", restructure: "정리 제안" };

/** One row: a file Tidy moved (with where it came from), or an index hit. */
interface Row {
  key: string;
  name: string;
  /** Where it is now. */
  path: string;
  isDir: boolean;
  exists: boolean;
  moved: FoundMove | null;
  /** Index hit (not in the move log): last modified. */
  mtime: number | null;
}

function fromMove(m: FoundMove): Row {
  return { key: `m:${m.nowPath}`, name: m.name, path: m.nowPath, isDir: m.isDir, exists: m.exists, moved: m, mtime: null };
}

function fromHit(h: SearchHit): Row {
  return { key: `s:${h.path}`, name: h.name, path: h.path, isDir: false, exists: h.exists, moved: null, mtime: h.mtime };
}

function parentOf(p: string): string {
  const i = Math.max(p.lastIndexOf("/"), p.lastIndexOf("\\"));
  return i > 0 ? p.slice(0, i) : p;
}

const DELAY_MS = 200;

/**
 * (0.18.0, SPEC §14.24) 찾기: type part of a name → files Tidy moved (where
 * each is **now**, after later moves) and then the rest of the index. Open
 * the file, its folder, copy the path, or put one file back. Empty box =
 * the most recently organized files.
 */
export function FindFiles({ onUndone }: { onUndone?: () => void }) {
  const [query, setQuery] = useState("");
  const [rows, setRows] = useState<Row[]>([]);
  const [loading, setLoading] = useState(false);
  const [sel, setSel] = useState(0);
  const [msg, setMsg] = useState<string | null>(null);
  const [all, setAll] = useState(false);
  const gen = useRef(0);
  const input = useRef<HTMLInputElement>(null);

  const load = (q: string) => {
    const g = ++gen.current;
    setLoading(true);
    const words = q.trim();
    const moved = ipc.findMoved(words, words ? 30 : 12).catch(() => [] as FoundMove[]);
    // The index knows files Tidy never moved (and files inside a folder moved whole).
    const hits = words ? ipc.search(words, 30).catch(() => [] as SearchHit[]) : Promise.resolve([] as SearchHit[]);
    void Promise.all([moved, hits]).then(([m, h]) => {
      if (g !== gen.current) return;
      const seen = new Set(m.map((x) => x.nowPath));
      setRows([...m.map(fromMove), ...h.filter((x) => !seen.has(x.path)).map(fromHit)]);
      setSel(0);
      setAll(false);
      setLoading(false);
    });
  };

  useEffect(() => {
    const t = setTimeout(() => load(query), query ? DELAY_MS : 0);
    return () => clearTimeout(t);
  }, [query]);

  const act = async (r: Row | undefined, how: "open" | "folder" | "copy") => {
    if (!r) return;
    setMsg(null);
    try {
      if (how === "copy") {
        await navigator.clipboard.writeText(r.path);
        setMsg(`경로를 복사했습니다: ${r.path}`);
      } else if (!r.exists) {
        setMsg(`지금은 그 자리에 없습니다: ${r.path} (지웠거나 다른 프로그램이 옮긴 것 같습니다)`);
        if (how === "folder") await ipc.openFolder(parentOf(r.path));
      } else if (how === "folder") await ipc.revealPath(r.path);
      else await ipc.openPath(r.path);
    } catch (e) {
      setMsg(String(e));
    }
  };

  const putBack = async (m: FoundMove) => {
    if (!window.confirm(`'${m.name}'을(를) 원래 자리로 되돌릴까요?\n${m.srcPath}`)) return;
    try {
      const r = await ipc.undoMove(m.moveId);
      setMsg(
        r.restored.length
          ? `원래 자리로 되돌렸습니다: ${m.srcPath}`
          : `되돌리지 못했습니다: ${r.conflicts.join("; ") || "이유를 알 수 없음"}`,
      );
      load(query);
      onUndone?.();
    } catch (e) {
      setMsg(String(e));
    }
  };

  const shown = all ? rows : rows.slice(0, query.trim() ? 20 : 8);

  const onKey = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setSel((s) => Math.min(s + 1, shown.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setSel((s) => Math.max(s - 1, 0));
    } else if (e.key === "Enter") {
      e.preventDefault();
      void act(shown[sel], e.ctrlKey || e.metaKey ? "folder" : "open");
    } else if (e.key === "Escape" && query) {
      e.preventDefault();
      setQuery("");
    }
  };

  return (
    <section className="card find-files" data-testid="find-files">
      <div className="card-head">
        <h2>파일 찾기</h2>
        <span className="sub">이름 일부만 쳐도 Tidy가 옮긴 파일이 지금 어디 있는지 보여 줍니다 · Enter 열기 · Ctrl+Enter 폴더 열기</span>
      </div>
      <input
        ref={input}
        className="find-input"
        data-testid="find-input"
        placeholder="파일 이름이나 폴더 이름 (예: 견적서, 가람정보 로그)"
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        onKeyDown={onKey}
        spellCheck={false}
      />
      {msg && (
        <div className="notice" data-testid="find-msg">
          {msg}
        </div>
      )}
      <div className="find-head muted small">
        {query.trim()
          ? loading
            ? "찾는 중…"
            : rows.length
              ? `${rows.length}개 찾음`
              : "찾은 파일이 없습니다 — 다른 낱말로 찾아 보거나, 위의 '검색'(Ctrl+Shift+Space)으로 본문까지 찾아 보세요"
          : rows.length
            ? "최근 정리한 파일"
            : loading
              ? ""
              : "아직 정리한 파일이 없습니다"}
      </div>
      <ul className="find-list">
        {shown.map((r, i) => {
          const m = r.moved;
          return (
            <li
              key={r.key}
              className={`find-row${i === sel ? " sel" : ""}${r.exists ? "" : " gone"}`}
              data-testid="find-row"
              onMouseEnter={() => setSel(i)}
              onDoubleClick={() => void act(r, "open")}
            >
              <div className="cell">
                <span className="name">
                  {r.isDir ? "📁 " : ""}
                  {r.name}
                  {m?.encrypted && <span className="tag lock" title="암호화 보관 파일 — 열면 임시 사본으로 풀어 엽니다">🔒</span>}
                  {m?.undone && <span className="tag">원래 자리로 되돌림</span>}
                  {m?.movedAgain && <span className="tag" title={`처음 옮긴 곳: ${m.dstPath}`}>그 뒤 다시 옮겨짐</span>}
                  {!r.exists && <span className="tag warn">없음</span>}
                </span>
                <PathText path={parentOf(r.path)} className="sub-path" prefix="지금 위치 " />
                <span className="muted small">
                  {m ? (
                    <>
                      {formatTime(m.movedAt)} · {m.ruleName ?? BY[m.decidedBy] ?? m.decidedBy} ·{" "}
                      {m.undone ? (
                        <PathText path={parentOf(m.dstPath)} inline prefix="옮겼던 곳 " />
                      ) : (
                        <PathText path={parentOf(m.srcPath)} inline prefix="원래 " />
                      )}
                    </>
                  ) : (
                    <>수정 {formatTime(r.mtime ?? 0)} · 검색으로 찾음</>
                  )}
                </span>
              </div>
              <span className="find-actions">
                <button className="primary" onClick={() => void act(r, "open")} disabled={!r.exists}>
                  열기
                </button>
                <button onClick={() => void act(r, "folder")}>폴더 열기</button>
                <button className="link" onClick={() => void act(r, "copy")}>
                  경로 복사
                </button>
                {m && !m.undone && r.exists && (
                  <button className="link" onClick={() => void putBack(m)} title={m.srcPath}>
                    원래 자리로
                  </button>
                )}
              </span>
            </li>
          );
        })}
      </ul>
      {!all && rows.length > shown.length && (
        <button className="link" onClick={() => setAll(true)}>
          {rows.length - shown.length}개 더 보기
        </button>
      )}
    </section>
  );
}
