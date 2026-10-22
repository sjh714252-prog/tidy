import * as ipc from "../ipc/commands";
import { inMainWindow } from "../ipc/transport";
import { useNav } from "../store/nav";
import { useStatus } from "../store/status";

/** True when AI features can be used (SPEC §14.4 "AI 필요"). */
export function useAiOn(): boolean {
  const s = useStatus((st) => st.status?.aiState);
  return s === "ready" || s === "starting";
}

/**
 * "AI 필요" badge + "AI 연결에서 켜기" link for features that need the AI
 * (shown only while the AI is off or unusable).
 */
export function AiNeeded({ always = false }: { always?: boolean }) {
  const on = useAiOn();
  const detail = useStatus((st) => st.status?.aiDetail);
  const goSettings = useNav((s) => s.goSettings);
  if (on && !always) return null;
  return (
    <span className="ai-needed" data-testid="ai-needed">
      <span className="tag ai" title={detail ?? "설정 → AI 연결에서 분류용 모델을 켜야 합니다"}>
        AI 필요
      </span>
      {!on && (
        <button
          className="link"
          onClick={() => {
            if (inMainWindow()) goSettings("ai");
            else void ipc.showWindow("main:settings?section=ai");
          }}
        >
          AI 연결에서 켜기
        </button>
      )}
    </span>
  );
}
