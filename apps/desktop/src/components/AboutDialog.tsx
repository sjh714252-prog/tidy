import { useEffect, useState } from "react";
import * as ipc from "../ipc/commands";
import type { AboutInfo, UpdateResult } from "../ipc/types";
import { useHome } from "../store/home";
import { useStatus } from "../store/status";
import { REPO } from "./HelpMenu";
import icon from "../../src-tauri/icons/128x128.png";

/**
 * 정보 창 (SPEC §14.6): icon, version, build, repository, license, config /
 * DB / log folders, and "업데이트 확인" (one GET, never blocking).
 */
export function AboutDialog() {
  const { aboutOpen, checkOnOpen, closeAbout } = useHome();
  const refreshStatus = useStatus((s) => s.refresh);
  const [info, setInfo] = useState<AboutInfo | null>(null);
  const [upd, setUpd] = useState<UpdateResult | null>(null);
  const [checking, setChecking] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  const check = async () => {
    setChecking(true);
    try {
      setUpd(await ipc.checkUpdate());
      void refreshStatus();
    } catch (e) {
      setUpd({ current: info?.version ?? "", info: null, error: String(e), checkedAt: 0 });
    } finally {
      setChecking(false);
    }
  };

  useEffect(() => {
    if (!aboutOpen) return;
    setUpd(null);
    setErr(null);
    void ipc.aboutInfo().then(setInfo, (e) => setErr(String(e)));
    if (checkOnOpen) void check();
    const esc = (e: KeyboardEvent) => e.key === "Escape" && closeAbout();
    window.addEventListener("keydown", esc);
    return () => window.removeEventListener("keydown", esc);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [aboutOpen, checkOnOpen]);

  if (!aboutOpen) return null;
  const open = (f: () => Promise<void>) => void f().catch((e) => setErr(String(e)));

  return (
    <div className="modal-back" onMouseDown={(e) => e.target === e.currentTarget && closeAbout()}>
      <div className="modal about" role="dialog" aria-label="Tidy 정보" data-testid="about-modal">
        <header className="row">
          <img src={icon} alt="" width={48} height={48} />
          <div>
            <h2>Tidy</h2>
            <div className="muted small">바탕화면·폴더 자동 정리 + 본문 검색</div>
          </div>
          <div className="spacer" />
          <button className="link" onClick={closeAbout} aria-label="닫기">
            ✕
          </button>
        </header>
        {info ? (
          <table className="grid kv">
            <tbody>
              <tr>
                <th>버전</th>
                <td data-testid="about-version">{info.version}</td>
              </tr>
              <tr>
                <th>빌드</th>
                <td data-testid="about-build">{info.gitSha}</td>
              </tr>
              <tr>
                <th>저장소</th>
                <td>
                  <button className="link" onClick={() => open(() => ipc.openUrl(REPO))}>
                    {REPO.replace("https://", "")}
                  </button>
                </td>
              </tr>
              <tr>
                <th>라이선스</th>
                <td>{info.license}</td>
              </tr>
              <tr>
                <th>설정 파일</th>
                <td className="path" title={info.configPath}>
                  {info.configPath}{" "}
                  <button className="link" onClick={() => open(() => ipc.openFolder(info.configDir))}>
                    폴더 열기
                  </button>
                </td>
              </tr>
              <tr>
                <th>DB</th>
                <td className="path" title={info.dbPath}>
                  {info.dbPath}{" "}
                  <button className="link" onClick={() => open(() => ipc.openFolder(info.dbDir))}>
                    폴더 열기
                  </button>
                </td>
              </tr>
              <tr>
                <th>로그</th>
                <td className="path" title={info.logDir}>
                  <button className="link" onClick={() => open(() => ipc.openFolder(info.logDir))}>
                    로그 폴더 열기
                  </button>
                </td>
              </tr>
            </tbody>
          </table>
        ) : (
          <p className="muted">{err ?? "불러오는 중…"}</p>
        )}
        <div className="update-box" data-testid="update-box">
          <button onClick={() => void check()} disabled={checking}>
            {checking ? "확인 중…" : "업데이트 확인"}
          </button>{" "}
          {upd && <UpdateLine u={upd} onErr={setErr} />}
          {!upd && info && !info.checkUpdates && (
            <span className="muted small">하루 한 번 자동 확인이 꺼져 있습니다(설정 ⑥ 고급).</span>
          )}
        </div>
        {err && <p className="error small">{err}</p>}
      </div>
    </div>
  );
}

function UpdateLine({ u, onErr }: { u: UpdateResult; onErr: (e: string) => void }) {
  if (!u.info)
    return (
      <span className="warn small" data-testid="update-result">
        확인하지 못했습니다(네트워크). 나중에 다시 해 보세요.
        {u.error && <span className="muted"> — {u.error}</span>}
      </span>
    );
  if (!u.info.newer)
    return (
      <span className="ok small" data-testid="update-result">
        최신 버전입니다 ({u.current})
      </span>
    );
  const info = u.info;
  return (
    <span className="small" data-testid="update-result">
      <b>새 버전 {info.latest}</b> (지금 {u.current}){" "}
      <button className="primary" onClick={() => void ipc.openUrl(info.url).catch((e) => onErr(String(e)))}>
        받기
      </button>
    </span>
  );
}
