import { useMemo, useState } from "react";
import * as ipc from "../ipc/commands";
import type { DupGroup, DupTrash, TrashResult } from "../ipc/types";
import { useWatchNames } from "../store/status";
import { formatSize, formatTime } from "./format";
import { PathText } from "./PathText";
import { watchOf } from "./paths";

/**
 * (M14, SPEC §14.9 2) 리포트 "중복 파일 (N그룹)": 그룹마다 남길 파일(라디오)과
 * "이 그룹 정리" 체크 → [미리보기](남김 / 휴지통 N개, 절약 용량) → [적용].
 * 적용 전에는 아무것도 옮기지 않는다. 휴지통 보내기는 코어 mover만 한다.
 */
export function DuplicateGroups({ groups, onApplied }: { groups: DupGroup[]; onApplied: () => void }) {
  const names = useWatchNames();
  const [keep, setKeep] = useState<Record<string, number>>({});
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const [allDefault, setAllDefault] = useState(false);
  const [preview, setPreview] = useState(false);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<TrashResult | null>(null);
  const [error, setError] = useState<string | null>(null);

  const keepOf = (g: DupGroup) => keep[g.hash] ?? g.keep;
  const plan: DupTrash[] = useMemo(
    () =>
      groups
        .filter((g) => picked.has(g.hash))
        .map((g) => {
          const k = keep[g.hash] ?? g.keep;
          return {
            hash: g.hash,
            keep: g.files[k]!.path,
            trash: g.files.filter((_, i) => i !== k).map((f) => f.path),
          };
        }),
    [groups, picked, keep],
  );
  const trashCount = plan.reduce((a, p) => a + p.trash.length, 0);
  const saved = groups.filter((g) => picked.has(g.hash)).reduce((a, g) => a + g.size * (g.files.length - 1), 0);

  const toggleAll = (on: boolean) => {
    setAllDefault(on);
    setPreview(false);
    if (on) {
      setKeep({});
      setPicked(new Set(groups.map((g) => g.hash)));
    } else setPicked(new Set());
  };
  const togglePick = (hash: string) => {
    const n = new Set(picked);
    if (n.has(hash)) n.delete(hash);
    else n.add(hash);
    setPicked(n);
    setPreview(false);
    setAllDefault(n.size === groups.length && Object.keys(keep).length === 0);
  };
  const choose = (g: DupGroup, i: number) => {
    setKeep({ ...keep, [g.hash]: i });
    if (!picked.has(g.hash)) setPicked(new Set([...picked, g.hash]));
    setPreview(false);
    if (i !== g.keep) setAllDefault(false);
  };
  const apply = async () => {
    setBusy(true);
    setError(null);
    try {
      const r = await ipc.trashDuplicates(plan);
      setResult(r);
      setPreview(false);
      setPicked(new Set());
      setKeep({});
      setAllDefault(false);
      onApplied();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };
  const label = (path: string) => {
    const w = watchOf(path, names);
    return <PathText path={path} alias={w?.alias} root={w?.path} />;
  };

  return (
    <div className="dup-groups" data-testid="dup-groups">
      {result && (
        <div className="notice" data-testid="dup-result">
          {result.trashed}개를 휴지통으로 보냈습니다 · {formatSize(result.bytes)} 절약. 기록은 되돌리기 탭에 있습니다(휴지통에서 복원).
          {result.failed.length > 0 && (
            <div className="small warn">
              남겨 둔 파일 {result.failed.length}개: {result.failed.join("; ")}
            </div>
          )}
        </div>
      )}
      {error && <div className="notice warn error">{error}</div>}
      {groups.length === 0 && <p className="muted">없음</p>}
      {groups.length > 0 && (
        <div className="toolbar dup-toolbar">
          <label className="check">
            <input
              type="checkbox"
              data-testid="dup-all"
              checked={allDefault}
              onChange={(e) => toggleAll(e.target.checked)}
            />{" "}
            모든 그룹에 기본 선택 적용
          </label>
          <span className="muted small">기본: 정리 위치 안 파일 &gt; 가장 오래된 것 &gt; 짧은 경로를 남깁니다</span>
          <span className="spacer" />
          <button data-testid="dup-preview" disabled={!plan.length || busy} onClick={() => setPreview(true)}>
            미리보기
          </button>
        </div>
      )}

      {preview && (
        <div className="card dup-preview" data-testid="dup-preview-table">
          <b>
            휴지통으로 {trashCount}개 · 용량 절약 {formatSize(saved)}
          </b>
          <div className="table">
            <div className="t-head cols-dup">
              <span>남길 파일</span>
              <span>휴지통으로</span>
            </div>
            {plan.map((p) => (
              <div className="t-row cols-dup" key={p.hash} data-testid="dup-preview-row">
                <div className="cell">{label(p.keep)}</div>
                <div className="cell">
                  {p.trash.map((t) => (
                    <div key={t}>{label(t)}</div>
                  ))}
                </div>
              </div>
            ))}
          </div>
          <div className="row">
            <button className="primary" data-testid="dup-apply" disabled={busy} onClick={() => void apply()}>
              {busy ? "보내는 중…" : `적용 (휴지통으로 ${trashCount}개)`}
            </button>
            <button onClick={() => setPreview(false)}>취소</button>
            <span className="muted small">적용 전에는 아무것도 옮기지 않습니다. 휴지통으로 보낸 파일은 휴지통에서 복원합니다.</span>
          </div>
        </div>
      )}

      {groups.map((g, gi) => (
        <div key={g.hash} className="card dup-group" data-testid="dup-group">
          <div className="dup-head">
            <label className="check">
              <input
                type="checkbox"
                data-testid="dup-pick"
                aria-label={`그룹 ${gi + 1} 정리`}
                checked={picked.has(g.hash)}
                onChange={() => togglePick(g.hash)}
              />{" "}
              이 그룹 정리
            </label>
            <span className="muted small">
              {formatSize(g.size)} × {g.files.length} · 낭비 {formatSize(g.wasted)}
            </span>
          </div>
          {g.files.map((f, i) => (
            <label key={f.path} className="dup-file" data-testid="dup-file">
              <input
                type="radio"
                name={`keep-${g.hash}`}
                aria-label="남길 파일"
                data-testid="dup-keep"
                checked={keepOf(g) === i}
                onChange={() => choose(g, i)}
              />
              <span className="dup-path">{label(f.path)}</span>
              {f.in_organize_root && <span className="tag">정리 위치</span>}
              <span className="muted small nowrap">{f.modified ? formatTime(f.modified) : ""}</span>
              <span className="small nowrap">{keepOf(g) === i ? "남김" : picked.has(g.hash) ? "휴지통" : ""}</span>
            </label>
          ))}
        </div>
      ))}
    </div>
  );
}
