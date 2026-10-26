import { useEffect } from "react";
import * as ipc from "../ipc/commands";
import { useReport } from "../store/report";
import { formatSize, formatTime } from "../components/format";
import { RestructurePanel } from "../components/RestructurePanel";
import { shortenTo, watchOf } from "../components/paths";
import { useWatchNames } from "../store/status";

/** 정리하기 (SPEC §9.5, §14.6): 정리 제안 + stats cards, types, big folders/files, duplicates, stale files, tree. */
export function ReportView() {
  const r = useReport();
  const names = useWatchNames();
  const loadRoots = useReport((s) => s.loadRoots);
  useEffect(() => {
    void loadRoots();
  }, [loadRoots]);

  const pick = async () => {
    const dir = await ipc.pickFolder(r.path || null);
    if (dir) void r.setPath(dir);
  };
  const rep = r.report;
  const join = (rel: string) => `${rep?.root ?? ""}/${rel}`;
  const wasted = rep?.duplicates.reduce((a, d) => a + d.wasted, 0) ?? 0;

  return (
    <div className="page report">
      <div className="toolbar">
        <label className="check" style={{ fontWeight: 600 }}>
          정리할 폴더
          <select
            value={r.path}
            data-testid="report-folder"
            aria-label="정리할 폴더"
            onChange={(e) => void r.setPath(e.target.value)}
          >
            {r.path && !r.roots.includes(r.path) && <option value={r.path}>{shortenTo(r.path, 40)} (다른 폴더)</option>}
            {r.roots.map((p) => {
              const w = watchOf(p, names);
              const label = w && w.path === p ? `${w.alias} — ${shortenTo(p, 32)}` : shortenTo(p, 48);
              return (
                <option key={p} value={p} title={p}>
                  {label}
                </option>
              );
            })}
          </select>
        </label>
        <button onClick={() => void pick()}>다른 폴더…</button>
      </div>
      {r.target && (
        <div className="notice warn" data-testid="organize-root-warning">
          ⚠ 이 폴더는 정리 결과가 모이는 곳입니다({r.target}). 여기서 정리하면 이미 정리된 파일을 다시 옮깁니다.
        </div>
      )}
      {r.error && <div className="notice warn error">{r.error}</div>}

      <RestructurePanel />

      <section className="card">
        <div className="card-head">
          <h2>폴더 리포트</h2>
          <span className="sub">파일 종류·큰 폴더·중복·오래 안 쓴 파일 (읽기만 합니다)</span>
          <span className="spacer" />
          <label className="check">
            <input type="checkbox" checked={r.dups} onChange={(e) => r.setDups(e.target.checked)} /> 중복 찾기(내용 읽음)
          </label>
          <button disabled={!r.path || r.loading} onClick={() => void r.build()}>
            {r.loading ? "분석 중…" : "리포트 만들기"}
          </button>
        </div>
      {rep && (
        <div>
          <div className="stat-cards">
            <Card label="파일" value={rep.stats.total_files.toLocaleString()} />
            <Card label="폴더" value={rep.stats.total_dirs.toLocaleString()} />
            <Card label="용량" value={formatSize(rep.stats.total_bytes)} />
            <Card label="맨 위 파일" value={rep.stats.root_level_files.toLocaleString()} />
            <Card label={`${rep.staleDays}일 넘게 안 쓴 파일`} value={rep.stats.stale_files.toLocaleString()} />
            <Card label="중복 낭비" value={formatSize(wasted)} />
            {rep.pii && <Card label="개인정보 의심 (미암호화)" value={`${rep.piiSuspected} (${rep.piiUnencrypted})`} />}
          </div>
          {rep.pii && (
            <p className={rep.piiUnencrypted ? "warn small" : "muted small"} data-testid="report-pii">
              🔒 {rep.pii} — 검색 인덱스 기준. 설정 → 보안(개인정보)에서 보관 방식을 정할 수 있습니다.
            </p>
          )}

          <div className="cols">
            <section>
              <h3>종류별</h3>
              <table className="grid">
                <tbody>
                  {rep.stats.by_type.slice(0, 15).map((t) => (
                    <tr key={t.ext}>
                      <td>{t.ext}</td>
                      <td className="num">{t.count}</td>
                      <td className="num">{formatSize(t.bytes)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </section>
            <section>
              <h3>큰 폴더</h3>
              <table className="grid">
                <tbody>
                  {rep.stats.largest_dirs.map((d) => (
                    <tr key={d.path}>
                      <td className="path" title={d.path}>
                        <button className="link" onClick={() => void ipc.revealPath(join(d.path))}>
                          {d.path}
                        </button>
                      </td>
                      <td className="num">{d.files}</td>
                      <td className="num">{formatSize(d.bytes)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
              <h3>큰 파일</h3>
              <table className="grid">
                <tbody>
                  {rep.stats.largest_files.map((f) => (
                    <tr key={f.path}>
                      <td className="path" title={f.path}>
                        <button className="link" onClick={() => void ipc.revealPath(join(f.path))}>
                          {f.path}
                        </button>
                      </td>
                      <td className="num">{formatSize(f.bytes)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </section>
          </div>

          <h3>중복 파일 ({rep.duplicates.length}그룹)</h3>
          {rep.duplicates.length === 0 ? (
            <p className="muted">{r.dups ? "없음" : "중복 찾기가 꺼져 있습니다"}</p>
          ) : (
            <table className="grid">
              <tbody>
                {rep.duplicates.map((g) => (
                  <tr key={g.hash}>
                    <td>
                      {g.paths.map((p) => (
                        <div key={p}>
                          <button className="link" onClick={() => void ipc.revealPath(join(p))}>
                            {p}
                          </button>
                        </div>
                      ))}
                    </td>
                    <td className="num">
                      {formatSize(g.size)} × {g.paths.length}
                    </td>
                    <td className="num">낭비 {formatSize(g.wasted)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
          <p className="muted small">삭제 기능은 없습니다. 필요하면 폴더를 열어 직접 휴지통으로 보내세요.</p>

          <h3>오래 안 쓴 파일</h3>
          {rep.stats.stale.length === 0 ? (
            <p className="muted">없음</p>
          ) : (
            <table className="grid">
              <tbody>
                {rep.stats.stale.map((f) => (
                  <tr key={f.path}>
                    <td className="path" title={f.path}>
                      <button className="link" onClick={() => void ipc.revealPath(join(f.path))}>
                        {f.path}
                      </button>
                    </td>
                    <td className="nowrap muted">{formatTime(f.last_used)}</td>
                    <td className="num">{formatSize(f.bytes)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
          {rep.stats.empty_dirs.length > 0 && (
            <>
              <h3>빈 폴더 ({rep.stats.empty_dirs.length})</h3>
              <p className="small">{rep.stats.empty_dirs.slice(0, 50).join(" · ")}</p>
            </>
          )}
          <h3>폴더 트리</h3>
          <pre className="tree">{rep.treeText}</pre>
          {rep.errors.length > 0 && <p className="warn small">읽지 못한 항목 {rep.errors.length}개</p>}
          <p className="muted small">HTML로 저장: tidy report "{rep.root}" --format html &gt; 리포트.html</p>
        </div>
      )}
      {!rep && !r.loading && <p className="muted small" style={{ margin: 0 }}>"리포트 만들기"를 누르면 이 폴더의 통계를 보여 줍니다.</p>}
      </section>
    </div>
  );
}

function Card({ label, value }: { label: string; value: string }) {
  return (
    <div className="stat">
      <span className="muted small">{label}</span>
      <b>{value}</b>
    </div>
  );
}
