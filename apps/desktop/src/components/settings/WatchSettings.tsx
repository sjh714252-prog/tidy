import { useState } from "react";
import * as ipc from "../../ipc/commands";
import type { FolderOverview, WatchFolder } from "../../ipc/types";
import { useHome } from "../../store/home";
import { useSettings } from "../../store/settings";
import { Help } from "../Help";
import { PathText } from "../PathText";
import { aliasOf, defaultAlias, knownFolder } from "../paths";

/**
 * 공통 정리 위치 (SPEC §14.8 5): the organize_root most "모으기" folders
 * share (first one on a tie), or null when every folder organizes in place.
 */
export function commonRoot(watches: WatchFolder[]): string | null {
  const n = new Map<string, number>();
  for (const w of watches) if (w.mode === "collect" && w.organizeRoot) n.set(w.organizeRoot, (n.get(w.organizeRoot) ?? 0) + 1);
  let best: string | null = null;
  for (const [r, c] of n) if (best == null || c > (n.get(best) ?? 0)) best = r;
  return best;
}

type Where = "common" | "in_place" | "other";
export function whereOf(w: WatchFolder, common: string | null): Where {
  if (w.mode !== "collect" || !w.organizeRoot) return "in_place";
  return w.organizeRoot === common ? "common" : "other";
}

const ICON: Record<string, string> = { 바탕화면: "D", 다운로드: "↓", 문서: "▤", 사진: "▣" };

function summary(w: WatchFolder, common: string | null): string {
  const parts = [
    w.kind === "network" ? "네트워크(NAS)" : null,
    w.recursive ? "하위 폴더 포함" : "바로 아래 파일만",
    w.indexContent ? "본문 검색" : "이름만 검색",
    whereOf(w, common) === "in_place" ? "이 폴더 안에서 정리" : `정리 위치 ${w.organizeRoot}`,
    w.exclude.length ? `제외 ${w.exclude.length}개` : "제외 없음",
  ];
  return parts.filter(Boolean).join(" · ");
}

const STATE: Record<FolderOverview["state"], { label: string; cls: string }> = {
  ok: { label: "● 정상", cls: "ok" },
  offline: { label: "● 끊김", cls: "warn" },
  paused: { label: "● 일시정지", cls: "muted" },
  idle: { label: "● 감시 전", cls: "muted" },
};

/** 설정 → 감시 폴더: 정리 위치 card + one card per folder, "편집" opens one. */
export function WatchSettings() {
  const s = useSettings();
  const draft = s.draft;
  const overview = useHome((h) => h.overview);
  const [open, setOpen] = useState<number | null>(null);
  if (!draft) return null;
  const common = commonRoot(draft.watches);

  const setCommon = async () => {
    const dir = await ipc.pickFolder(common);
    if (!dir) return;
    // Folders on "공통" follow; with no common root yet, every folder collects there.
    draft.watches.forEach((w, i) => {
      if (common == null || whereOf(w, common) === "common") s.setWatch(i, { mode: "collect", organizeRoot: dir });
    });
  };
  const addFolder = async () => {
    const dir = await ipc.pickFolder();
    if (!dir) return;
    const before = draft.watches.length;
    await s.addWatch(dir);
    const d = useSettings.getState().draft;
    if (d && d.watches.length > before) {
      // New folders join the common organize location when there is one.
      if (common) s.setWatch(before, { mode: "collect", organizeRoot: common });
      setOpen(before);
    }
  };

  return (
    <>
      <div>
        <h1>감시 폴더</h1>
        <p className="lead">이 폴더들에 새 파일이 생기면 규칙대로 정리합니다. 파일 자체는 검색 인덱스에 들어갑니다.</p>
      </div>

      <div className="card root-card" data-testid="common-root">
        <div className="what">
          <b>정리 위치</b>
          <span className="small muted">
            {common
              ? "모든 폴더의 파일이 여기 아래 분류 폴더로 모입니다. 폴더마다 다르게 하려면 편집에서 바꿉니다."
              : "아직 정하지 않았습니다 — 각 폴더 안에서 정리합니다. 정하면 모든 폴더의 파일이 여기 아래로 모입니다."}
          </span>
        </div>
        <span className="root-value" title={common ?? ""} data-testid="common-root-value">
          {common ?? "각 폴더 안"}
        </span>
        <button onClick={() => void setCommon()}>찾아보기…</button>
      </div>

      <div className="stack">
        {draft.watches.map((w, i) => (
          <WatchCard
            key={`${i}:${w.path}`}
            i={i}
            w={w}
            common={common}
            open={open === i}
            state={overview?.folders.find((f) => f.path === w.path)?.state ?? null}
            onToggle={() => setOpen(open === i ? null : i)}
            onRemoved={() => setOpen(null)}
          />
        ))}
      </div>
      <div>
        <button className="dashed" onClick={() => void addFolder()}>
          + 폴더 추가
        </button>
      </div>
    </>
  );
}

function WatchCard({
  i,
  w,
  common,
  open,
  state,
  onToggle,
  onRemoved,
}: {
  i: number;
  w: WatchFolder;
  common: string | null;
  open: boolean;
  state: FolderOverview["state"] | null;
  onToggle: () => void;
  onRemoved: () => void;
}) {
  const s = useSettings();
  const saved = s.loaded?.watches.some((x) => x.path === w.path);
  const known = knownFolder(w.path);
  const name = aliasOf(w.path, w.alias);
  const icon = w.kind === "network" ? "N" : known ? (ICON[known] ?? known[0]) : (name[0] ?? "?").toUpperCase();
  const st = !saved ? { label: "● 저장 전", cls: "muted" } : state ? STATE[state] : null;
  const set = (p: Partial<WatchFolder>) => s.setWatch(i, p);
  const where = whereOf(w, common);

  const head = (
    <span aria-hidden className={known ? "wc-icon known" : "wc-icon"}>
      {icon}
    </span>
  );
  if (!open)
    return (
      <article className="watch-card" data-testid="watch-card" data-path={w.path}>
        <div className="wc-head">
          {head}
          <div className="wc-main">
            <div className="wc-title">
              <span className="alias" data-testid="watch-alias">
                {name}
              </span>
              <PathText path={w.path} max={32} />
            </div>
            <div className="wc-summary" data-testid="watch-summary-line">
              {summary(w, common)}
            </div>
          </div>
          {st && <span className={`wc-state ${st.cls}`}>{st.label}</span>}
          <button onClick={onToggle}>편집</button>
        </div>
      </article>
    );

  const remove = () => {
    if (!window.confirm(`'${name}' 폴더 감시를 그만둘까요? 폴더와 파일은 그대로 남고, 저장하면 더 이상 정리하지 않습니다.`)) return;
    s.removeWatch(i);
    onRemoved();
  };
  return (
    <article className="watch-card open" data-testid="watch-card" data-path={w.path} data-open="true">
      <div className="wc-head">
        {head}
        <div className="wc-main">
          <div className="wc-edit-title">
            <label htmlFor={`alias-${i}`} className="small muted">
              별칭
            </label>
            <input
              id={`alias-${i}`}
              className="alias-input"
              aria-label="별칭"
              placeholder={defaultAlias(w.path)}
              value={w.alias ?? ""}
              onChange={(e) => set({ alias: e.target.value || null })}
            />
            <PathText path={w.path} />
            <button className="link small" onClick={() => void ipc.openFolder(w.path).catch(() => {})}>
              폴더 열기
            </button>
          </div>
        </div>
        <button onClick={onToggle}>접기</button>
      </div>

      <div className="wc-options">
        <label>
          <input type="checkbox" checked={w.recursive} onChange={(e) => set({ recursive: e.target.checked })} /> 하위 폴더 안의
          파일도 정리 <Help id="recursive" />
        </label>
        <label>
          <input type="checkbox" checked={w.indexContent} onChange={(e) => set({ indexContent: e.target.checked })} /> 파일
          안의 글자까지 검색 (본문 인덱싱) <Help id="indexContent" />
        </label>
        <label>
          종류 <Help id="kind" />
          <select
            value={w.kind}
            aria-label="종류"
            onChange={(e) => {
              const kind = e.target.value as "local" | "network";
              // NAS default: metadata only (SPEC §5.9).
              set({ kind, indexContent: kind === "local" });
            }}
          >
            <option value="local">내 PC 폴더</option>
            <option value="network">네트워크(NAS)</option>
          </select>
        </label>
        <label>
          정리 위치 <Help id="organizeMode" />
          <select
            value={where}
            aria-label="정리 위치"
            onChange={async (e) => {
              const v = e.target.value as Where;
              if (v === "in_place") set({ mode: "in_place", organizeRoot: null });
              else if (v === "common" && common) set({ mode: "collect", organizeRoot: common });
              else if (v === "other") {
                const dir = await ipc.pickFolder(w.organizeRoot ?? common);
                if (dir) set({ mode: "collect", organizeRoot: dir });
              }
            }}
          >
            <option value="common" disabled={!common}>
              {common ? `공통 (${common})` : "공통 (정리 위치를 먼저 정하세요)"}
            </option>
            <option value="in_place">이 폴더 안에서</option>
            <option value="other">다른 폴더…</option>
          </select>
        </label>
      </div>
      {where === "other" && (
        <div className="wc-nas">
          <label>
            다른 정리 위치
            <input
              className="organize-root"
              aria-label="다른 정리 위치"
              value={w.organizeRoot ?? ""}
              onChange={(e) => set({ organizeRoot: e.target.value || null })}
            />
          </label>
          <button
            className="sm"
            onClick={async () => {
              const dir = await ipc.pickFolder(w.organizeRoot);
              if (dir) set({ organizeRoot: dir });
            }}
          >
            찾아보기…
          </button>
        </div>
      )}
      {w.kind === "network" && (
        <div className="wc-nas" data-testid="nas-options">
          <label>
            확인 간격(초) <Help id="pollInterval" />
            <input
              type="number"
              min={5}
              className="narrow"
              placeholder="120"
              value={w.pollIntervalS ?? ""}
              onChange={(e) => set({ pollIntervalS: e.target.value ? Math.max(5, Number(e.target.value)) : null })}
            />
          </label>
          <label title="변경 알림을 보내지 않는 NAS(SMB/NFS)에서 켜세요">
            <input type="checkbox" checked={w.pollOnly} onChange={(e) => set({ pollOnly: e.target.checked })} /> 주기 확인만
            사용 <Help id="pollOnly" />
          </label>
          <label title="본문 인덱싱을 켰을 때, 백그라운드 인덱싱이 본문을 읽는 시간대. 비우면 항상">
            본문 인덱싱 시간대 <Help id="contentWindow" />
            <input
              className="narrow"
              placeholder="02:00-05:00"
              disabled={!w.indexContent}
              value={w.contentIndexWindow ?? ""}
              onChange={(e) => set({ contentIndexWindow: e.target.value || null })}
            />
          </label>
          <span className="muted small">연결이 끊기면 이 폴더만 일시정지하고 60초마다 다시 확인합니다. 이동은 복사→검증 후 원본 정리.</span>
        </div>
      )}

      <div className="wc-sub" data-testid="exclude-row">
        <div className="title">
          건드리지 않을 하위 폴더 <span className="muted" style={{ fontWeight: 400 }}>(제외)</span> <Help id="exclude" />
        </div>
        <div className="chips">
          {w.exclude.length === 0 && <span className="muted small">없음 — 이 폴더 전체를 정리합니다.</span>}
          {w.exclude.map((x) => (
            <span key={x} className="chip" title={x}>
              {x.startsWith(w.path) ? x.slice(w.path.length).replace(/^[\\/]+/, "") || x : x}
              <button aria-label={`제외 해제 ${x}`} onClick={() => set({ exclude: w.exclude.filter((y) => y !== x) })}>
                ×
              </button>
            </span>
          ))}
          <button
            className="chip-add"
            onClick={async () => {
              const dir = await ipc.pickFolder(w.path);
              if (dir && !w.exclude.includes(dir)) set({ exclude: [...w.exclude, dir] });
            }}
          >
            + 추가
          </button>
        </div>
      </div>

      <div className="wc-sub">
        <button className="link danger" style={{ alignSelf: "flex-start" }} onClick={remove}>
          이 폴더 감시 그만두기
        </button>
      </div>
    </article>
  );
}
