import { useEffect } from "react";
import type { AiProvider } from "../ipc/types";
import { useAiSettings } from "../store/aiSettings";
import { useStatus } from "../store/status";

/**
 * AI (SPEC §5.8): provider / model of the active profile, key (stored only in
 * the OS keychain — never shown again), auto-move threshold, content sending.
 * Without AI everything keeps working with rules + search.
 */
export function AiSettingsSection() {
  const { draft, apiKey, busy, message, error, load, patch, setProvider, setApiKey, save, deleteKey, test } =
    useAiSettings();
  const refreshStatus = useStatus((s) => s.refresh);

  useEffect(() => {
    void load();
  }, [load]);

  if (!draft) return <p className="muted small">{error ?? "AI 설정 불러오는 중…"}</p>;
  const needsKey = draft.provider === "anthropic" || (draft.provider === "openai" && !!draft.keyRef);

  return (
    <section>
      <h2>AI 분류 (선택)</h2>
      <p className="muted small">
        규칙에 맞지 않는 파일만 AI에 물어봅니다. 확신도가 기준 이상이고 <code>ai_auto = true</code> 규칙의 폴더일 때만
        바로 옮기고, 나머지는 확인 팝업으로 묻습니다. AI가 없거나 실패해도 규칙과 검색은 그대로 동작합니다.
      </p>
      <label className="row">
        <input type="checkbox" checked={draft.enabled} onChange={(e) => patch({ enabled: e.target.checked })} />
        <span>AI 분류 사용</span>
      </label>
      <label className="row">
        <span>프로필</span>
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
                <span>키 이름</span>
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
              <span>effort</span>
              <select value={draft.effort} onChange={(e) => patch({ effort: e.target.value })}>
                <option value="">(모델 기본값)</option>
                <option value="low">low — 분류에 충분, 빠르고 저렴</option>
                <option value="medium">medium</option>
                <option value="high">high</option>
              </select>
            </label>
          )}
        </>
      )}
      <label className="row">
        <span>자동 이동 기준</span>
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
      </label>
      <div className="actions">
        <button
          className="primary"
          disabled={busy}
          onClick={() =>
            void save().then((ok) => {
              if (ok) void refreshStatus();
            })
          }
        >
          AI 설정 저장
        </button>
        <button disabled={busy || !draft.enabled || !draft.provider} onClick={() => void test()}>
          연결 테스트
        </button>
        {busy && <span className="muted small">처리 중…</span>}
      </div>
      {message && <p className="ok small">{message}</p>}
      {error && <p className="error small">{error}</p>}
    </section>
  );
}
