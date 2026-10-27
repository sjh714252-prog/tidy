import { useEffect, useState } from "react";
import * as ipc from "../ipc/commands";
import type { AiProvider, AiSettings, SemanticStatus } from "../ipc/types";
import { explainAiError, useAiSettings, type AiTestResult, type AiTestTarget } from "../store/aiSettings";
import { Help } from "./Help";

/**
 * AI (SPEC §5.8): provider / model of the active profile, key (stored only in
 * the OS keychain — never shown again), auto-move threshold, content sending.
 * Without AI everything keeps working with rules + search.
 */
export function AiSettingsSection() {
  const {
    draft,
    apiKey,
    embedApiKey,
    busy,
    message,
    error,
    load,
    patch,
    setProvider,
    setApiKey,
    setEmbedProvider,
    setEmbedApiKey,
    deleteKey,
    test,
    tests,
    dirty,
  } = useAiSettings();
  const [sem, setSem] = useState<SemanticStatus | null>(null);
  const refreshSem = () => void ipc.semanticStatus().then(setSem, () => setSem(null));

  useEffect(() => {
    void load();
    refreshSem();
  }, [load]);

  if (!draft) return <p className="muted small">{error ?? "AI 설정 불러오는 중…"}</p>;
  const unsaved = dirty();
  const testRow = (which: AiTestTarget, label: string, disabled: boolean, why: string) => (
    <div className="actions ai-test" data-testid={`ai-test-${which}`}>
      <button
        disabled={busy || tests[which].busy || disabled}
        title={disabled ? why : unsaved ? "바꾼 설정을 먼저 저장한 뒤 테스트합니다" : ""}
        onClick={() => void test(which)}
      >
        {unsaved ? `저장 후 ${label}` : label}
      </button>
      {tests[which].busy && <span className="muted small">확인 중…</span>}
      {disabled && <span className="muted small">{why}</span>}
      <TestResult r={tests[which]} />
    </div>
  );
  const PROVIDER_LABEL: Record<string, string> = {
    anthropic: "Anthropic (Claude)",
    openai: "OpenAI 호환",
    ollama: "Ollama (로컬)",
  };
  const savedMark = (p: string, current: string, saved: Record<string, unknown>) =>
    p !== current && saved[p] ? " · 저장된 설정" : "";
  const needsKey = draft.provider === "anthropic" || (draft.provider === "openai" && !!draft.keyRef);

  return (
    <div className="ai-section" data-testid="ai-section">
      <p className="muted small">
        규칙에 맞지 않는 파일만 AI에 물어봅니다. 확신도가 기준 이상이고 AI 자동 이동을 허용한 규칙의 폴더일 때만
        바로 옮기고, 나머지는 확인 팝업으로 묻습니다. 업무 맥락 정리와 "AI로 규칙 만들기"도 이 연결을 씁니다. AI가 없거나 실패해도 규칙과 검색은 그대로 동작합니다.
      </p>
      <label className="row">
        <input type="checkbox" checked={draft.enabled} onChange={(e) => patch({ enabled: e.target.checked })} />
        <span>AI 사용 (분류·업무 맥락 정리·규칙 초안)</span>
      </label>
      <label className="row">
        <span title="회사용·집용처럼 AI 설정을 여러 벌 두고 고를 수 있습니다">프로필</span>
        <input
          list="ai-profiles"
          value={draft.activeProfile}
          onChange={(e) => patch({ activeProfile: e.target.value })}
        />
        <datalist id="ai-profiles">
          {draft.profiles.map((p) => (
            <option key={p} value={p} />
          ))}
        </datalist>
      </label>
      <label className="row">
        <span>제공자</span>
        <select value={draft.provider} onChange={(e) => setProvider(e.target.value as AiProvider)}>
          <option value="">(없음)</option>
          {(["anthropic", "openai", "ollama"] as const).map((p) => (
            <option key={p} value={p}>
              {PROVIDER_LABEL[p]}
              {savedMark(p, draft.provider, draft.saved)}
            </option>
          ))}
        </select>
      </label>
      <p className="muted small">제공자마다 설정(모델·키 이름·주소)을 따로 기억해, 바꿨다가 돌아오면 그대로 복원됩니다.</p>
      {draft.provider && (
        <>
          <label className="row">
            <span>모델</span>
            <input className="wide" value={draft.model} onChange={(e) => patch({ model: e.target.value })} />
          </label>
          {draft.provider !== "anthropic" && (
            <label className="row">
              <span>주소</span>
              <input className="wide" value={draft.baseUrl} onChange={(e) => patch({ baseUrl: e.target.value })} />
            </label>
          )}
          {draft.provider !== "ollama" && (
            <>
              <label className="row">
                <span>
                  키 이름 <Help id="keychain" />
                </span>
                <input
                  value={draft.keyRef}
                  placeholder="tidy/anthropic"
                  onChange={(e) => patch({ keyRef: e.target.value })}
                />
                <span className={draft.keyStored ? "ok small" : "warn small"}>
                  {draft.keyStored ? "키체인에 저장됨" : needsKey ? "키 없음" : ""}
                </span>
                {draft.keyStored && (
                  <button className="link" onClick={() => void deleteKey()}>
                    키 삭제
                  </button>
                )}
              </label>
              <label className="row">
                <span>API 키</span>
                <input
                  className="wide"
                  type="password"
                  autoComplete="off"
                  placeholder={draft.keyStored ? "바꿀 때만 입력" : "저장하면 OS 키체인에만 보관됩니다"}
                  value={apiKey}
                  onChange={(e) => setApiKey(e.target.value)}
                />
              </label>
            </>
          )}
          {draft.provider === "anthropic" && (
            <label className="row">
              <span>생각 깊이</span>
              <select value={draft.effort} onChange={(e) => patch({ effort: e.target.value })}>
                <option value="">(모델 기본값)</option>
                <option value="low">낮음 — 분류에 충분, 빠르고 저렴</option>
                <option value="medium">보통</option>
                <option value="high">높음</option>
              </select>
            </label>
          )}
        </>
      )}
      <label className="row">
        <span>
          자동 이동 기준 <Help id="confidence" />
        </span>
        <input
          type="number"
          min={0.5}
          max={1}
          step={0.05}
          value={draft.autoThreshold}
          onChange={(e) => patch({ autoThreshold: Number(e.target.value) })}
        />
        <span className="muted small">확신도가 이 값 이상일 때만 묻지 않고 이동</span>
      </label>
      <label className="row">
        <input type="checkbox" checked={draft.sendContent} onChange={(e) => patch({ sendContent: e.target.checked })} />
        <span>본문 앞부분(기본 1,500자)도 보내기 — 끄면 파일명·크기·날짜·폴더만 보냅니다</span>
        <Help id="sendContent" />
      </label>
      {testRow(
        "classify",
        "AI 연결 테스트",
        !draft.enabled || !draft.provider,
        !draft.enabled ? "AI 사용을 켜야 테스트할 수 있습니다" : "제공자를 고르세요",
      )}
      <h3>
        의미검색 모델 <Help id="embedding" />
      </h3>
      <p className="muted small">
        검색창에서 "작년에 받은 통신 요금 고지서"처럼 자연어로 찾을 수 있게 파일 이름과 본문 앞부분을 벡터로 바꿔
        둡니다. Anthropic에는 임베딩 API가 없어 OpenAI 호환 또는 Ollama(로컬)를 씁니다. 모델이 없거나 실패하면 단어
        검색만으로 동작합니다.
      </p>
      <label className="row">
        <span>제공자</span>
        <select
          value={draft.embedProvider}
          onChange={(e) => setEmbedProvider(e.target.value as AiSettings["embedProvider"])}
        >
          <option value="">(사용 안 함)</option>
          {(["openai", "ollama"] as const).map((p) => (
            <option key={p} value={p}>
              {PROVIDER_LABEL[p]}
              {savedMark(p, draft.embedProvider, draft.embedSaved)}
            </option>
          ))}
        </select>
        {sem && (
          <span className={sem.state === "ready" ? "ok small" : sem.state === "unavailable" ? "warn small" : "muted small"}>
            {sem.state === "ready"
              ? `켜짐 · ${sem.embedded}/${sem.files}개 파일 임베딩됨`
              : sem.state === "unavailable"
                ? `일시 중단: ${sem.detail ?? ""}`
                : `꺼짐${sem.detail ? ` — ${sem.detail}` : ""}`}
          </span>
        )}
      </label>
      {draft.embedProvider && (
        <>
          <label className="row">
            <span>모델</span>
            <input className="wide" value={draft.embedModel} onChange={(e) => patch({ embedModel: e.target.value })} />
          </label>
          <label className="row">
            <span>주소</span>
            <input
              className="wide"
              value={draft.embedBaseUrl}
              onChange={(e) => patch({ embedBaseUrl: e.target.value })}
            />
          </label>
          {draft.embedProvider === "openai" && (
            <>
              <label className="row">
                <span>키 이름</span>
                <input
                  value={draft.embedKeyRef}
                  placeholder="tidy/openai (로컬 서버면 비워 두기)"
                  onChange={(e) => patch({ embedKeyRef: e.target.value })}
                />
                <span className={draft.embedKeyStored ? "ok small" : "muted small"}>
                  {draft.embedKeyStored ? "키체인에 저장됨" : draft.embedKeyRef ? "키 없음" : ""}
                </span>
                {draft.embedKeyStored && draft.embedKeyRef !== draft.keyRef && (
                  <button className="link" onClick={() => void deleteKey("embed")}>
                    키 삭제
                  </button>
                )}
              </label>
              {draft.embedKeyRef && draft.embedKeyRef !== draft.keyRef && (
                <label className="row">
                  <span>API 키</span>
                  <input
                    className="wide"
                    type="password"
                    autoComplete="off"
                    placeholder={draft.embedKeyStored ? "바꿀 때만 입력" : "저장하면 OS 키체인에만 보관됩니다"}
                    value={embedApiKey}
                    onChange={(e) => setEmbedApiKey(e.target.value)}
                  />
                </label>
              )}
            </>
          )}
        </>
      )}
      {draft.embedProvider &&
        testRow(
          "embed",
          "의미검색 연결 테스트",
          !draft.enabled,
          "AI 사용을 켜야 의미검색 모델도 동작합니다",
        )}
      <p className="muted small">키 저장 위치: {draft.keyStore}</p>
      <div className="actions">
        <span className="muted small">아래 "저장"을 누르면 AI 설정도 함께 저장됩니다.</span>
        {busy && <span className="muted small">처리 중…</span>}
      </div>
      {message && <p className="ok small">{message}</p>}
      {error && <p className="error small">{error}</p>}
    </div>
  );
}

/** One section's test outcome: what to do (if known) plus the raw message. */
function TestResult({ r }: { r: AiTestResult }) {
  if (r.busy || r.text === undefined) return null;
  if (r.ok) return <p className="ok small ai-test-result">✓ {r.text}</p>;
  const hint = explainAiError(r.text);
  return (
    <div className="ai-test-result">
      <p className="error small">✗ 연결 실패{hint ? ` — ${hint}` : ""}</p>
      <p className="muted small">{r.text}</p>
    </div>
  );
}
