import { useEffect } from "react";
import { useLearn } from "../store/learn";

/** "이 규칙을 자동화할까요?" — shown above the queue when the learner has ideas. */
export function LearnPanel({ refreshKey }: { refreshKey: number }) {
  const { items, message, error, refresh, accept, dismiss } = useLearn();
  useEffect(() => {
    void refresh();
  }, [refresh, refreshKey]);

  if (!items.length && !message && !error) return null;
  return (
    <section className="learn">
      {items.map((s) => (
        <div key={s.key} className="learn-card">
          <div>
            <strong>이 규칙을 자동화할까요?</strong> <span className="muted">같은 결정 {s.count}회</span>
          </div>
          <div>
            {s.ruleName} <span className="muted">→ {s.dest}</span>
          </div>
          <div className="muted small">예: {s.examples.join(", ")}</div>
          <details>
            <summary className="small">추가될 규칙 보기</summary>
            <pre className="toml">{s.ruleToml}</pre>
          </details>
          <div className="actions">
            <button className="primary" onClick={() => void accept(s)}>
              규칙 추가
            </button>
            <button onClick={() => void dismiss(s)}>다시 묻지 않기</button>
          </div>
        </div>
      ))}
      {message && <div className="notice">{message}</div>}
      {error && <div className="error pad">{error}</div>}
    </section>
  );
}
