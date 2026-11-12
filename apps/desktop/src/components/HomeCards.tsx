import { useCallback, useEffect, useState } from "react";
import * as ipc from "../ipc/commands";
import { onActivity, onOrganize, onStatus } from "../ipc/events";
import type { CustomerStat, FolderOverview, Overview, StoredPlan, TodaySummary } from "../ipc/types";
import { organizeFolders, organizeNow, useHome } from "../store/home";
import { useNav } from "../store/nav";
import { useStatus } from "../store/status";
import { useAiOn } from "./AiNeeded";
import { PathText } from "./PathText";
import { aliasOf, baseName } from "./paths";
import { useReport } from "../store/report";

const STATE: Record<FolderOverview["state"], { label: string; cls: string }> = {
  ok: { label: "정상", cls: "ok" },
  offline: { label: "연결 끊김", cls: "warn" },
  paused: { label: "일시정지", cls: "muted" },
  idle: { label: "감시 전", cls: "muted" },
};

function count(f: FolderOverview, o: Overview): string {
  if (f.network) return "네트워크 폴더 — 세지 않음";
  if (!f.backlog) return o.computing ? "세는 중…" : "—";
  const n = f.backlog.capped ? `${o.cap.toLocaleString()}+` : f.backlog.total.toLocaleString();
  return `${n}개`;
}

function useOverview() {
  const { overview, load } = useHome();
  useEffect(() => {
    void load();
    const subs = [onOrganize(() => void load()), onStatus(() => void load())];
    return () => subs.forEach((p) => void p.then((f) => f()));
  }, [load]);
  return overview;
}

/**
 * "아직 정리 안 된 기존 파일" (SPEC §14.8 3): one card per watch folder —
 * alias / path / count · 새 폴더 N / [지금 정리] (a preview; nothing moves
 * until it is applied).
 */
export function OrganizeCard() {
  const o = useOverview();
  if (!o || !o.folders.length) return null;
  return (
    <section className="card" data-testid="organize-card">
      <div className="card-head baseline">
        <h2>아직 정리 안 된 기존 파일</h2>
        <span className="sub">
          감시를 시작하기 전부터 있던 파일은 [지금 정리] → 미리보기 → [적용]을 눌러야 옮겨집니다.{" "}
          {o.askBeforeMove
            ? "새 파일도 혼자 옮기지 않고 확인 대기에서 먼저 묻습니다 (옮기기 전에 항상 묻기)."
            : "새 파일은 감시 중 정리되고, 새 고객사·이슈 폴더가 필요하면 확인 대기에서 묻습니다."}
        </span>
      </div>
      <UnappliedPlans />
      <div className="folder-grid">
        {o.folders.map((f) => (
          <FolderCard key={f.path} f={f} o={o} />
        ))}
      </div>
    </section>
  );
}

function FolderCard({ f, o }: { f: FolderOverview; o: Overview }) {
  const aiOn = useAiOn();
  const b = f.backlog;
  const files = b?.total ?? 0;
  const folders = b?.newFolders ?? 0;
  const n = b ? (b.capped ? `${o.cap.toLocaleString()}+` : files.toLocaleString()) : null;
  const curateTitle = aiOn
    ? "하위 폴더를 통째로 업무 맥락(고객사/날짜_이슈/종류)으로 정리합니다. 미리보기 후 승인해야 옮깁니다."
    : "폴더를 통째로 정리하려면 AI가 필요합니다 (설정 → AI 연결)";
  return (
    <div className="folder-card" data-testid="backlog-folder" data-path={f.path}>
      <div className="alias">{aliasOf(f.path, f.alias)}</div>
      <PathText path={f.path} max={24} />
      {f.network ? (
        <div className="muted small">네트워크 폴더 — 세지 않음</div>
      ) : !b ? (
        <div className="muted small">{o.computing ? "세는 중…" : "—"}</div>
      ) : (
        <div
          className="big"
          data-testid="backlog-count"
          title={`규칙에 맞음 ${b.toMove.toLocaleString()} · 규칙 없음 ${b.noRule.toLocaleString()}`}
        >
          {n}
          <small> 개</small>
          {folders > 0 &&
            (files > 0 ? (
              <small>
                {" "}
                ·{" "}
                <button
                  className="link"
                  data-testid="new-folders"
                  disabled={!aiOn}
                  title={curateTitle}
                  onClick={() => void organizeFolders(f)}
                >
                  정리할 폴더 {folders}
                </button>
              </small>
            ) : (
              <small data-testid="new-folders"> · 정리할 폴더 {folders}</small>
            ))}
        </div>
      )}
      {!!b?.settling && (
        <span className="muted small" title="복사 중이거나 방금 바뀐 폴더는 조용해진 뒤 셉니다">
          복사 중 {b.settling}개
        </span>
      )}
      {b && files > 0 ? (
        <button className="primary" data-testid="organize-now" onClick={() => void organizeNow(f, o.canCurate)}>
          지금 정리
        </button>
      ) : b && folders > 0 ? (
        <button
          className="primary"
          data-testid="organize-now"
          disabled={!aiOn}
          title={curateTitle}
          onClick={() => void organizeFolders(f)}
        >
          지금 정리
        </button>
      ) : b && !b.kept?.length ? (
        <span className="done">✓ 정리됨</span>
      ) : null}
      {!!b?.kept?.length && <KeptFolders kept={b.kept} />}
    </div>
  );
}

/**
 * (0.13.1) Sub-folders the count leaves out, each with why — so a card
 * never reads "0개 · 정리됨" over a desktop full of folders without saying
 * what it skipped.
 */
function KeptFolders({ kept }: { kept: { name: string; path: string; reason: string }[] }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="small" data-testid="kept-folders">
      <button className="link small" aria-expanded={open} onClick={() => setOpen(!open)}>
        그대로 둔 폴더 {kept.length}개 {open ? "▾" : "▸ 왜?"}
      </button>
      {open && (
        <ul className="kept-list">
          {kept.map((k) => (
            <li key={k.path} title={k.path}>
              <b>{k.name}</b> — <span className="muted">{k.reason}</span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

const two = (n: number) => String(n).padStart(2, "0");
const hm = (t: number) => {
  const d = new Date(t * 1000);
  return `${two(d.getHours())}:${two(d.getMinutes())}`;
};
const md = (t: number) => {
  const d = new Date(t * 1000);
  return `${d.getMonth() + 1}/${d.getDate()}`;
};
const midnight = () => {
  const d = new Date();
  d.setHours(0, 0, 0, 0);
  return Math.floor(d.getTime() / 1000);
};

/** 오늘 한 일: today's moves from the move log (a folder moved whole = one row). */
export function TodayCard({ onShowAll }: { onShowAll: () => void }) {
  const [t, setT] = useState<TodaySummary | null>(null);
  const pending = useStatus((s) => s.status?.pendingCount ?? 0);
  const load = useCallback(() => void ipc.todayMoves(midnight(), 8).then(setT).catch(() => {}), []);
  useEffect(() => {
    load();
    let timer: ReturnType<typeof setTimeout> | undefined;
    const soon = () => {
      clearTimeout(timer);
      timer = setTimeout(load, 400);
    };
    const subs = [onActivity(soon), onOrganize(soon)];
    return () => {
      clearTimeout(timer);
      subs.forEach((p) => void p.then((f) => f()));
    };
  }, [load]);
  return (
    <section className="card" data-testid="today-card">
      <div className="card-head">
        <h2>오늘 한 일</h2>
        <span className="sub" data-testid="today-counts">
          이동 {(t?.moved ?? 0).toLocaleString()} · 확인 대기 {pending.toLocaleString()} · 되돌림 {(t?.undone ?? 0).toLocaleString()}
        </span>
        <span className="spacer" />
        <button className="link" onClick={onShowAll}>
          전체 보기
        </button>
      </div>
      <div className="list">
        {!t?.items.length ? (
          <div className="list-empty">오늘은 아직 옮긴 파일이 없습니다.</div>
        ) : (
          t.items.map((x) => (
            <div key={`${x.batch_id}:${x.src}`} className="list-row done-row" data-testid="today-row">
              <span className="muted">{hm(x.at)}</span>
              <span className="ellipsis" title={x.src}>
                {x.name}
                {x.is_dir && <span className="muted"> (폴더 · 파일 {x.files.toLocaleString()})</span>}
              </span>
              <PathText path={x.dest_dir} prefix="→ " />
            </div>
          ))
        )}
      </div>
    </section>
  );
}

/** (M12) 고객사별 보기: issue folders · files · last change; 열기 opens the folder. */
export function CustomerCard() {
  const { customers, loadCustomers } = useHome();
  const goSettings = useNav((s) => s.goSettings);
  useEffect(() => {
    void loadCustomers();
    const sub = onOrganize(() => void loadCustomers());
    return () => void sub.then((f) => f());
  }, [loadCustomers]);
  return (
    <section className="card" data-testid="customer-card">
      <div className="card-head">
        <h2>고객사별 보기</h2>
        <span className="spacer" />
        <button className="link" onClick={() => goSettings("context")}>
          사전 편집
        </button>
      </div>
      <div className="list">
        {!customers?.length ? (
          <div className="list-empty">업무 맥락 정리를 하면 고객사별로 모아 보여 줍니다.</div>
        ) : (
          customers.map((c) => <CustomerRow key={c.name} c={c} />)
        )}
      </div>
    </section>
  );
}

function CustomerRow({ c }: { c: CustomerStat }) {
  const open = () => void ipc.openFolder(c.dirs[0] ?? "").catch((e) => alert(String(e)));
  const other = c.name === "기타";
  return (
    <div className="list-row customer-row" data-testid="customer-row" data-name={c.name}>
      <span style={{ fontWeight: 600 }} className={other ? "muted" : undefined} title={c.dirs.join("\n")}>
        {c.name}
      </span>
      <span className="muted">
        폴더 {c.issues.toLocaleString()} · 파일 {c.files.toLocaleString()}
        {c.capped ? "+" : ""}
        {c.last ? ` · ${md(c.last)}` : ""}
      </span>
      <button className="link small" title={c.dirs.join("\n")} onClick={open}>
        열기
      </button>
    </div>
  );
}

/** 감시 현황 패널: one row per watch folder (opened from the status bar). */
export function WatchPanel() {
  const open = useHome((s) => s.panelOpen);
  const toggle = useHome((s) => s.togglePanel);
  const o = useOverview();
  if (!open || !o) return null;
  return (
    <section className="card watch-panel" data-testid="watch-panel">
      <div className="card-head">
        <h2>감시 현황</h2>
        <span className="spacer" />
        <button className="link" onClick={toggle}>
          접기
        </button>
      </div>
      {o.folders.length === 0 ? (
        <p className="muted">감시 중인 폴더가 없습니다. 설정 → 감시 폴더에서 추가하세요.</p>
      ) : (
        <div className="table">
          <div className="t-head">
            <span>폴더</span>
            <span>정리 방식</span>
            <span>상태</span>
            <span className="hide-narrow">제외</span>
            <span className="hide-narrow">기존 파일 미정리</span>
          </div>
          {o.folders.map((f) => (
            <div key={f.path} className="t-row" data-testid="watch-row">
              <div className="cell">
                <span className="name">
                  {aliasOf(f.path, f.alias)}
                  {f.network && <span className="tag">NAS</span>}
                </span>
                <PathText path={f.path} className="sub-path" />
              </div>
              <span className="small">
                {f.mode === "collect" && f.organizeRoot ? (
                  <PathText path={f.organizeRoot} prefix="→ " />
                ) : (
                  "이 폴더 안"
                )}
              </span>
              <span className={`small ${STATE[f.state].cls}`}>● {STATE[f.state].label}</span>
              <span className="small hide-narrow">{f.exclude ? `${f.exclude}개` : "—"}</span>
              <span className="small hide-narrow">
                {count(f, o)}{" "}
                {!!f.backlog?.total && (
                  <button className="sm" onClick={() => void organizeNow(f, o.canCurate)}>
                    지금 정리
                  </button>
                )}
              </span>
            </div>
          ))}
        </div>
      )}
    </section>
  );
}

const PLAN_SOURCE: Record<string, string> = {
  ai: "AI",
  rules: "규칙",
  context: "업무 맥락",
  command: "명령",
  relocate: "정리 위치로 옮기기",
  imported: "제안 파일",
  compact: "정리 위치 다듬기",
};

/**
 * (0.11.1) Previews made but never applied (last 14 days): nothing moved
 * for them, which looked like "정리가 안 돈다". One click reopens it.
 */
function UnappliedPlans() {
  const [plans, setPlans] = useState<StoredPlan[]>([]);
  const [tick, setTick] = useState(0);
  useEffect(() => {
    // Plans can be saved in the background (0.11.2 relocation): re-read on activity.
    const sub = onActivity((a) => a.kind === "info" && setTick((t) => t + 1));
    return () => void sub.then((f) => f());
  }, []);
  useEffect(() => {
    const since = Date.now() / 1000 - 14 * 86400;
    void ipc
      .listPlans()
      .then((ps) => {
        const seen = new Set<string>();
        setPlans(
          ps.filter((p) => {
            if (p.applied_batch || !p.moves || p.created_at < since || seen.has(p.root)) return false;
            seen.add(p.root);
            return true;
          }),
        );
      })
      .catch(() => setPlans([]));
  }, [tick]);
  if (!plans.length) return null;
  const reopen = async (p: StoredPlan) => {
    useNav.getState().setTab("report");
    await useReport.getState().setPath(p.root);
    await useReport.getState().openPlan(p.id);
  };
  return (
    <div className="notice warn" data-testid="unapplied-plans">
      {plans.map((p) => (
        <div key={p.id} className="row">
          <span>
            <b>적용하지 않은 미리보기</b> · {baseName(p.root)} · {PLAN_SOURCE[p.source] ?? p.source} {p.moves.toLocaleString()}개 —
            아직 아무것도 옮기지 않았습니다.
          </span>
          <button className="sm primary" onClick={() => void reopen(p)}>
            이어서 보기
          </button>
          <button
            className="sm"
            data-testid="discard-plan"
            title="이 미리보기를 지웁니다. 파일은 아무것도 옮기지 않습니다."
            onClick={() => void ipc.discardPlan(p.id).then(() => setTick((t) => t + 1))}
          >
            버리기
          </button>
        </div>
      ))}
    </div>
  );
}
