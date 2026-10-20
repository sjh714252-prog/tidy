import { useState } from "react";
import { HELP, type HelpId } from "./helpTexts";

/**
 * ⓘ next to a hard setting (SPEC §14.4): one line, and an example when
 * opened further. Texts live in helpTexts.ts so the wizard and the settings
 * tab say the same thing.
 */
export function Help({ id }: { id: HelpId }) {
  const [open, setOpen] = useState(false);
  // Open toward the left when there is no room on the right.
  const [flip, setFlip] = useState(false);
  const h = HELP[id];
  return (
    <span className="help" data-help={id}>
      <button
        type="button"
        className="help-i"
        aria-label={`도움말: ${h.title}`}
        aria-expanded={open}
        title={h.line}
        onClick={(e) => {
          e.preventDefault();
          const r = e.currentTarget.getBoundingClientRect();
          setFlip(r.left + 340 > window.innerWidth);
          setOpen((o) => !o);
        }}
      >
        ⓘ
      </button>
      {open && (
        <span className={flip ? "help-pop flip" : "help-pop"} role="note">
          <span className="help-line">{h.line}</span>
          {h.example && (
            <details>
              <summary>예시</summary>
              <span className="help-ex">{h.example}</span>
            </details>
          )}
        </span>
      )}
    </span>
  );
}
