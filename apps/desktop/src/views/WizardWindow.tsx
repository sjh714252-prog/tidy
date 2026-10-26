import { useEffect } from "react";
import * as ipc from "../ipc/commands";
import { hideCurrentWindow } from "../ipc/transport";
import { onNavigate } from "../ipc/events";
import { AiSettingsSection } from "../components/AiSettingsSection";
import { OrganizeModeField, anchorOf } from "../components/OrganizeMode";
import { Help } from "../components/Help";
import { useWizard } from "../store/wizard";

const STEPS = ["감시 폴더", "시작 규칙", "AI (선택)", "마무리"];

const join = (base: string, rel: string) =>
  rel.startsWith("{root}") ? base + rel.slice("{root}".length) : `${base.replace(/[\\/]+$/, "")}/${rel}`;

/**
 * First-run wizard (SPEC §14.1): folders + organize mode → starter rules →
 * AI (optional) → summary and "감시 시작". "나중에" on every step. Nothing
 * is written or moved before "감시 시작".
 */
export function WizardWindow() {
  const w = useWizard();
  const load = useWizard((s) => s.load);

  useEffect(() => {
    void load();
    // Re-opened ("마법사 다시 실행", tray): start over with fresh data.
    const un = onNavigate((t) => {
      if ((t as string) === "wizard") void load();
    });
    return () => {
      void un.then((f) => f());
    };
  }, [load]);

  const later = () => void hideCurrentWindow();
  const checked = w.folders.filter((f) => f.checked);
  const starters = w.info?.starters ?? [];
  const countOf = (i: number) => w.counts?.rules.find((r) => r.index === i);
  const customOffset = starters.length;
  const validCustom = w.custom.filter((c) => c.name.trim() && c.dest.trim() && (c.ext.length || c.nameContains.trim()));
  const collectMissing = checked.some((f) => f.mode === "collect" && !f.organizeRoot);

  if (!w.info) return <p className="pad muted">{w.error ?? "불러오는 중…"}</p>;

  return (
    <div className="wizard" data-testid="wizard">
      <header className="wizard-head">
        <h1>Tidy 시작하기</h1>
        <ol className="steps">
          {STEPS.map((s, i) => (
            <li key={s} className={w.step === i + 1 ? "on" : w.step > i + 1 ? "done" : ""}>
              {i + 1}. {s}
            </li>
          ))}
        </ol>
        <p className="muted small">
          {w.step <= 4
            ? "마법사를 끝내기 전에는 아무 파일도 옮기지 않습니다. 고른 것만 저장됩니다."
            : "감시를 시작했습니다."}
        </p>
      </header>

      <section className="wizard-body">
        {w.step === 1 && (
          <div data-testid="step-folders">
            <h2>어떤 폴더를 지켜볼까요?</h2>
            <p className="muted small">
              새 파일이 생기면 규칙대로 정리합니다. 폴더마다 정리 방식을 고를 수 있습니다:{" "}
              <b>이 폴더 안에서 정리</b>(하위 폴더를 만들어 그 안에서만 분류) 또는 <b>지정 위치로 모으기</b>(예:
              D:\정리 아래로). <Help id="organizeMode" />
            </p>
            <table className="grid">
              <tbody>
                {w.folders.map((f, i) => (
                  <tr key={f.path} data-testid="wizard-folder">
                    <td>
                      <label>
                        <input
                          type="checkbox"
                          checked={f.checked}
                          onChange={(e) => w.setFolder(i, { checked: e.target.checked })}
                        />{" "}
                        <b>{f.label}</b>
                      </label>
                      <div className="muted small path">{f.path}</div>
                    </td>
                    <td>{f.checked && <OrganizeModeField w={f} onChange={(p) => w.setFolder(i, p)} />}</td>
                  </tr>
                ))}
              </tbody>
            </table>
            <button
              onClick={async () => {
                const dir = await ipc.pickFolder();
                if (dir) await w.addFolder(dir);
              }}
            >
              다른 폴더…
            </button>
          </div>
        )}

        {w.step === 2 && (
          <div data-testid="step-rules">
            <h2>
              어떻게 나눌까요? <Help id="ruleOrder" />
            </h2>
            <p className="muted small">
              체크한 것만 규칙이 됩니다. 숫자는 지금 고른 폴더에 있는 파일 중 해당하는 수입니다(아직 옮기지 않음).
              {!!w.info.existingRules.length &&
                ` 이미 있는 규칙 ${w.info.existingRules.length}개(${w.info.existingRules.join(", ")})는 그대로 두고, 같은 이름의 시작 규칙은 건너뜁니다.`}
            </p>
            <ul className="starter-list">
              {starters.map((s, i) => {
                const c = countOf(i);
                return (
                  <li key={s.id} data-testid="starter">
                    <label>
                      <input type="checkbox" checked={w.starters.has(s.id)} onChange={() => w.toggleStarter(s.id)} />{" "}
                      <b>{s.title}</b> <span className="muted">{s.summary}</span>
                    </label>
                    <span className="match-count" data-testid="starter-count">
                      {c ? `지금 ${c.count}개 해당${c.examples.length ? ` · 예: ${c.examples.join(", ")}` : ""}` : ""}
                    </span>
                  </li>
                );
              })}
            </ul>
            <h3>기타 직접 추가</h3>
            {w.custom.map((c, i) => {
              const k = validCustom.indexOf(c);
              const cnt = k >= 0 ? countOf(customOffset + k) : undefined;
              return (
                <div className="row custom-rule" key={i} data-testid="custom-rule">
                  <input placeholder="이름" value={c.name} onChange={(e) => w.setCustom(i, { name: e.target.value })} />
                  <input
                    placeholder="확장자 (쉼표로: psd, ai)"
                    value={c.ext.join(", ")}
                    onChange={(e) =>
                      w.setCustom(i, {
                        ext: e.target.value
                          .split(/[,\s]+/)
                          .map((x) => x.replace(/^\./, "").toLowerCase())
                          .filter(Boolean),
                      })
                    }
                  />
                  <input
                    placeholder="폴더 (예: 디자인/{yyyy})"
                    value={c.dest}
                    onChange={(e) => w.setCustom(i, { dest: e.target.value })}
                  />
                  <span className="match-count">{cnt ? `지금 ${cnt.count}개 해당` : ""}</span>
                  <button className="link" onClick={() => w.removeCustom(i)}>
                    삭제
                  </button>
                </div>
              );
            })}
            <button onClick={() => w.addCustom()}>+ 직접 추가</button>
          </div>
        )}

        {w.step === 3 && (
          <div data-testid="step-ai">
            <h2>AI를 쓸까요? (선택)</h2>
            <p className="muted small">
              규칙에 맞지 않는 파일을 AI에 물어보고, 검색창에서 자연어로 찾을 수 있습니다. 건너뛰면 규칙과 단어
              검색만으로 동작합니다. 키는 OS 키체인에만 저장됩니다. <Help id="keychain" />
            </p>
            <p className="notice small" data-testid="wizard-curate-note">
              고객사·이슈별로 묶는 <b>업무 맥락 정리</b>를 쓰려면 AI가 필요합니다. 여기서 켜 두면 나중에 설정 → 업무
              맥락에서 "바탕화면에서 뽑기"로 고객사 사전을 만들 수 있습니다.
            </p>
            <AiSettingsSection />
          </div>
        )}

        {w.step === 4 && (
          <div data-testid="step-summary">
            <h2>이렇게 정리합니다</h2>
            {checked.map((f) => (
              <div key={f.path} className="summary-folder">
                <b>{f.path}</b>
                <div className="muted small">
                  {f.mode === "collect" ? `→ ${f.organizeRoot ?? "(기준 폴더 없음)"} 아래로 모으기` : "→ 이 폴더 안에서 정리"}
                </div>
                <ul>
                  {[...starters.filter((s) => w.starters.has(s.id)).map((s) => s.card), ...validCustom].map((c) => (
                    <li key={c.name}>
                      {c.name}: {c.ext.length ? c.ext.map((e) => `.${e}`).join(" ") : c.nameContains} →{" "}
                      {join(anchorOf(f), c.dest)}
                      {c.confirm ? " (옮기기 전 확인)" : ""}
                    </li>
                  ))}
                </ul>
              </div>
            ))}
            {!checked.length && <p className="error">감시할 폴더를 하나 이상 고르세요.</p>}
            {!w.starters.size && !validCustom.length && (
              <p className="muted">규칙을 고르지 않았습니다. 감시만 하고, 규칙은 나중에 설정 탭에서 만들 수 있습니다.</p>
            )}
          </div>
        )}

        {w.step === 5 && w.result && (
          <div data-testid="step-done">
            <h2>감시를 시작했습니다</h2>
            <p>
              지금 있는 파일 {w.result.existingFiles}개 중 {w.result.matchingFiles}개가 규칙에 걸립니다. 이미 있던 파일은
              건드리지 않았습니다.
            </p>
            {w.result.matchingFiles > 0 && (
              <p className="muted small">미리보기로 확인하고, 고른 것만 한 번에 정리할 수 있습니다(승인 전에는 옮기지 않음).</p>
            )}
            <div className="actions">
              {w.result.matchingFiles > 0 && (
                <button
                  className="primary"
                  onClick={async () => {
                    const path = w.result?.folders[0] ?? "";
                    await ipc.showWindow(`main:report?plan=rules&path=${encodeURIComponent(path)}`);
                    later();
                  }}
                >
                  기존 파일 미리보기
                </button>
              )}
              <button onClick={later}>닫기</button>
            </div>
          </div>
        )}
        {w.error && <p className="error">{w.error}</p>}
      </section>

      {w.step <= 4 && (
        <footer className="wizard-foot">
          <button onClick={later}>나중에</button>
          <div className="spacer" />
          {w.step > 1 && <button onClick={() => w.go(w.step - 1)}>이전</button>}
          {w.step === 3 && <button onClick={() => w.go(4)}>건너뛰기</button>}
          {w.step < 4 && (
            <button
              className="primary"
              disabled={w.step === 1 && (!checked.length || collectMissing)}
              onClick={() => w.go(w.step + 1)}
            >
              다음
            </button>
          )}
          {w.step === 4 && (
            <button className="primary" disabled={!checked.length || w.finishing} onClick={() => void w.finish()}>
              {w.finishing ? "시작하는 중…" : "감시 시작"}
            </button>
          )}
        </footer>
      )}
    </div>
  );
}
