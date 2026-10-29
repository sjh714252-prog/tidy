import { useEffect } from "react";
import type { NasState } from "../ipc/types";
import { useNas } from "../store/nas";
import { useNav } from "../store/nav";

/**
 * (M18) 홈 › NAS: one row per connected NAS — connected or not, what waits
 * for an answer there, and how many files it organized today. Hidden when
 * no NAS is connected.
 */
export function NasCard() {
  const { list, loaded, load } = useNas();
  const goSettings = useNav((s) => s.goSettings);
  const setTab = useNav((s) => s.setTab);
  useEffect(() => {
    void load();
    const t = setInterval(() => void load(), 60_000);
    return () => clearInterval(t);
  }, [load]);
  if (!loaded || !list.length) return null;
  return (
    <section className="card" data-testid="nas-card">
      <div className="card-head">
        <h2>NAS</h2>
        <span className="sub">NAS가 직접 감시하고 정리합니다</span>
        <span className="spacer" />
        <button className="link" onClick={() => void load()}>
          새로 고침
        </button>
        <button className="link" onClick={() => goSettings("nas")}>
          NAS 설정
        </button>
      </div>
      <div className="list">
        {list.map((n) => (
          <NasRow key={n.name} n={n} onPending={() => setTab("pending")} onHistory={() => setTab("history")} />
        ))}
      </div>
    </section>
  );
}

function NasRow({ n, onPending, onHistory }: { n: NasState; onPending: () => void; onHistory: () => void }) {
  const st = n.status;
  const offline = st?.watches.filter((w) => !w.online).length ?? 0;
  return (
    <div className="list-row nas-row" data-testid="nas-row" data-name={n.name}>
      <span style={{ fontWeight: 600 }}>
        <span className={`dot ${n.ok ? "ok" : "off"}`} /> {n.name}
      </span>
      {n.ok && st ? (
        <span className="muted">
          {st.watching ? `감시 폴더 ${st.watches.length}개` : st.watches.length ? "감시 멈춤" : "감시 폴더 없음"}
          {offline > 0 && <span className="warn"> · 연결 끊긴 폴더 {offline}</span>}
          {" · "}
          <button className="link small" onClick={onHistory}>
            오늘 {st.todayMoved.toLocaleString()}개 정리
          </button>
          {st.pending > 0 && (
            <>
              {" · "}
              <button className="link small" data-testid="nas-pending-link" onClick={onPending}>
                확인 대기 {st.pending.toLocaleString()}
              </button>
            </>
          )}
          {st.ocrWaiting > 0 && ` · 글자 인식 대기 ${st.ocrWaiting.toLocaleString()}`}
          {st.error && <span className="warn"> · {st.error}</span>}
        </span>
      ) : (
        <span className="warn" title={n.error ?? undefined}>
          연결 안 됨{n.error ? ` — ${n.error}` : ""}
        </span>
      )}
    </div>
  );
}
