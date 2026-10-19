import { Fragment, useEffect } from "react";
import * as ipc from "../ipc/commands";
import { useSettings } from "../store/settings";
import { useStatus } from "../store/status";
import { AiSettingsSection } from "../components/AiSettingsSection";
import { OrganizeModeField, anchorOf } from "../components/OrganizeMode";
import { RuleCards } from "../components/RuleCards";

/**
 * Settings (SPEC §9.7, §14.1): watch folders with their organize mode →
 * rule cards (TOML under "고급", kept in sync both ways) → AI → DB → misc.
 */
export function SettingsView() {
  const s = useSettings();
  const load = useSettings((st) => st.load);
  const refreshStatus = useStatus((st) => st.refresh);
  const { draft, rulesCheck, editor, cards } = s;
  const aiState = useStatus((st) => st.status?.aiState);

  useEffect(() => {
    void load();
  }, [load]);

  if (!draft) return <p className="muted pad">{s.error ?? "불러오는 중…"}</p>;

  const addFolder = async () => {
    const dir = await ipc.pickFolder();
    if (dir) await s.addWatch(dir);
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
              <th>정리 방식</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {draft.watches.map((w, i) => (
              <Fragment key={i}>
                <tr>
                  <td>
                    <input className="wide" value={w.path} onChange={(e) => s.setWatch(i, { path: e.target.value })} />
                  </td>
                  <td>
                    <input
                      type="checkbox"
                      checked={w.recursive}
                      onChange={(e) => s.setWatch(i, { recursive: e.target.checked })}
                    />
                  </td>
                  <td>
                    <input
                      type="checkbox"
                      checked={w.indexContent}
                      onChange={(e) => s.setWatch(i, { indexContent: e.target.checked })}
                    />
                  </td>
                  <td>
                    <select
                      value={w.kind}
                      onChange={(e) => {
                        const kind = e.target.value as "local" | "network";
                        // NAS default: metadata only (SPEC §5.9).
                        s.setWatch(i, { kind, indexContent: kind === "local" });
                      }}
                    >
                      <option value="local">로컬</option>
                      <option value="network">네트워크(NAS)</option>
                    </select>
                  </td>
                  <td>
                    <OrganizeModeField w={w} onChange={(p) => s.setWatch(i, p)} />
                  </td>
                  <td>
                    <button className="link" onClick={() => s.removeWatch(i)}>
                      삭제
                    </button>
                  </td>
                </tr>
                {w.kind === "network" && (
                  <tr className="subrow">
                    <td colSpan={6}>
                      <label>
                        폴링 간격(초){" "}
                        <input
                          type="number"
                          min={5}
                          className="narrow"
                          placeholder="120"
                          value={w.pollIntervalS ?? ""}
                          onChange={(e) =>
                            s.setWatch(i, { pollIntervalS: e.target.value ? Math.max(5, Number(e.target.value)) : null })
                          }
                        />
                      </label>
                      <label title="변경 알림을 보내지 않는 NAS(SMB/NFS)에서 켜세요">
                        <input
                          type="checkbox"
                          checked={w.pollOnly}
                          onChange={(e) => s.setWatch(i, { pollOnly: e.target.checked })}
                        />{" "}
                        폴링만 사용
                      </label>
                      <label title="본문 인덱싱을 켰을 때, 백그라운드 인덱싱이 본문을 읽는 시간대. 비우면 항상">
                        본문 인덱싱 시간대{" "}
                        <input
                          className="narrow"
                          placeholder="02:00-05:00"
                          disabled={!w.indexContent}
                          value={w.contentIndexWindow ?? ""}
                          onChange={(e) => s.setWatch(i, { contentIndexWindow: e.target.value || null })}
                        />
                      </label>
                      <span className="muted small">
                        연결이 끊기면 이 폴더만 일시정지하고 60초마다 다시 확인합니다. 이동은 복사→검증 후 원본 정리.
                      </span>
                    </td>
                  </tr>
                )}
              </Fragment>
            ))}
          </tbody>
        </table>
        <button onClick={() => void addFolder()}>폴더 추가…</button>
      </section>

      <section className="rules-section">
        <h2>규칙</h2>
        <p className="muted small">
          위에서부터 검사하고 처음 맞는 규칙 하나만 적용합니다. 목적지가 상대 경로면 각 감시 폴더의 정리 방식을
          따릅니다(이 폴더 안 / 기준 폴더 아래). 저장하기 전에는 아무것도 바뀌지 않습니다.
          {editor?.preview &&
            ` 지금 감시 폴더의 파일 ${editor.preview.scanned}개 중 ${editor.preview.scanned - editor.preview.unmatched}개가 규칙에 걸립니다.`}
          {editor?.preview?.skipped.length ? ` (네트워크 폴더는 세지 않음: ${editor.preview.skipped.join(", ")})` : ""}
        </p>
        {cards && (
          <RuleCards
            cards={cards}
            preview={editor?.preview ?? null}
            anchors={draft.watches.map(anchorOf)}
            readOnly={!!editor && !editor.cards}
            aiMarked={s.aiMarked}
            onChange={s.setCards}
          />
        )}
        {cards && !cards.length && <p className="muted">아직 규칙이 없습니다. 규칙을 추가하거나 마법사로 시작 규칙을 고르세요.</p>}
        <div className="actions">
          <button onClick={() => s.addCard()} disabled={!!editor && !editor.cards}>
            + 규칙 추가
          </button>
          <button
            disabled={aiState !== "ready" || s.drafting || (!!editor && !editor.cards)}
            title={aiState === "ready" ? "감시 폴더의 파일 목록(이름·크기·날짜)을 보내 규칙 초안을 받습니다" : "AI를 켜면 쓸 수 있습니다"}
            onClick={() => void s.aiDraft()}
          >
            {s.drafting ? "AI가 규칙을 만드는 중…" : "AI로 규칙 만들기"}
          </button>
        </div>
        {s.draftNote && <p className="muted small">{s.draftNote}</p>}
        {rulesCheck &&
          (rulesCheck.ok ? (
            <p className="ok small">✓ 규칙 {rulesCheck.rules.length}개: {rulesCheck.rules.join(", ")}</p>
          ) : (
            <p className="error small">
              ✗ {rulesCheck.error}
              {editor && !editor.cards && " — 고칠 때까지 카드는 읽기 전용입니다"}
            </p>
          ))}
        <details className="advanced">
          <summary>고급: rules.toml 직접 편집</summary>
          <p className="muted small">
            {draft.rulesPath}
            {!draft.rulesExists && " — 아직 없음: 저장하면 만들어집니다."} 여기서 고치면 카드에 바로 반영되고, 카드를
            고치면 여기도 바뀝니다(주석·순서·카드에 없는 키는 그대로).
          </p>
          <textarea
            className="rules"
            spellCheck={false}
            value={draft.rulesText}
            onChange={(e) => s.setRules(e.target.value)}
          />
        </details>
      </section>

      <AiSettingsSection />

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
        <label className="row">
          <input
            type="checkbox"
            checked={draft.autostart.wanted}
            onChange={(e) => s.patch({ autostart: { ...draft.autostart, wanted: e.target.checked } })}
          />
          <span>로그인 시 자동 시작 — 창 없이 트레이에서 감시</span>
          <span className="muted small" data-testid="autostart-state">
            {draft.autostart.registered == null
              ? "(개발 빌드에서는 OS에 등록하지 않음)"
              : draft.autostart.registered
                ? "OS 시작 항목에 등록됨"
                : "등록 안 됨"}
          </span>
        </label>
        <p className="muted small">설정 파일: {draft.configPath}</p>
        <button onClick={() => void ipc.openWizard()}>마법사 다시 실행</button>
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
