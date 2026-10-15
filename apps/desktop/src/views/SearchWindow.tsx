import { useEffect, useRef } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import * as ipc from "../ipc/commands";
import { onFocusSearch } from "../ipc/events";
import { useSearch } from "../store/search";
import { Highlight } from "../components/Highlight";
import { formatSize, formatTime } from "../components/format";

const MATCH_LABEL = { name: "이름", content: "본문", both: "이름+본문", filter: "필터" } as const;

/**
 * Global-hotkey search (SPEC §5.6, §9.2). Keyboard only:
 * ↑/↓ select · Enter open · Ctrl+Enter open folder · Esc hide.
 */
export function SearchWindow() {
  const { query, hits, selected, loading, error, setQuery, move, select } = useSearch();
  const input = useRef<HTMLInputElement>(null);
  const list = useRef<HTMLUListElement>(null);

  useEffect(() => {
    const focus = () => {
      input.current?.focus();
      input.current?.select();
    };
    focus();
    const un = onFocusSearch(focus);
    return () => {
      void un.then((f) => f());
    };
  }, []);

  useEffect(() => {
    list.current?.querySelector<HTMLElement>(`[data-i="${selected}"]`)?.scrollIntoView({ block: "nearest" });
  }, [selected]);

  const current = hits[selected];
  const hide = () => void getCurrentWindow().hide();
  const open = async (folder: boolean) => {
    if (!current) return;
    try {
      if (folder) await ipc.revealPath(current.path);
      else await ipc.openPath(current.path);
      hide();
    } catch (e) {
      alert(String(e));
    }
  };

  const onKey = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      move(1);
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      move(-1);
    } else if (e.key === "Enter") {
      e.preventDefault();
      void open(e.ctrlKey || e.metaKey);
    } else if (e.key === "Escape") {
      e.preventDefault();
      if (query) setQuery("");
      else hide();
    }
  };

  return (
    <div className="search" onKeyDown={onKey}>
      <input
        ref={input}
        className="search-input"
        placeholder="검색어 · ext:pdf · in:~/Documents · after:2026-09 · size:>10mb · type:doc"
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        spellCheck={false}
        autoFocus
      />
      <div className="search-body">
        <ul className="results" ref={list}>
          {hits.map((h, i) => (
            <li
              key={h.path}
              data-i={i}
              className={i === selected ? "sel" : ""}
              onMouseEnter={() => select(i)}
              onDoubleClick={() => void open(false)}
            >
              <div className="r-name">
                <Highlight text={h.nameHighlight} />
                {!h.exists && <span className="tag warn">없음</span>}
              </div>
              <div className="r-dir">{h.dir}</div>
            </li>
          ))}
          {!hits.length && query.trim() && !loading && (
            <li className="empty">{error ? `오류: ${error}` : "결과 없음"}</li>
          )}
        </ul>
        <aside className="preview">
          {current ? (
            <>
              <h3>
                <Highlight text={current.nameHighlight} />
              </h3>
              <p className="muted">{current.path}</p>
              <p className="muted">
                {formatSize(current.size)} · {formatTime(current.mtime)} · {MATCH_LABEL[current.matchedIn]}
              </p>
              {current.snippet ? (
                <blockquote className="snippet">
                  <Highlight text={current.snippet} />
                </blockquote>
              ) : (
                <p className="muted">
                  {current.textStatus === "ok" ? "본문 일치 없음" : `본문 없음 (${current.textStatus})`}
                </p>
              )}
              <div className="actions">
                <button onClick={() => void open(false)}>열기 ⏎</button>
                <button onClick={() => void open(true)}>폴더 열기 Ctrl+⏎</button>
                <button onClick={() => void navigator.clipboard.writeText(current.path)}>경로 복사</button>
              </div>
            </>
          ) : (
            <p className="muted">↑↓ 선택 · Enter 열기 · Ctrl+Enter 폴더 열기 · Esc 닫기</p>
          )}
        </aside>
      </div>
      <footer className="search-foot muted">
        {loading ? "검색 중…" : hits.length ? `${hits.length}건` : ""}
      </footer>
    </div>
  );
}
