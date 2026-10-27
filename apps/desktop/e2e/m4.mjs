// M4 GUI test (PR #5): AI settings section, offline fallback (rule 4), and the
// learn loop (3 same decisions → "이 규칙을 자동화할까요?" → 규칙 추가).
// Run after e2e/m3.mjs-style setup: app on the dev bridge, Vite on :1420,
// T = throw-away config dir with "바탕 화면" already watched is NOT required —
// this script configures the watch itself.
import { chromium } from "playwright";
import fs from "node:fs";
import path from "node:path";

const T = process.env.T;
const DESK = path.join(T, "바탕 화면");
const SHOT = path.join(process.env.S, "shots");
fs.mkdirSync(SHOT, { recursive: true });
const results = [];
const ok = (step, what, pass, note = "") => {
  results.push({ step, what, pass, note });
  console.log(`${pass ? "PASS" : "FAIL"} [${step}] ${what} ${note}`);
};
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
import { addWatchFolder, gotoSettings, openToml, saveSettings, statusText, tab } from "./lib.mjs";
const bridge = async (cmd, args = {}) =>
  (await (await fetch(`http://127.0.0.1:1421/invoke/${cmd}`, { method: "POST", body: JSON.stringify(args) })).json()).value;

const browser = await chromium.launch();
const ctx = await browser.newContext({ viewport: { width: 960, height: 800 } });
const main = await ctx.newPage();
main.on("dialog", (d) => d.accept(process.env.PROMPT_ANSWER ?? ""));
await main.goto("http://localhost:1420/#/main");
await main.waitForSelector(".topbar");
await sleep(1000);

// A. watch folder + rules via settings (same as M3 step 3)
{
  await gotoSettings(main, "watch");
  await addWatchFolder(main, DESK);
  const ta = await openToml(main);
  await ta.fill(
    `[[rule]]\nname = "설치 확인"\nmatch = { ext = ["exe"] }\ndest = '${T}/설치 파일'\nconfirm = true\n\n` +
      `[[rule]]\nname = "Tidy 테스트"\nmatch = { ext = ["txt"], name_regex = "^tidy-test" }\ndest = "~/Documents/Tidy/테스트 결과/{yyyy}/{mm}"\n\n` +
      (await ta.inputValue()),
  );
  await saveSettings(main);
  const bar = await statusText(main);
  ok("A", "감시 설정 저장", bar.includes("감시 1곳"), bar.replace(/\n/g, " | "));
  ok("A", "상태바 AI 표시 (꺼짐)", bar.includes("AI"), "");
}

// B. AI section: enable with an unreachable Ollama → saved, test fails gracefully
{
  await gotoSettings(main, "ai");
  const ai = main.locator("[data-testid=ai-section]");
  ok("B", "설정 → AI 연결 화면 표시", (await ai.count()) === 1 && main.url().endsWith("#/main/settings/ai"), main.url());
  await ai.locator("label.row", { hasText: "AI 사용" }).locator("input[type=checkbox]").check();
  // Issue #8: unsaved AI edits survive a tab round-trip (dialog: stay / leave) …
  // (the page-level dialog handler accepts the "leave without saving?" confirm)
  await tab(main, "홈").click();
  await sleep(300);
  await tab(main, "설정").click();
  await sleep(800);
  const kept = await ai.locator("label.row", { hasText: "AI 사용" }).locator("input[type=checkbox]").isChecked();
  ok("B", "#8 저장 전 탭 이동해도 AI 입력 유지", kept);
  const hint = await main.locator("[data-testid=savebar]").innerText();
  ok("B", "#8 저장바에 '저장하지 않은 변경' 표시", hint.includes("저장하지 않은"), hint);
  await ai.locator("select").first().selectOption("ollama");
  await sleep(300);
  const base = ai.locator("label.row", { hasText: "주소" }).locator("input");
  if (await base.count()) await base.fill("http://127.0.0.1:9");
  // Issue #8: the single bottom "저장" saves the AI section too.
  await saveSettings(main);
  const msg = await ai.locator("p.ok").innerText().catch(() => "");
  const err = await ai.locator("p.error").innerText().catch(() => "");
  ok("B", "AI 설정 저장 메시지", msg.length > 0 && !err, `msg=${msg} err=${err}`);
  const cfg = fs.readFileSync(path.join(T, "config.toml"), "utf8");
  ok("B", "config.toml [ai] 기록, 키 문자열 없음", /\[ai/.test(cfg) && !/sk-|api_key *= *"[^"]+"/.test(cfg), cfg.match(/\[ai[^\n]*/)?.[0] ?? "");
  await main.reload();
  await main.waitForSelector(".topbar");
  await sleep(1000);
  ok("B", "새로고침해도 설정 → AI 연결 화면 그대로 (#/main/settings/ai)", (await ai.count()) === 1);
  const persisted = await main.locator("label.row", { hasText: "AI 사용" }).locator("input[type=checkbox]").isChecked();
  const prov = await main.locator("[data-testid=ai-section]").locator("select").first().inputValue();
  ok("B", "#8 하단 저장 → 재진입 후 AI 설정 유지", persisted && prov === "ollama", `enabled=${persisted} provider=${prov}`);
  await ai.getByRole("button", { name: "연결 테스트" }).click();
  await sleep(6000);
  const err2 = await ai.locator("p.error").innerText().catch(() => "");
  const msg2 = await ai.locator("p.ok").innerText().catch(() => "");
  ok("B", "연결 테스트 실패가 오류로 표시 (앱은 계속 동작)", err2.length > 0 || /실패|오류|연결/.test(msg2), `err=${err2.slice(0, 80)} msg=${msg2.slice(0, 80)}`);
  await main.screenshot({ path: `${SHOT}/m4-B-ai-settings.png`, fullPage: true });
  const st = await bridge("get_status");
  // "ready" only means the provider was built from config; per-call failures
  // fall back to rules (step C) — by design (SPEC §5.4), not connectivity.
  ok("B", "status.aiState 는 ready|unavailable", st.aiState === "ready" || st.aiState === "unavailable", `aiState=${st.aiState} detail=${st.aiDetail}`);
}

// C. Rule 4: with AI enabled but unreachable, rules still move files, and a
//    no-rule file is reported (no hang) within a few seconds.
{
  await tab(main, "홈").click();
  await main.getByRole("button", { name: "전체 보기" }).click();
  await main.getByRole("checkbox", { name: /규칙 없는 파일 숨기기/ }).uncheck();
  fs.writeFileSync(path.join(DESK, "tidy-test ai폴백.txt"), "AI 없이도 규칙으로");
  fs.writeFileSync(path.join(DESK, "무규칙.unknownext"), "no rule");
  await sleep(8000);
  const feed = await main.locator("[data-testid=activity-feed]").innerText();
  ok("C", "규칙 매치 파일은 AI와 무관하게 이동", /이동[\s\S]*ai폴백/.test(feed), "");
  ok("C", "규칙 없는 파일: 규칙 없음/확인 대기로 기록 (AI 실패 폴백)", /무규칙\.unknownext/.test(feed) && /(규칙 없음|확인 대기|오류)/.test(feed), feed.split("\n").filter((l) => l.includes("무규칙")).join(" | ").slice(0, 160));
  ok("C", "규칙 없는 파일은 제자리", fs.existsSync(path.join(DESK, "무규칙.unknownext")));
  await main.screenshot({ path: `${SHOT}/m4-C-fallback.png` });
}

// D. Learn loop: 3× "다른 곳…" with the same name prefix + ext to the same folder
{
  const c = await ctx.newPage();
  c.on("dialog", (d) => d.accept(process.env.PROMPT_ANSWER ?? ""));
  await c.goto("http://localhost:1420/#/confirm");
  await sleep(500);
  process.env.PROMPT_ANSWER = path.join(T, "과금");
  for (const n of [1, 2, 3]) {
    fs.writeFileSync(path.join(DESK, `KT과금내역서_2026-0${n}.exe`), "");
    await sleep(4500);
    await c.getByRole("button", { name: "다른 곳…", exact: true }).click();
    await sleep(2000);
  }
  const moved = [1, 2, 3].filter((n) => fs.existsSync(path.join(T, "과금", `KT과금내역서_2026-0${n}.exe`))).length;
  ok("D", "같은 폴더로 3회 이동", moved === 3, `moved=${moved}`);
  await c.close();

  await tab(main, "확인 대기").click();
  await sleep(1500);
  const card = main.locator(".learn-card").first();
  const shown = (await card.count()) > 0;
  const txt = shown ? await card.innerText() : "";
  ok("D", "학습 제안 카드 표시", shown && txt.includes("3회") && txt.includes("과금"), txt.replace(/\n/g, " | ").slice(0, 160));
  await main.screenshot({ path: `${SHOT}/m4-D-learn.png` });
  if (shown) {
    await card.getByRole("button", { name: "규칙 추가" }).click();
    await sleep(2500);
    const rules = fs.readFileSync(path.join(T, "rules.toml"), "utf8");
    const top = rules.split("[[rule]]")[1] ?? "";
    ok("D", "규칙 추가 → rules.toml 맨 위에 학습 규칙", top.includes("과금") && top.includes("KT과금내역서"), top.trim().split("\n").slice(0, 4).join(" | "));
    // the new rule now applies automatically
    fs.writeFileSync(path.join(DESK, "KT과금내역서_2026-04.exe"), "");
    await sleep(5000);
    const auto = fs.existsSync(path.join(T, "과금", "KT과금내역서_2026-04.exe"));
    const stillPending = (await bridge("get_status")).pendingCount;
    ok("D", "학습된 규칙으로 4번째 파일 처리 (자동 이동 또는 확인 대기)", auto || stillPending > 0, `auto=${auto} pending=${stillPending}`);
    ok("D", "제안 카드 사라짐", (await main.locator(".learn-card").count()) === 0);
  }
}

fs.writeFileSync(path.join(process.env.S, "m4-results.json"), JSON.stringify(results, null, 2));
await browser.close();
const fails = results.filter((r) => !r.pass).length;
console.log(`\n${results.length - fails}/${results.length} passed`);
