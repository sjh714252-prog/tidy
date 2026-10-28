import { useEffect, useState } from "react";
import * as ipc from "../../ipc/commands";
import type { OcrStatus } from "../../ipc/types";

/**
 * (M15, SPEC §14.11) "사진·스캔 PDF 속 글자도 검색": one switch (saved
 * right away), what reads the text on this PC, and how far it got.
 * Refreshes every few seconds while there is something waiting.
 */
export function OcrCard() {
  const [st, setSt] = useState<OcrStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let alive = true;
    const load = () =>
      ipc
        .ocrStatus()
        .then((s) => alive && (setSt(s), setError(null)))
        .catch((e) => alive && setError(String(e)));
    void load();
    const t = window.setInterval(load, 4000);
    return () => {
      alive = false;
      window.clearInterval(t);
    };
  }, []);

  const run = async (f: () => Promise<OcrStatus>) => {
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

  return (
    <div className="card" data-testid="ocr-card">
      <h2>사진·스캔 PDF 속 글자 검색</h2>
      <label className="check">
        <input
          type="checkbox"
          data-testid="ocr-enabled"
          disabled={!st}
          checked={st?.enabled ?? false}
          onChange={(e) => {
            const on = e.target.checked;
            // Show the new state right away; the answer replaces it.
            setSt((cur) => (cur ? { ...cur, enabled: on } : cur));
            void run(() => ipc.setOcrEnabled(on));
          }}
        />
        캡처·사진·스캔한 PDF 안의 글자도 읽어서 검색되게 하기 — 이 PC 안에서만 읽고 밖으로 보내지 않습니다
      </label>
      {st && (
        <p className="small" style={{ margin: 0 }} data-testid="ocr-state">
          {st.engine ? `읽는 도구: ${st.engine}` : "이 PC에서는 글자를 읽을 도구가 없습니다"}
          {st.enabled && (
            <>
              {" · "}
              {st.pending > 0 ? `남은 파일 ${st.pending}개 · ` : ""}
              글자 찾음 {st.ok}개{st.empty > 0 ? ` · 글자 없음 ${st.empty}개` : ""}
              {st.failed > 0 ? ` · 실패 ${st.failed}개` : ""}
            </>
          )}
        </p>
      )}
      {st?.note && (
        <p className="muted small" style={{ margin: 0 }} data-testid="ocr-note">
          {st.note}
        </p>
      )}
      {st && st.failed > 0 && (
        <div>
          <button disabled={busy} data-testid="ocr-retry" onClick={() => void run(ipc.ocrRetry)}>
            실패한 {st.failed}개 다시 시도
          </button>
        </div>
      )}
      {error && <p className="warn small">{error}</p>}
    </div>
  );
}
