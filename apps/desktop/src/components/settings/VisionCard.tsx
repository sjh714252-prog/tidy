import { useEffect, useState } from "react";
import * as ipc from "../../ipc/commands";
import type { VisionStatus } from "../../ipc/types";

/**
 * (M16, SPEC §14.13) "사진 내용 인식": the AI looks at pictures and adds a
 * short description, so they are found and sorted by what they show. Off
 * by default (pictures leave the PC unless the model is local). The switch
 * is saved right away; model and daily limit with their own button.
 * Refreshes every few seconds.
 */
export function VisionCard() {
  const [st, setSt] = useState<VisionStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [model, setModel] = useState("");
  const [limit, setLimit] = useState("");
  const [dirty, setDirty] = useState(false);

  useEffect(() => {
    let alive = true;
    const load = () =>
      ipc
        .visionStatus()
        .then((s) => {
          if (!alive) return;
          setSt(s);
          setError(null);
        })
        .catch((e) => alive && setError(String(e)));
    void load();
    const t = window.setInterval(load, 5000);
    return () => {
      alive = false;
      window.clearInterval(t);
    };
  }, []);

  // Fill the fields from the saved settings until the user edits them.
  useEffect(() => {
    if (st && !dirty) {
      setModel(st.modelOverride);
      setLimit(String(st.dailyLimit));
    }
  }, [st, dirty]);

  const run = async (f: () => Promise<VisionStatus>) => {
    setBusy(true);
    try {
      setSt(await f());
      setError(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const limitNum = Number.parseInt(limit, 10);
  const limitOk = Number.isFinite(limitNum) && limitNum >= 1;
  const provider = st?.model?.split(" · ")[0] ?? null;

  return (
    <div className="card" data-testid="vision-card">
      <h2>사진 내용 인식</h2>
      <label className="check">
        <input
          type="checkbox"
          data-testid="vision-enabled"
          disabled={!st || busy}
          checked={st?.enabled ?? false}
          onChange={(e) => {
            const on = e.target.checked;
            setSt((cur) => (cur ? { ...cur, enabled: on } : cur));
            void run(() => ipc.setVisionSettings(on));
          }}
        />
        AI가 사진을 보고 무엇이 찍혔는지 설명을 붙여 두기 — "영수증", "화이트보드"처럼 내용으로 찾고, 새 사진도 내용을 보고 정리합니다
      </label>
      {st && (
        <p className="muted small" style={{ margin: 0 }} data-testid="vision-privacy">
          {st.local
            ? "사진은 이 PC의 로컬 AI(Ollama)로만 보냅니다."
            : `사진을 줄여서 AI 회사${provider ? `(${provider})` : ""}로 보냅니다. 개인정보가 보이는 사진(글자 인식으로 먼저 확인)과 보안 폴더 파일은 보내지 않습니다.`}
        </p>
      )}
      {st?.enabled && (
        <p className="small" style={{ margin: 0 }} data-testid="vision-state">
          {st.model ? `사진을 보는 모델: ${st.model}` : "AI 모델이 설정되지 않았습니다"}
          {" · "}
          {st.pending > 0 ? `남은 사진 ${st.pending}장 · ` : ""}
          설명 붙임 {st.ok}장
          {st.empty > 0 ? ` · 설명할 것 없음 ${st.empty}장` : ""}
          {st.failed > 0 ? ` · 실패 ${st.failed}장` : ""}
          {st.skipped > 0 ? ` · 개인정보 의심이라 안 보냄 ${st.skipped}장` : ""}
          {" · "}
          오늘 {st.today}/{st.dailyLimit}장
          {st.total > 0 && !st.local ? ` · 지금까지 약 $${st.costUsd.toFixed(2)}` : ""}
        </p>
      )}
      {st?.note && (
        <p className="warn small" style={{ margin: 0 }} data-testid="vision-note">
          {st.note}
        </p>
      )}
      {st?.enabled && (
        <div className="row" style={{ gap: 8, flexWrap: "wrap", alignItems: "flex-end" }}>
          <label className="small" style={{ display: "grid", gap: 4 }}>
            사진용 모델 (비우면 분류 모델 그대로)
            <input
              data-testid="vision-model"
              value={model}
              placeholder="예: qwen2.5vl:7b"
              onChange={(e) => {
                setModel(e.target.value);
                setDirty(true);
              }}
            />
          </label>
          <label className="small" style={{ display: "grid", gap: 4 }}>
            하루 최대 (장)
            <input
              data-testid="vision-limit"
              type="number"
              min={1}
              style={{ width: 90 }}
              value={limit}
              onChange={(e) => {
                setLimit(e.target.value);
                setDirty(true);
              }}
            />
          </label>
          <button
            data-testid="vision-save"
            disabled={busy || !dirty || !limitOk}
            onClick={() =>
              void run(async () => {
                const s = await ipc.setVisionSettings(true, model.trim(), limitNum);
                setDirty(false);
                return s;
              })
            }
          >
            저장
          </button>
          {st.failed > 0 && (
            <button disabled={busy} data-testid="vision-retry" onClick={() => void run(ipc.visionRetry)}>
              실패한 {st.failed}장 다시 시도
            </button>
          )}
        </div>
      )}
      {st?.enabled && (
        <p className="muted small" style={{ margin: 0 }}>
          Ollama는 사진을 볼 수 있는 모델(예: qwen2.5vl, gemma3)이 필요합니다. 이미 있던 사진은 하루 최대 장수만큼 최근 것부터 처리합니다.
        </p>
      )}
      {error && <p className="warn small">{error}</p>}
    </div>
  );
}
