import * as ipc from "../ipc/commands";
import type { WatchFolder } from "../ipc/types";

/** Anchor of a watch folder: where relative rule destinations / {root} go. */
export const anchorOf = (w: WatchFolder) =>
  w.mode === "collect" && w.organizeRoot ? w.organizeRoot : w.path;

/**
 * Per-folder "정리 방식" (SPEC §14.1): organize inside this folder, or
 * collect into category folders under another folder.
 */
export function OrganizeModeField({
  w,
  onChange,
}: {
  w: WatchFolder;
  onChange: (p: Partial<WatchFolder>) => void;
}) {
  const pickRoot = async () => {
    const dir = await ipc.pickFolder(w.organizeRoot ?? null);
    if (dir) onChange({ organizeRoot: dir });
  };
  return (
    <span className="organize-mode">
      <select
        value={w.mode}
        onChange={(e) => onChange({ mode: e.target.value as WatchFolder["mode"] })}
        aria-label="정리 방식"
      >
        <option value="in_place">이 폴더 안에서 정리</option>
        <option value="collect">지정 위치로 모으기</option>
      </select>
      {w.mode === "collect" && (
        <>
          <input
            className="organize-root"
            placeholder="기준 폴더 (예: D:\정리)"
            value={w.organizeRoot ?? ""}
            onChange={(e) => onChange({ organizeRoot: e.target.value || null })}
          />
          <button onClick={() => void pickRoot()}>찾아보기…</button>
        </>
      )}
    </span>
  );
}
