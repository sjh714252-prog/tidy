import { useEffect, useRef } from "react";
import { hideCurrentWindow } from "../ipc/transport";
import * as ipc from "../ipc/commands";
import { onFocusSearch } from "../ipc/events";
import { useSearch } from "../store/search";
import { Highlight } from "../components/Highlight";
import { formatSize, formatTime } from "../components/format";

const MATCH_LABEL = { name: "이름", content: "본문", both: "이름+본문", filter: "필터", semantic: "의미" } as const;

const pct = (x: number) => `${Math.round(x * 100)}%`;

/**
 * Global-hotkey search (SPEC §5.6, §9.2). Keyboard only:
 * ↑/↓ select · Enter open · Ctrl+Enter open folder · Esc hide.
 */
export function SearchWindow() {
  const {
    query,
    hits,
    selected,
    loading,
    semanticLoading,
    mode,
    natural,
    note,
    semantic,
    error,
    setQuery,
    move,
    select,
    refreshSemantic,
  } = useSearch();
  const input = useRef<HTMLInputElement>(null);
  const list = useRef<HTMLUListElement>(null);

  useEffect(() => {
    const focus = () => {
      input.current?.focus();
      input.current?.select();
    };
    focus();
    void refreshSemantic();
    const un = onFocusSearch(() => {
      focus();
      void refreshSemantic();
    });
    return () => {
      void un.then((f) => f());
    };
  }, []);

  useEffect(() => {
    list.current?.querySelector<HTMLElement>(`[data-i="${selected}"]`)?.scrollIntoView({ block: "nearest" });
  }, [selected]);

  const current = hits[selected];
  const hide = () => void hideCurrentWindow();
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
        placeholder={
          semantic?.state === "ready"
            ? "검색어 또는 질문 (예: 작년 통신 요금 고지서) · ext:pdf · in:~/Documents · after:2026-09"
            : "검색어 · ext:pdf · in:~/Documents · after:2026-09 · size:>10mb · type:doc"
        }
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
                {h.matchedIn === "semantic" && (
                  <span className="tag sem" title="단어는 없지만 뜻이 가까운 파일">
                    의미 {h.similarity != null ? pct(h.similarity) : ""}
                  </span>
                )}
                {h.vault && (
                  <span className="tag lock" data-testid="lock-badge" title="암호화 보관 파일 — 열면 임시 사본으로 풀어 엽니다">
                    🔒
                  </span>
                )}
                {!h.vault && h.pii && (
                  <span className="tag warn" title={`개인정보: ${h.pii} — 암호화되지 않음`}>
                    개인정보
                  </span>
                )}
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
              {current.vault && (
                <p className="small" data-testid="lock-note">
                  🔒 암호화 보관 파일{current.pii ? ` · 개인정보: ${current.pii}` : ""}. 열기를 누르면 보관 비밀번호로 임시
                  사본을 풀어 열고, 임시 사본은 잠시 뒤 지웁니다.
                </p>
              )}
              {!current.vault && current.pii && <p className="warn small">개인정보: {current.pii} (암호화되지 않음)</p>}
              <p className="muted">
                {formatSize(current.size)} · {formatTime(current.mtime)} · {MATCH_LABEL[current.matchedIn]}
                {current.similarity != null && ` · 유사도 ${pct(current.similarity)}`}
              </p>
              {current.matchedIn === "semantic" && current.snippet && (
                <p className="muted small">단어는 일치하지 않지만 내용의 뜻이 질의와 가깝습니다</p>
              )}
              {current.snippet ? (
                <blockquote className="snippet">
                  <Highlight text={current.snippet} />
                </blockquote>
              ) : (
                <p className="muted">
                  {current.matchedIn === "semantic"
                    ? "단어는 일치하지 않지만 내용의 뜻이 질의와 가깝습니다"
                    : current.textStatus === "ok"
                      ? "본문 일치 없음"
                      : `본문 없음 (${current.textStatus})`}
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
        <span>{loading ? "검색 중…" : hits.length ? `${hits.length}건` : ""}</span>
        <span className="search-mode" data-mode={mode ?? ""}>
          {semanticLoading
            ? "의미 분석 중…"
            : note
              ? note
              : mode === "hybrid"
                ? `단어+의미${natural ? " (질문형)" : ""}`
                : semantic?.state === "ready"
                  ? `의미검색 켜짐 · ${semantic.detail ?? ""} · ${semantic.embedded}/${semantic.files}`
                  : semantic?.state === "unavailable"
                    ? "의미검색 일시 중단 — 단어 검색만"
                    : "단어 검색"}
        </span>
      </footer>
    </div>
  );
}
