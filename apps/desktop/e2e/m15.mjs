// M15 GUI test: text in images and scanned PDFs is searchable (SPEC
// §14.11), driven through the dev bridge. Needs tesseract (+ kor) and
// poppler-utils on the machine running the app.
//
//   A. a capture (png) and a scanned PDF already on the desktop → added as
//      a watch folder → background recognition finishes (설정 › 고급 card:
//      engine, "글자 찾음 2개")
//   B. search window: the company name inside the picture finds both files,
//      with the "사진 속 글자" badge
//   C. switch off → config.toml [ocr] enabled = false, on again
//   D. a copy dropped while watching is found by its text right away (same
//      content → cache)
//
// Run: S=/tmp/tidy-e2e bash e2e/run-headless.sh m15
import { chromium } from "playwright";
import fs from "node:fs";
import path from "node:path";
import { addWatchFolder, bridge, gotoSettings, saveSettings, sleep, waitFor } from "./lib.mjs";

const T = process.env.T;
const S = process.env.S;
const CFG = path.join(T, "config.toml");
const DESK = path.join(T, "바탕 화면");
const SHOT = path.join(S, "shots");
const FIX = path.join(import.meta.dirname, "..", "..", "..", "crates", "tidy-core", "tests", "fixtures", "ocr");
fs.mkdirSync(SHOT, { recursive: true });
fs.copyFileSync(path.join(FIX, "견적서 캡처.png"), path.join(DESK, "캡처 2026-10-28.png"));
fs.copyFileSync(path.join(FIX, "스캔 견적서.pdf"), path.join(DESK, "스캔 0001.pdf"));

const results = [];
const ok = (step, what, pass, note = "") => {
  results.push({ step, what, pass, note });
  console.log(`${pass ? "PASS" : "FAIL"} [${step}] ${what} ${note}`);
};
const searchNames = async (q) =>
  (await bridge("search", { query: q, limit: 20, customer: null })).map((h) => path.basename(h.path));

const browser = await chromium.launch();
const ctx = await browser.newContext({ viewport: { width: 1280, height: 900 } });
const main = await ctx.newPage();
main.on("dialog", (d) => {
  if (d.type() === "prompt") void d.accept(process.env.PROMPT_ANSWER ?? "");
  else void d.accept();
});
await main.goto("http://localhost:1420/#/main");
await main.waitForSelector(".topbar");
await sleep(1000);

// A. watch folder → recognition in the background
{
  await gotoSettings(main, "watch");
  await addWatchFolder(main, DESK);
  await saveSettings(main, 3500);
  const done = await waitFor(async () => (await bridge("ocr_status")).ok >= 2, 90000, 1500);
  const st = await bridge("ocr_status");
  ok("A", "백그라운드 인식: 글자 찾음 2개", done, JSON.stringify(st));
  await gotoSettings(main, "advanced");
  const card = main.locator("[data-testid=ocr-card]");
  await sleep(500);
  const text = await card.innerText();
  ok(
    "A",
    "설정 › 고급 카드: 제목·스위치 켜짐·읽는 도구·글자 찾음",
    text.includes("사진·스캔 PDF 속 글자 검색") &&
      (await card.locator("[data-testid=ocr-enabled]").isChecked()) &&
      text.includes("읽는 도구: tesseract") &&
      text.includes("글자 찾음 2개"),
    text.replace(/\n/g, " | "),
  );
  await card.screenshot({ path: `${SHOT}/m15-A-card.png` });
}

// B. search window
{
  const s = await ctx.newPage();
  await s.goto("http://localhost:1420/#/search");
  await s.waitForSelector(".search-input");
  await s.locator(".search-input").fill("가람정보");
  await sleep(1500);
  const rows = await s.locator(".results li").allInnerTexts();
  ok(
    "B",
    "'가람정보'(그림 속 글자) → 캡처·스캔 PDF 둘 다",
    rows.some((r) => r.includes("캡처 2026-10-28.png")) && rows.some((r) => r.includes("스캔 0001.pdf")),
    rows.join(" / ").replace(/\n/g, " "),
  );
  ok("B", "'사진 속 글자' 표시", (await s.locator("[data-testid=ocr-badge]").count()) >= 2);
  await s.screenshot({ path: `${SHOT}/m15-B-search.png` });
  await s.close();
}

// C. switch off / on
{
  const sw = main.locator("[data-testid=ocr-enabled]");
  await sw.uncheck();
  await sleep(1500);
  const off = /\[ocr\][\s\S]*enabled = false/.test(fs.readFileSync(CFG, "utf8"));
  ok("C", "끄기 → config.toml [ocr] enabled = false", off && !(await bridge("ocr_status")).enabled);
  await sw.check();
  await sleep(1500);
  ok("C", "다시 켜기 → enabled = true", (await bridge("ocr_status")).enabled && /enabled = true/.test(fs.readFileSync(CFG, "utf8")));
}

// D. a copy while watching
{
  fs.copyFileSync(path.join(FIX, "견적서 캡처.png"), path.join(DESK, "견적 사본.png"));
  const found = await waitFor(async () => (await searchNames("가람정보")).includes("견적 사본.png"), 40000, 1000);
  ok("D", "감시 중 들어온 사본 → 글자로 찾힘", found, (await searchNames("가람정보")).join(", "));
}

const failed = results.filter((r) => !r.pass);
console.log(`\n${results.length - failed.length}/${results.length} passed`);
fs.writeFileSync(path.join(S, "m15-results.json"), JSON.stringify(results, null, 2));
await browser.close();
process.exit(failed.length ? 1 : 0);
