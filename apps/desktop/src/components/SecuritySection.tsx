import { useEffect } from "react";
import * as ipc from "../ipc/commands";
import type { PiiSettings } from "../ipc/types";
import { MIN_PASSWORD, useSecurity } from "../store/security";

const ITEMS: { key: "rrn" | "account" | "namePhone" | "nameEmail"; min: keyof PiiSettings; label: string; tip: string }[] = [
  { key: "rrn", min: "minRrn", label: "주민등록번호", tip: "앞 6자리-뒤 7자리, 날짜·검증 자릿수까지 맞을 때" },
  { key: "account", min: "minAccount", label: "계좌번호", tip: "10~14자리 번호 근처에 '계좌·입금·예금주'나 은행명이 있을 때" },
  { key: "namePhone", min: "minNamePhone", label: "이름 + 휴대전화", tip: "같은 줄(표에서는 같은 행)에 이름과 010 번호가 함께" },
  { key: "nameEmail", min: "minNameEmail", label: "이름 + 이메일", tip: "같은 줄(표에서는 같은 행)에 이름과 메일 주소가 함께" },
];

/**
 * Settings → 보안(개인정보) (SPEC §14.2): what counts as personal
 * information, where such files go, whether to ask first, and the vault
 * password (OS keychain only). Saved by the settings tab's one save button.
 */
export function SecuritySection() {
  const { draft, password, password2, message, error, load, patch, patchPii, setPassword, invalid } = useSecurity();

  useEffect(() => {
    void load();
  }, [load]);

  if (!draft) return <p className="muted small">{error ?? "보안 설정 불러오는 중…"}</p>;
  const p = draft.pii;
  const bad = invalid();
  const pickVault = async () => {
    const dir = await ipc.pickFolder(draft.vaultDir || null);
    if (dir) patch({ vaultDir: dir });
  };

  return (
    <section className="security" data-testid="security-section">
      <h2>보안(개인정보)</h2>
      <p className="muted small">
        본문과 파일 이름에서 개인정보를 찾아(인터넷 없이) 암호화 zip(AES-256)으로 보관합니다. 암호화한 파일도 검색에
        🔒로 나오고, 되돌리기로 원래 자리에 풀 수 있습니다. 지금 인덱스에서 개인정보 의심 {draft.suspected}개 중{" "}
        {draft.unencrypted}개가 암호화되지 않았고, 암호화된 보관 파일은 {draft.locked}개입니다.
      </p>

      <h3>무엇을 개인정보로 볼까</h3>
      <label className="row">
        <input type="checkbox" checked={p.enabled} onChange={(e) => patchPii({ enabled: e.target.checked })} />
        <span>개인정보 탐지 사용</span>
      </label>
      <table className="grid pii-items">
        <thead>
          <tr>
            <th>항목</th>
            <th>탐지</th>
            <th>이 건수 이상이면</th>
          </tr>
        </thead>
        <tbody>
          {ITEMS.map((it) => (
            <tr key={it.key} title={it.tip}>
              <td>{it.label}</td>
              <td>
                <input
                  type="checkbox"
                  aria-label={it.label}
                  disabled={!p.enabled}
                  checked={p[it.key]}
                  onChange={(e) => patchPii({ [it.key]: e.target.checked } as Partial<PiiSettings>)}
                />
              </td>
              <td>
                <input
                  type="number"
                  min={1}
                  className="narrow"
                  aria-label={`${it.label} 임계치`}
                  disabled={!p.enabled || !p[it.key]}
                  value={p[it.min] as number}
                  onChange={(e) => patchPii({ [it.min]: Math.max(1, Number(e.target.value) || 1) } as Partial<PiiSettings>)}
                />
                건
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      <label className="row">
        <span>제외 패턴</span>
        <input
          className="wide"
          placeholder="^010-?0000-?0000$, 회사 대표 계좌…"
          disabled={!p.enabled}
          value={p.exclude.join(", ")}
          onChange={(e) =>
            patchPii({
              exclude: e.target.value
                .split(",")
                .map((x) => x.trim())
                .filter((x, i, a) => x || i === a.length - 1),
            })
          }
        />
      </label>
      <p className="muted small">정규식, 쉼표로 구분. 찾은 값(그대로 또는 숫자만)이 맞으면 세지 않습니다.</p>
      <label className="row">
        <input
          type="checkbox"
          disabled={!p.enabled}
          checked={p.aiCheck}
          onChange={(e) => patchPii({ aiCheck: e.target.checked })}
        />
        <span>AI가 켜져 있으면 한 번 더 확인 (숫자·메일 아이디는 가려서 보냄, 실패하면 정규식 판단 유지)</span>
      </label>

      <h3>어떻게 보관할까</h3>
      <label className="row">
        <input
          type="radio"
          name="sec-mode"
          checked={draft.mode === "rules"}
          onChange={() => patch({ mode: "rules" })}
        />
        <span>규칙 안에서 암호화 — 규칙 카드의 "개인정보면 암호화"를 켠 규칙만, 그 규칙의 목적지에 zip으로</span>
      </label>
      <label className="row">
        <input
          type="radio"
          name="sec-mode"
          checked={draft.mode === "vault"}
          onChange={() => patch({ mode: "vault" })}
        />
        <span>보안 폴더로 모으기 — 개인정보 파일은 다른 규칙보다 먼저 보안 폴더로(암호화)</span>
      </label>
      <label className="row">
        <span>보안 폴더</span>
        <input
          className="wide"
          placeholder="예: D:\정리\보안"
          value={draft.vaultDir}
          onChange={(e) => patch({ vaultDir: e.target.value })}
        />
        <button onClick={() => void pickVault()}>폴더 선택…</button>
      </label>
      <label className="row">
        <span>승인 방식</span>
        <select value={draft.approval} onChange={(e) => patch({ approval: e.target.value as "confirm" | "auto" })}>
          <option value="confirm">확인 후 암호화 (기본) — 팝업에 파일명·탐지 항목·건수</option>
          <option value="auto">자동 — 묻지 않고 암호화</option>
        </select>
      </label>

      <h3>보관 비밀번호</h3>
      <p className="small" data-testid="vault-key-state">
        {draft.hasPassword ? (
          <span className="ok">
            ✓ 설정됨 — {draft.keyStore}에 저장
            {draft.previousPasswords > 0 && ` · 이전 비밀번호 ${draft.previousPasswords}개 보관(예전 zip용)`}
          </span>
        ) : (
          <span className="warn">아직 없음 — 비밀번호를 정하기 전에는 자동 모드여도 확인 팝업으로 묻습니다.</span>
        )}
        {draft.keyError && <span className="error"> 키체인 오류: {draft.keyError}</span>}
      </p>
      <label className="row">
        <span>{draft.hasPassword ? "새 비밀번호" : "비밀번호"}</span>
        <input
          type="password"
          autoComplete="new-password"
          placeholder={`${MIN_PASSWORD}자 이상`}
          value={password}
          onChange={(e) => setPassword(e.target.value, password2)}
        />
        <span>한 번 더</span>
        <input
          type="password"
          autoComplete="new-password"
          value={password2}
          onChange={(e) => setPassword(password, e.target.value)}
        />
      </label>
      <p className="muted small">
        비밀번호는 OS 키체인에만 저장되고 설정 파일·로그에는 남지 않습니다. 바꿔도 이미 만든 zip은 다시 암호화하지 않고
        이전 비밀번호로 엽니다. zip은 반디집·7-Zip·알집 같은 AES 지원 압축 프로그램이나 Tidy 검색의 "열기"로 열 수
        있습니다(Windows 탐색기 기본 기능은 AES zip을 열지 못함).
      </p>
      {bad && (password || password2 || draft.mode === "vault") && <p className="warn small">{bad}</p>}
      {error && <p className="error small">{error}</p>}
      {message && !error && <p className="ok small">{message}</p>}
    </section>
  );
}
