import { useEffect, useRef, useState } from "react";
import * as ipc from "../ipc/commands";
import { useHome } from "../store/home";

export const REPO = "https://github.com/sjh714252-prog/tidy";
export const MANUAL_URL = `${REPO}/blob/main/docs/manual.md`;

/** "문의·버그 신고": a new issue with version / OS / config path filled in. */
export async function issueUrl(): Promise<string> {
  let env = "";
  try {
    const a = await ipc.aboutInfo();
    env = `- 버전: ${a.version} (빌드 ${a.gitSha})\n- OS: ${a.os}\n- 설정 파일: ${a.configPath}\n`;
  } catch {
    env = "- 버전: (알 수 없음)\n";
  }
  const body =
    "## 무엇이 문제인가요? / 문의 내용\n\n(여기에 적어 주세요. 파일 내용이나 API 키는 적지 마세요.)\n\n" +
    "## 어떻게 하면 다시 생기나요?\n\n1. \n\n---\n" +
    env;
  return `${REPO}/issues/new?title=${encodeURIComponent("[문의] ")}&body=${encodeURIComponent(body)}`;
}

/**
 * Top bar "도움말 ▾" (SPEC §14.6): 사용 설명서 / 문의·버그 신고 / 업데이트
 * 확인 / 정보. Links open in the default browser.
 */
export function HelpMenu() {
  const [open, setOpen] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const openAbout = useHome((s) => s.openAbout);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", close);
    return () => document.removeEventListener("mousedown", close);
  }, [open]);
  const go = async (f: () => Promise<void> | void) => {
    setOpen(false);
    setErr(null);
    try {
      await f();
    } catch (e) {
      setErr(String(e));
    }
  };
  return (
    <div className="help-menu" ref={ref}>
      <button aria-haspopup="menu" aria-expanded={open} onClick={() => setOpen((o) => !o)}>
        도움말 ▾
      </button>
      {open && (
        <div className="menu" role="menu" data-testid="help-menu">
          <button role="menuitem" onClick={() => void go(() => ipc.openUrl(MANUAL_URL))}>
            사용 설명서
          </button>
          <button role="menuitem" onClick={() => void go(async () => ipc.openUrl(await issueUrl()))}>
            문의·버그 신고
          </button>
          <button role="menuitem" onClick={() => void go(() => openAbout(true))}>
            업데이트 확인
          </button>
          <button role="menuitem" onClick={() => void go(() => openAbout(false))}>
            정보
          </button>
        </div>
      )}
      {err && (
        <span className="error small" title={err}>
          열지 못했습니다
        </span>
      )}
    </div>
  );
}
