import { useEffect, useState } from "react";

/** Same breakpoint as the CSS (SPEC §14.8: < 900px → cards). */
export const NARROW = "(max-width: 899px)";

export function useNarrow(): boolean {
  const [narrow, setNarrow] = useState(() => window.matchMedia(NARROW).matches);
  useEffect(() => {
    const m = window.matchMedia(NARROW);
    const on = () => setNarrow(m.matches);
    m.addEventListener("change", on);
    return () => m.removeEventListener("change", on);
  }, []);
  return narrow;
}
