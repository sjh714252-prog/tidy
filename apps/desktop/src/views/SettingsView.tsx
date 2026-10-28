import { useEffect, useState } from "react";
import * as ipc from "../ipc/commands";
import type { SettingsKey } from "../ipc/types";
import { useSettings } from "../store/settings";
import { useStatus } from "../store/status";
import { AiSettingsSection } from "../components/AiSettingsSection";
import { useAiSettings } from "../store/aiSettings";
import { useSecurity } from "../store/security";
import { SecuritySection } from "../components/SecuritySection";
import { anchorOf } from "../components/OrganizeMode";
import { RuleCards } from "../components/RuleCards";
import { Help } from "../components/Help";
import { AiNeeded, useAiOn } from "../components/AiNeeded";
import { ContextSection } from "../components/ContextSection";
import { useCurate } from "../store/curate";
import { useHome } from "../store/home";
import { useNav } from "../store/nav";
import { isAbsoluteDest } from "../components/destPaths";
import { WatchSettings, commonRoot } from "../components/settings/WatchSettings";

const MENU: { key: SettingsKey; label: string; ai?: boolean }[] = [
  { key: "watch", label: "감시 폴더" },
  { key: "rules", label: "규칙" },
  { key: "context", label: "업무 맥락", ai: true },
  { key: "ai", label: "AI 연결" },
  { key: "security", label: "보안 · 개인정보" },
  { key: "advanced", label: "고급" },
];

/**
 * Settings (SPEC §14.8 5): a menu on the left, one topic on the right
 * (`#/main/settings/<key>`). One save bar for everything (#8): edits stay
 * while moving between topics.
 */
export function SettingsView() {
  const s = useSettings();
  const load = useSettings((st) => st.load);
  const refreshStatus = useStatus((st) => st.refresh);
  const ai = useAiSettings();
  const sec = useSecurity();
  const cur = useCurate();
  const section = useNav((n) => n.section);
  const loadOverview = useHome((h) => h.load);
  const [version, setVersion] = useState<string>("");
  const openAbout = useHome((h) => h.openAbout);
  const { draft, rulesCheck, cards } = s;

  useEffect(() => {
    void load();
    void loadOverview();
    void ipc
      .aboutInfo()
      .then((a) => setVersion(a.version))
      .catch(() => {});
  }, [load, loadOverview]);

  if (!draft) return <p className="muted pad">{s.error ?? "불러오는 중…"}</p>;

  // One save for everything (issue #8): general settings first, then the AI,
  // security and work-context pages if they have unsaved edits. Each is
  // independent; a failure in one is shown in the save bar.
  const save = async () => {
    await s.save();
    if (ai.dirty()) await ai.save();
    if (sec.dirty()) await sec.save();
    if (cur.dirty()) await cur.save();
    await refreshStatus();
    void loadOverview(true);
  };
  const anyDirty = () => s.dirty() || ai.dirty() || sec.dirty() || cur.dirty();
  const revert = async () => {
    if (anyDirty() && !window.confirm("저장하지 않은 변경을 버릴까요?")) return;
    await s.load();
    await ai.load(true);
    await sec.load(true);
    await cur.load(true);
  };
  const aiError = ai.error;
  const secBlock = sec.dirty() ? sec.invalid() : null;
  const collectRoots = [
    ...new Set(draft.watches.filter((w) => w.mode === "collect" && w.organizeRoot).map((w) => w.organizeRoot as string)),
  ];
  const absRules = collectRoots.length ? (cards ?? []).filter((c) => isAbsoluteDest(c.dest)).length : 0;
  const busy = s.saving || ai.busy || sec.busy || cur.busy;

  return (
    <div className="settings">
      <div className="settings-body">
        <nav className="settings-nav" aria-label="설정 메뉴">
          {MENU.map((m) => (
            <a
              key={m.key}
              href={`#/main/settings/${m.key}`}
              className={section === m.key ? "on" : undefined}
              aria-current={section === m.key ? "page" : undefined}
              data-testid={`settings-nav-${m.key}`}
            >
              {m.label}
              {m.ai && <span className="ai-badge">AI</span>}
            </a>
          ))}
          <span className="nav-foot">
            Tidy {version} ·{" "}
            <button className="link small" onClick={() => openAbout(false)}>
              정보
            </button>
          </span>
        </nav>
        <div className="settings-main">
          <section className="settings-page" data-section={section} data-testid={`settings-${section}`}>
            {section === "watch" && <WatchSettings />}
            {section === "rules" && <RulesPage collectRoots={collectRoots} />}
            {section === "context" && (
              <>
                <div className="page-head">
                  <div>
                    <h1>
                      업무 맥락 <AiNeeded always />
                    </h1>
                    <p className="lead">고객사·이슈·문서 종류 사전과 업무 맥락 정리 방식입니다. AI가 있어야 동작합니다.</p>
                  </div>
                </div>
                <div className="card">
                  <ContextSection />
                </div>
              </>
            )}
            {section === "ai" && (
              <>
                <div>
                  <h1>AI 연결</h1>
                  <p className="lead">AI가 없어도 규칙과 검색은 그대로 동작합니다. 키는 OS 키체인에만 저장합니다.</p>
                </div>
                <div className="card">
                  <AiSettingsSection />
                </div>
              </>
            )}
            {section === "security" && (
              <>
                <div>
                  <h1>보안 · 개인정보</h1>
                  <p className="lead">개인정보가 든 파일을 찾아 암호화 zip으로 보관하는 방식을 정합니다.</p>
                </div>
                <div className="card">
                  <SecuritySection />
                </div>
              </>
            )}
            {section === "advanced" && <AdvancedPage />}
          </section>
        </div>
      </div>

      <footer className="savebar" data-testid="savebar">
        {s.error && <span className="error">{s.error}</span>}
        {!s.error && aiError && <span className="error">AI: {aiError}</span>}
        {!s.error && !aiError && sec.error && <span className="error">보안: {sec.error}</span>}
        {!s.error && !aiError && !sec.error && cur.error && <span className="error">업무 맥락: {cur.error}</span>}
        {!s.error && !aiError && !sec.error && secBlock && <span className="warn small">보안: {secBlock}</span>}
        {s.message && !s.error && !anyDirty() && (
          <span className="ok" data-testid="save-message">
            {s.message}
          </span>
        )}
        {!s.error && anyDirty() && <span className="warn">저장하지 않은 변경이 있습니다</span>}
        {!s.error && !anyDirty() && !s.message && <span className="muted">저장하지 않은 변경 없음</span>}
        {absRules > 0 && (
          <span className="warn small" data-testid="abs-rules-warning">
            규칙 {absRules}개가 정리 위치를 무시합니다
          </span>
        )}
        <div className="spacer" />
        <button onClick={() => void revert()} disabled={busy}>
          변경 취소
        </button>
        <button
          className="primary"
          onClick={() => void save()}
          disabled={busy || !!secBlock || (rulesCheck ? !rulesCheck.ok : false)}
        >
          {busy ? "저장 중…" : "저장"}
        </button>
      </footer>
    </div>
  );
}

/** 설정 → 규칙 (Rules.html). */
function RulesPage({ collectRoots }: { collectRoots: string[] }) {
  const s = useSettings();
  const aiOn = useAiOn();
  const { draft, rulesCheck, editor, cards } = s;
  if (!draft) return null;
  const common = commonRoot(draft.watches);
  const readOnly = !!editor && !editor.cards;
  return (
    <>
      <div className="page-head">
        <div>
          <h1>규칙</h1>
          <p className="lead">
            <Help id="ruleOrder" /> 위에서부터 검사해 처음 맞는 규칙 하나를 적용합니다.{" "}
            {common ? `목적지는 정리 위치(${common}) 아래 상대 경로입니다.` : "목적지가 상대 경로면 각 감시 폴더 안에 만듭니다."}
            {editor?.preview &&
              ` 지금 감시 폴더의 파일 ${editor.preview.scanned.toLocaleString()}개 중 ${(editor.preview.scanned - editor.preview.unmatched).toLocaleString()}개가 규칙에 걸립니다.`}
            {editor?.preview?.skipped.length ? ` (네트워크 폴더는 세지 않음: ${editor.preview.skipped.join(", ")})` : ""}
          </p>
        </div>
        <button
          disabled={!aiOn || s.drafting || readOnly}
          title={aiOn ? "감시 폴더의 파일 목록(이름·크기·날짜)을 보내 규칙 초안을 받습니다" : "AI를 켜면 쓸 수 있습니다"}
          onClick={() => void s.aiDraft()}
        >
          {s.drafting ? "AI가 규칙을 만드는 중…" : "AI로 규칙 만들기"}
        </button>
        <button className="primary" onClick={() => s.addCard()} disabled={readOnly}>
          + 규칙 추가
        </button>
      </div>
      {!aiOn && (
        <div>
          <AiNeeded />
        </div>
      )}
      {s.draftNote && <p className="notice">{s.draftNote}</p>}
      {cards && (
        <RuleCards
          cards={cards}
          preview={editor?.preview ?? null}
          anchors={draft.watches.map(anchorOf)}
          collectRoots={collectRoots}
          readOnly={readOnly}
          aiMarked={s.aiMarked}
          open={s.openRule}
          onOpen={s.setOpenRule}
          onChange={s.setCards}
        />
      )}
      {cards && !cards.length && <p className="muted">아직 규칙이 없습니다. 규칙을 추가하거나 마법사로 시작 규칙을 고르세요.</p>}
      {rulesCheck &&
        (rulesCheck.ok ? (
          <p className="ok rules-check" data-testid="rules-check">
            ✓ 규칙 {rulesCheck.rules.length}개: {rulesCheck.rules.join(", ")}
          </p>
        ) : (
          <p className="error rules-check" data-testid="rules-check">
            ✗ {rulesCheck.error}
            {readOnly && " — 고칠 때까지 카드는 읽기 전용입니다"}
          </p>
        ))}
      <details className="toml-edit advanced" data-testid="toml-edit">
        <summary>고급: rules.toml 직접 편집</summary>
        <p className="muted small">
          {draft.rulesPath}
          {!draft.rulesExists && " — 아직 없음: 저장하면 만들어집니다."} 여기서 고치면 카드에 바로 반영되고, 카드를 고치면
          여기도 바뀝니다(주석·순서·카드에 없는 키는 그대로).
        </p>
        <textarea
          className="rules"
          aria-label="rules.toml"
          spellCheck={false}
          value={draft.rulesText}
          onChange={(e) => s.setRules(e.target.value)}
        />
      </details>
    </>
  );
}

/** 설정 → 고급: 무시한 파일, DB, 단축키, 조용 모드, 자동 시작, 업데이트 확인, 마법사. */
function AdvancedPage() {
  const s = useSettings();
  const draft = s.draft;
  if (!draft) return null;
  const dbChanged = (s.loaded?.dbPathCustom ?? "") !== (draft.dbPathCustom ?? "");
  return (
    <>
      <div>
        <h1>고급</h1>
        <p className="lead">자주 바꿀 일이 없는 설정입니다.</p>
      </div>
      <div className="card">
        <IgnoredFiles />
      </div>
      <div className="card">
        <h2>데이터베이스</h2>
        <label className="field">
          <span className="label">DB 경로</span>
          <input
            className="wide"
            aria-label="DB 경로"
            placeholder={`기본값: ${draft.dbPathDefault}`}
            value={draft.dbPathCustom ?? ""}
            onChange={(e) => s.patch({ dbPathCustom: e.target.value })}
          />
        </label>
        <p className="muted small" style={{ margin: 0 }}>
          현재: {draft.dbPath}. 로컬 디스크 경로만 사용하세요(네트워크 드라이브의 SQLite는 깨질 수 있음).
          {dbChanged && " 경로를 바꾸면 새 DB를 쓰며 기존 이동 기록·확인 대기·인덱스는 옮겨지지 않습니다."}
        </p>
      </div>
      <div className="card">
        <h2>기타</h2>
        <label className="field">
          <span className="label">검색 단축키</span>
          <input aria-label="검색 단축키" value={draft.hotkey} onChange={(e) => s.patch({ hotkey: e.target.value })} />
        </label>
        <label className="check">
          <input type="checkbox" checked={draft.quietMode} onChange={(e) => s.patch({ quietMode: e.target.checked })} />
          조용 모드 — 확인 팝업을 띄우지 않고 확인 대기 탭에만 쌓기
        </label>
        <label className="check">
          <input
            type="checkbox"
            checked={draft.autostart.wanted}
            onChange={(e) => s.patch({ autostart: { ...draft.autostart, wanted: e.target.checked } })}
          />
          로그인 시 자동 시작 — 창 없이 트레이에서 감시
          <span className="muted small" data-testid="autostart-state">
            {draft.autostart.registered == null
              ? "(개발 빌드에서는 OS에 등록하지 않음)"
              : draft.autostart.registered
                ? "OS 시작 항목에 등록됨"
                : "등록 안 됨"}
          </span>
        </label>
        <label className="check">
          <input
            type="checkbox"
            data-testid="check-updates"
            checked={draft.checkUpdates}
            onChange={(e) => s.patch({ checkUpdates: e.target.checked })}
          />
          새 버전 확인(켤 때마다·하루 한 번) — 새 버전이 있으면 상태줄에 "새 버전" 표시, 누르면 지금 업데이트(실패해도 아무것도 막지 않음)
        </label>
        <label className="check">
          <input
            type="checkbox"
            data-testid="keep-shortcuts"
            checked={draft.keepShortcuts}
            onChange={(e) => s.patch({ keepShortcuts: e.target.checked })}
          />
          바로가기는 그대로 두기 — 바탕화면의 바로가기(.lnk, .url 등)는 정리하지 않고 제자리에 둡니다
        </label>
        <p className="muted small" style={{ margin: 0 }}>
          설정 파일: {draft.configPath}
        </p>
        <div>
          <button onClick={() => void ipc.openWizard()}>마법사 다시 실행</button>
        </div>
      </div>
    </>
  );
}

/** Files marked "무시" in the confirm popup / queue, with a way back. */
function IgnoredFiles() {
  const [files, setFiles] = useState<string[] | null>(null);
  const load = async () => setFiles(await ipc.listIgnored().catch(() => []));
  useEffect(() => {
    void load();
  }, []);
  if (!files) return null;
  return (
    <div className="ignored-files" data-testid="ignored-files">
      <h2>
        무시한 파일 <span className="muted small">({files.length}개)</span>
      </h2>
      <p className="muted small">
        확인 팝업·확인 대기에서 "무시"를 누른 파일입니다. 다시 정리 대상으로 돌리려면 "해제"를 누르세요.
      </p>
      {files.length === 0 ? (
        <p className="muted small">없음</p>
      ) : (
        <ul>
          {files.map((f) => (
            <li key={f}>
              <span title={f}>{f}</span>{" "}
              <button
                className="link"
                onClick={async () => {
                  await ipc.unignore(f);
                  await load();
                }}
              >
                해제
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
