import { useCommand } from "../store/command";
import { useWatchNames } from "../store/status";
import { formatSize, formatTime } from "./format";
import { PathText } from "./PathText";
import { watchOf } from "./paths";

/** Where a found file is now (its folder, shown with the watch alias). */
function Where({ p }: { p: string }) {
  const names = useWatchNames();
  const dir = p.replace(/[\\/][^\\/]*$/, "") || p;
  const w = watchOf(dir, names);
  return <PathText path={dir} alias={w?.alias} root={w?.path} className="sub-path" />;
}

const name = (p: string) => p.split(/[\\/]/).filter(Boolean).pop() ?? p;

/**
 * 홈 맨 위 "말로 시키기" (SPEC §14.13): 한 줄 명령 → 해석 칩 + 찾은 파일
 * → [N개 옮기기] (누르기 전엔 아무것도 옮기지 않음) → [되돌리기].
 */
export function CommandCard() {
  const c = useCommand();
  const r = c.result;
  const plan = r?.plan ?? null;
  const done = !!c.applied;
  const n = c.selected.size;
  return (
    <section className="card" data-testid="command-card">
      <div className="card-head baseline">
        <h2>말로 시키기</h2>
        <span className="sub">찾거나 옮길 파일을 말로 적으세요. 옮기기 전에 목록을 먼저 보여 드립니다.</span>
      </div>
      <form
        className="toolbar"
        onSubmit={(e) => {
          e.preventDefault();
          void c.run();
        }}
      >
        <input
          type="text"
          data-testid="command-input"
          aria-label="명령"
          style={{ flex: 1, minWidth: 220 }}
          placeholder="예: 지난달 받은 PDF를 프로젝트 폴더로 옮겨줘"
          value={c.text}
          onChange={(e) => c.setText(e.target.value)}
        />
        <button type="submit" className="primary" disabled={c.busy || !c.text.trim()} data-testid="command-run">
          {c.busy && !r ? "해석하는 중…" : "미리보기"}
        </button>
        {r && (
          <button type="button" className="link" onClick={c.reset}>
            닫기
          </button>
        )}
      </form>
      {c.error && (
        <div className="notice warn" data-testid="command-error">
          {c.error}
        </div>
      )}
      {r && (
        <div className="stack" data-testid="command-result">
          <div className="chips" data-testid="command-understood">
            {r.understood.map((u) => (
              <span key={u} className="chip">
                {u}
              </span>
            ))}
            {r.parsed_by === "rules" && (
              <span className="muted small" data-testid="command-offline">
                · AI 없이 해석함
              </span>
            )}
          </div>
          {r.note && <div className="muted small">{r.note}</div>}
          {r.warnings.map((w) => (
            <div key={w} className="notice warn" data-testid="command-warning">
              {w}
            </div>
          ))}
          {r.files.length > 0 && (
            <>
              {plan && !done && (
                <label className="check">
                  <input
                    type="checkbox"
                    aria-label="모두 선택"
                    checked={n === plan.moves.length}
                    onChange={(e) => c.setAll(e.target.checked)}
                  />{" "}
                  모두 <span className="muted">· 선택 {n} / {plan.moves.length}</span>
                </label>
              )}
              <ul className="plain-list" data-testid="command-files" style={{ maxHeight: 320, overflow: "auto" }}>
                {(plan ? plan.moves.map((m) => ({ id: m.id, path: m.from })) : r.files.map((f, i) => ({ id: i + 1, path: f.path }))).map(
                  ({ id, path }, i) => {
                    const f = r.files[i];
                    return (
                      <li key={path} className="toolbar" data-testid="command-file" style={{ flexWrap: "nowrap", minWidth: 0 }}>
                        {plan && (
                          <input
                            type="checkbox"
                            aria-label="선택"
                            disabled={done}
                            checked={c.selected.has(id)}
                            onChange={() => c.toggle(id)}
                          />
                        )}
                        <span className="name nowrap" title={path}>
                          {name(path)}
                        </span>
                        <span style={{ flex: 1, minWidth: 0, display: "flex" }}>
                          <Where p={path} />
                        </span>
                        {f && <span className="muted small nowrap">{formatSize(f.size)}</span>}
                        {f && <span className="muted small nowrap">{formatTime(f.mtime)}</span>}
                      </li>
                    );
                  },
                )}
              </ul>
            </>
          )}
          {plan && !done && (
            <div className="toolbar">
              <button className="primary" disabled={c.busy || !n} onClick={() => void c.apply()} data-testid="command-apply">
                {c.busy ? "옮기는 중…" : `${n}개 옮기기`}
              </button>
              <button onClick={c.reset}>취소</button>
            </div>
          )}
          {!plan && r.intent.action === "find" && r.files.length > 0 && (
            <div className="muted small">찾기만 했습니다. 옮기려면 "…폴더로 옮겨줘"를 붙여 다시 시키세요.</div>
          )}
          {c.applied && (
            <div className="notice" data-testid="command-applied">
              {c.applied.moved}개를 옮겼습니다
              {c.applied.duplicates.length > 0 && ` · 똑같은 파일이 있어 그대로 둠 ${c.applied.duplicates.length}개`}
              {c.applied.errors.length > 0 && ` · 실패 ${c.applied.errors.length}개: ${c.applied.errors.join("; ")}`}
              {c.applied.moved > 0 && !c.undone && (
                <>
                  {" "}
                  <button className="link" disabled={c.busy} onClick={() => void c.undo()} data-testid="command-undo">
                    되돌리기
                  </button>
                </>
              )}
              {c.undone && <div data-testid="command-undone">{c.undone}</div>}
            </div>
          )}
        </div>
      )}
    </section>
  );
}
