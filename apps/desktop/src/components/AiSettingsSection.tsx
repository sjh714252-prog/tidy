import { useEffect, useState } from "react";
import * as ipc from "../ipc/commands";
import type { AiProvider, AiSettings, SemanticStatus } from "../ipc/types";
import { useAiSettings } from "../store/aiSettings";
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
  } = useAiSettings();
  const [sem, setSem] = useState<SemanticStatus | null>(null);
  const refreshSem = () => void ipc.semanticStatus().then(setSem, () => setSem(null));

  useEffect(() => {
    void load();
    refreshSem();
  }, [load]);

  if (!draft) return <p className="muted small">{error ?? "AI 설정 불러오는 중…"}</p>;
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
          <option value="anthropic">Anthropic (Claude)</option>
          <option value="openai">OpenAI 호환</option>
          <option value="ollama">Ollama (로컬)</option>
        </select>
      </label>
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
          <option value="openai">OpenAI 호환</option>
          <option value="ollama">Ollama (로컬)</option>
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
      <p className="muted small">키 저장 위치: {draft.keyStore}</p>
      <div className="actions">
        <span className="muted small">아래 "저장"을 누르면 AI 설정도 함께 저장됩니다.</span>
        <button disabled={busy || !draft.enabled || !draft.provider} onClick={() => void test()}>
          연결 테스트
        </button>
        {busy && <span className="muted small">처리 중…</span>}
      </div>
      {message && <p className="ok small">{message}</p>}
      {error && <p className="error small">{error}</p>}
    </div>
  );
}
