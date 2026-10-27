import { useState } from "react";
import { MoreMenu, type MoreItem } from "./MoreMenu";
import * as ipc from "../ipc/commands";
import { useHome } from "../store/home";

export const REPO = "https://github.com/sjh714252-prog/tidy";
export const MANUAL_URL = `${REPO}/blob/main/docs/manual.md`;
/** (M12) docs/manual.md "### 고객사 자료는 어디 있나요?" (GitHub heading anchor). */
export const CUSTOMER_HELP_URL = `${MANUAL_URL}#고객사-자료는-어디-있나요`;

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

/** 도움말 items (SPEC §14.6), shared by "?" and the narrow-window ≡ menu. */
export function helpItems(openAbout: (check?: boolean) => void, onErr: (e: string) => void): MoreItem[] {
  const go = (f: () => Promise<void> | void) => () => {
    Promise.resolve()
      .then(f)
      .catch((e) => onErr(String(e)));
  };
  return [
    { label: "사용 설명서", onClick: go(() => ipc.openUrl(MANUAL_URL)) },
    { label: "고객사 자료는 어디 있나요?", onClick: go(() => ipc.openUrl(CUSTOMER_HELP_URL)) },
    { label: "문의·버그 신고", onClick: go(async () => ipc.openUrl(await issueUrl())) },
    { label: "업데이트 확인", onClick: go(() => openAbout(true)) },
    { label: "정보", onClick: go(() => openAbout(false)) },
  ];
}

/** Top bar "?" (도움말): 사용 설명서 / 문의·버그 신고 / 업데이트 확인 / 정보. */
export function HelpMenu() {
  const [err, setErr] = useState<string | null>(null);
  const openAbout = useHome((s) => s.openAbout);
  return (
    <>
      <MoreMenu label="도움말" testid="help-menu" items={helpItems(openAbout, setErr)}>
        ?
      </MoreMenu>
      {err && (
        <span className="error small" title={err}>
          열지 못했습니다
        </span>
      )}
    </>
  );
}
