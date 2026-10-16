import { useEffect } from "react";
import * as ipc from "../ipc/commands";
import { useSettings } from "../store/settings";
import { useStatus } from "../store/status";

/** Settings (SPEC §9.7, M3 subset): watch folders, rules (raw TOML), DB path, hotkey. */
export function SettingsView() {
  const s = useSettings();
  const load = useSettings((st) => st.load);
  const refreshStatus = useStatus((st) => st.refresh);
  const { draft, rulesCheck } = s;

  useEffect(() => {
    void load();
  }, [load]);

  if (!draft) return <p className="muted pad">{s.error ?? "불러오는 중…"}</p>;

  const addFolder = async () => {
    try {
      const dir = await ipc.pickFolder();
      if (dir) s.addWatch(dir);
    } catch {
      // No native dialog (dev bridge): add an empty row to type the path into.
      s.addWatch("");
    }
  };
  const save = async () => {
    await s.save();
    await refreshStatus();
  };
  const dbChanged = (s.loaded?.dbPathCustom ?? "") !== (draft.dbPathCustom ?? "");

  return (
    <div className="settings">
      <section>
        <h2>감시 폴더</h2>
        <p className="muted small">
          새 파일이 생기면 규칙에 따라 정리합니다. 폴더 자체도 검색 인덱스에 들어갑니다.
        </p>
        <table className="grid">
          <thead>
            <tr>
              <th>경로</th>
              <th>하위 폴더</th>
              <th>본문 인덱싱</th>
              <th>종류</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {draft.watches.map((w, i) => (
              <tr key={i}>
                <td>
                  <input className="wide" value={w.path} onChange={(e) => s.setWatch(i, { path: e.target.value })} />
                </td>
                <td>
                  <input type="checkbox" checked={w.recursive} onChange={(e) => s.setWatch(i, { recursive: e.target.checked })} />
                </td>
                <td>
                  <input
                    type="checkbox"
                    checked={w.indexContent}
                    onChange={(e) => s.setWatch(i, { indexContent: e.target.checked })}
                  />
                </td>
                <td>
                  <select value={w.kind} onChange={(e) => s.setWatch(i, { kind: e.target.value as "local" | "network" })}>
                    <option value="local">로컬</option>
                    <option value="network">네트워크(NAS)</option>
                  </select>
                </td>
                <td>
                  <button className="link" onClick={() => s.removeWatch(i)}>
                    삭제
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
        <button onClick={() => void addFolder()}>폴더 추가…</button>
      </section>

      <section>
        <h2>규칙 (rules.toml)</h2>
        <p className="muted small">
          {draft.rulesPath}
          {!draft.rulesExists && " — 아직 없음: 아래는 예시이며 저장하면 이 내용으로 만들어집니다."}
        </p>
        <textarea
          className="rules"
          spellCheck={false}
          value={draft.rulesText}
          onChange={(e) => s.setRules(e.target.value)}
        />
        {rulesCheck &&
          (rulesCheck.ok ? (
            <p className="ok small">✓ 규칙 {rulesCheck.rules.length}개: {rulesCheck.rules.join(", ")}</p>
          ) : (
            <p className="error small">✗ {rulesCheck.error}</p>
          ))}
      </section>

      <section>
        <h2>데이터베이스</h2>
        <label className="row">
          <span>DB 경로</span>
          <input
            className="wide"
            placeholder={`기본값: ${draft.dbPathDefault}`}
            value={draft.dbPathCustom ?? ""}
            onChange={(e) => s.patch({ dbPathCustom: e.target.value })}
          />
        </label>
        <p className="muted small">
          현재: {draft.dbPath}. 로컬 디스크 경로만 사용하세요(네트워크 드라이브의 SQLite는 깨질 수 있음).
          {dbChanged && " 경로를 바꾸면 새 DB를 쓰며 기존 이동 기록·대기열·인덱스는 옮겨지지 않습니다."}
        </p>
      </section>

      <section>
        <h2>기타</h2>
        <label className="row">
          <span>검색 단축키</span>
          <input value={draft.hotkey} onChange={(e) => s.patch({ hotkey: e.target.value })} />
        </label>
        <label className="row">
          <input type="checkbox" checked={draft.quietMode} onChange={(e) => s.patch({ quietMode: e.target.checked })} />
          <span>조용 모드 — 확인 팝업을 띄우지 않고 대기열에만 쌓기</span>
        </label>
        <p className="muted small">설정 파일: {draft.configPath}</p>
      </section>

      <footer className="savebar">
        {s.error && <span className="error">{s.error}</span>}
        {s.message && <span className="ok">{s.message}</span>}
        <div className="spacer" />
        <button onClick={() => void s.load()} disabled={s.saving}>
          되돌리기
        </button>
        <button className="primary" onClick={() => void save()} disabled={s.saving || (rulesCheck ? !rulesCheck.ok : false)}>
          {s.saving ? "저장 중…" : "저장"}
        </button>
      </footer>
    </div>
  );
}
