// 0.18.1: the search window must not wait for the NAS or for a long apply.
//   A. no NAS set up → the NAS is never asked ("NAS 찾는 중…" never shows)
//   B. while a long job holds the app lock (a plan being applied), search
//      still answers (it reads through its own connection)
//   C. a slow search shows [중단]; clicking it stops waiting; Esc does too
//   D. ✕ closes the window (stops whatever is running)
// Fixtures: fix-search.setup.mjs.
//
// Run: S=/tmp/tidy-e2e bash e2e/run-headless.sh fix-search
import { chromium } from "playwright";
import fs from "node:fs";
import path from "node:path";
import { bridge, sleep, waitFor } from "./lib.mjs";

const T = process.env.T;
const S = process.env.S;
const DESK = path.join(T, "바탕 화면");
const ORG = path.join(T, "정리");
const SHOT = path.join(S, "shots");
fs.mkdirSync(SHOT, { recursive: true });

const results = [];
const ok = (step, what, pass, note = "") => {
  results.push({ step, what, pass, note });
  console.log(`${pass ? "PASS" : "FAIL"} [${step}] ${what} ${note}`);
};

const browser = await chromium.launch();
const ctx = await browser.newContext({ viewport: { width: 900, height: 560 } });

// A file to find: the rule moves it to 정리/문서 and indexes it.
const QUOTE = path.join(ORG, "문서", "가람정보 AI 활용 견적서.txt");
fs.writeFileSync(path.join(DESK, "가람정보 AI 활용 견적서.txt"), "가람정보 AI 활용 방안 견적");
ok("0", "파일이 정리/문서로 옮겨짐", await waitFor(() => fs.existsSync(QUOTE), 30000));
await waitFor(async () => (await bridge("search", { query: "견적서", limit: 5 })).length > 0, 20000);

const s = await ctx.newPage();
const nasCalls = [];
s.on("request", (r) => {
  if (r.url().includes("/invoke/nas_search")) nasCalls.push(r.url());
});
await s.goto("http://localhost:1420/#/search");
await s.waitForSelector(".search-input");
await sleep(800);
const input = s.locator(".search-input");
const foot = s.locator(".search-foot");
const rows = s.locator(".results li[data-i]");

// A. no NAS configured
{
  ok("A", "설정된 NAS 없음", (await bridge("nas_names")).length === 0);
  let sawNas = false;
  await input.fill("견적서");
  const end = Date.now() + 2500;
  while (Date.now() < end) {
    if ((await foot.innerText()).includes("NAS")) sawNas = true;
    await sleep(100);
  }
  ok("A", "결과가 나옴", (await rows.count()) > 0);
  ok("A", "'NAS 찾는 중…' 안 보임", !sawNas, (await foot.innerText()).replace(/\n/g, " "));
  ok("A", "NAS에 묻지 않음", nasCalls.length === 0, `${nasCalls.length}번`);
}

// B. a long job holds the app lock (stands in for applying a big plan)
{
  await input.fill("");
  await bridge("debug_hold_lock", { which: "app", ms: 8000 });
  await sleep(300);
  const t0 = Date.now();
  await input.fill("AI 활용");
  const found = await waitFor(async () => (await rows.count()) > 0, 5000, 100);
  ok("B", "앱이 바쁜 동안에도 검색 결과가 바로 나옴", found, `${Date.now() - t0}ms`);
  await s.screenshot({ path: path.join(SHOT, "fix-search-B.png") });
  await sleep(8000);
}

// C. a slow search: [중단], then Esc
{
  await input.fill("");
  await bridge("debug_hold_lock", { which: "search", ms: 6000 });
  await sleep(200);
  await input.fill("견적");
  const stop = s.locator("[data-testid=search-stop]");
  ok("C", "검색 중에 [중단] 버튼", await waitFor(async () => (await stop.count()) > 0, 3000, 100));
  await s.screenshot({ path: path.join(SHOT, "fix-search-C1.png") });
  await stop.click();
  await sleep(200);
  const txt = (await foot.innerText()).replace(/\n/g, " ");
  ok("C", "중단 뒤 '검색 중…' 사라짐", !txt.includes("검색 중…") && txt.includes("검색 멈춤"), txt);
  ok("C", "멈춘 안내", (await s.locator("[data-testid=search-empty]").innerText()).includes("멈췄"));
  await sleep(6500);
  ok("C", "늦게 온 답은 버림", (await rows.count()) === 0);
  await s.screenshot({ path: path.join(SHOT, "fix-search-C2.png") });

  // Esc stops a running search (does not clear the query)
  await bridge("debug_hold_lock", { which: "search", ms: 4000 });
  await sleep(200);
  await input.fill("견적서");
  await waitFor(async () => (await stop.count()) > 0, 3000, 100);
  await input.press("Escape");
  await sleep(200);
  ok("C", "Esc = 검색 중단 (검색어는 그대로)", (await stop.count()) === 0 && (await input.inputValue()) === "견적서");
  await sleep(4500);
  // Typing again searches again
  await input.fill("견적서 ");
  ok("C", "검색어를 고치면 다시 찾음", await waitFor(async () => (await rows.count()) > 0, 5000, 100));
}

// D. ✕ closes the window
{
  const close = s.locator("[data-testid=search-close]");
  ok("D", "닫기 ✕ 버튼 보임", await close.isVisible());
  await s.screenshot({ path: path.join(SHOT, "fix-search-D.png") });
  await close.click();
  await sleep(300);
  const hidden = await s.evaluate(() => window.__tidyHidden ?? 0);
  ok("D", "✕ 누르면 창 숨김", hidden > 0, `${hidden}번`);
}

await browser.close();
const failed = results.filter((r) => !r.pass);
console.log(`\n${results.length - failed.length}/${results.length} passed`);
process.exit(failed.length ? 1 : 0);
