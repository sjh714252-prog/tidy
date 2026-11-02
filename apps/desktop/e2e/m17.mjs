// M17 GUI test: natural-language commands on the home tab (SPEC §14.13),
// driven through the dev bridge. No AI configured → the offline parser.
//
//   A. "지난달 받은 견적서 관련 엑셀 전부 프로젝트 폴더로" → 미리보기: chips
//      (옮기기 · 기간 · 종류 엑셀 · 단어 견적서 · 옮길 곳 …/프로젝트), "AI 없이
//      해석함", the two 견적서 spreadsheets of last month, nothing moved yet
//   B. untick one → [1개 옮기기] → only that one moved; 되돌리기 tab has
//      the "명령 #N" batch
//   C. [되돌리기] → back on the desktop
//   D. a find command lists without a move button; a delete command is refused
//
// Run: S=/tmp/tidy-e2e bash e2e/run-headless.sh m17
import { chromium } from "playwright";
import fs from "node:fs";
import path from "node:path";
import { addWatchFolder, bridge, gotoSettings, saveSettings, sleep, tab, waitFor } from "./lib.mjs";

const T = process.env.T;
const S = process.env.S;
const DESK = path.join(T, "바탕 화면");
const PROJ = path.join(DESK, "업무", "프로젝트");
const SHOT = path.join(S, "shots");
fs.mkdirSync(SHOT, { recursive: true });
fs.mkdirSync(PROJ, { recursive: true });

// Last month, mid-month noon (local time).
const now = new Date();
const lastMonth = new Date(now.getFullYear(), now.getMonth() - 1, 15, 12);
const older = new Date(now.getFullYear(), now.getMonth() - 3, 15, 12);
for (const [name, body, when] of [
  ["견적서 9월분.xlsx", "견적 합계", lastMonth],
  ["견적서 단가표.xlsx", "견적 단가", lastMonth],
  ["견적서 계약.pdf", "견적 계약", lastMonth],
  ["견적서 예전.xlsx", "견적 합계", older],
  ["정산 내역.xlsx", "정산", lastMonth],
]) {
  const p = path.join(DESK, name);
  fs.writeFileSync(p, body);
  fs.utimesSync(p, when, when);
}

const results = [];
const ok = (step, what, pass, note = "") => {
  results.push({ step, what, pass, note });
  console.log(`${pass ? "PASS" : "FAIL"} [${step}] ${what} ${note}`);
};

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

await gotoSettings(main, "watch");
await addWatchFolder(main, DESK);
await saveSettings(main, 3500);
const indexed = await waitFor(
  async () => (await bridge("search", { query: "견적서", limit: 20, customer: null })).length >= 4,
  60000,
  1000,
);
ok("-", "감시 폴더 색인", indexed);

const card = main.locator("[data-testid=command-card]");
const ask = async (text) => {
  await tab(main, "홈").click();
  await sleep(300);
  await card.locator("[data-testid=command-input]").fill(text);
  await card.locator("[data-testid=command-run]").click();
  await waitFor(async () => (await card.locator("[data-testid=command-result], [data-testid=command-error]").count()) > 0, 20000, 300);
  await sleep(300);
};
const fileNames = async () =>
  (await card.locator("[data-testid=command-file] .name").allInnerTexts()).map((s) => s.trim()).sort();

// A
{
  await ask("지난달 받은 견적서 관련 엑셀 전부 프로젝트 폴더로");
  const chips = await card.locator("[data-testid=command-understood]").innerText();
  ok(
    "A",
    "해석 칩: 옮기기·기간·종류 엑셀·단어 견적서·옮길 곳 프로젝트",
    chips.includes("옮기기") && chips.includes("기간 ") && chips.includes("종류 엑셀") && chips.includes("단어 견적서") && chips.includes("프로젝트"),
    chips.replace(/\n/g, " | "),
  );
  ok("A", "AI 없이 해석함 표시", (await card.locator("[data-testid=command-offline]").count()) === 1);
  const names = await fileNames();
  ok("A", "지난달 견적서 엑셀 2개만", JSON.stringify(names) === JSON.stringify(["견적서 9월분.xlsx", "견적서 단가표.xlsx"]), names.join(", "));
  ok("A", "아직 아무것도 안 옮김", fs.existsSync(path.join(DESK, "견적서 단가표.xlsx")) && fs.readdirSync(PROJ).length === 0);
  ok("A", "[2개 옮기기] 버튼", (await card.locator("[data-testid=command-apply]").innerText()).includes("2개 옮기기"));
  await card.screenshot({ path: `${SHOT}/m17-A-preview.png` });
}

// B
let batchCount = 0;
{
  const rows = card.locator("[data-testid=command-file]");
  const n = await rows.count();
  for (let i = 0; i < n; i++) {
    if ((await rows.nth(i).innerText()).includes("견적서 단가표.xlsx")) await rows.nth(i).locator("input[type=checkbox]").uncheck();
  }
  await sleep(200);
  ok("B", "하나 빼면 [1개 옮기기]", (await card.locator("[data-testid=command-apply]").innerText()).includes("1개 옮기기"));
  await card.locator("[data-testid=command-apply]").click();
  await waitFor(async () => (await card.locator("[data-testid=command-applied]").count()) > 0, 15000, 300);
  const msg = await card.locator("[data-testid=command-applied]").innerText();
  ok(
    "B",
    "고른 것만 옮김",
    msg.includes("1개를 옮겼습니다") && fs.existsSync(path.join(PROJ, "견적서 9월분.xlsx")) && fs.existsSync(path.join(DESK, "견적서 단가표.xlsx")),
    msg,
  );
  await card.screenshot({ path: `${SHOT}/m17-B-applied.png` });
  const batches = await bridge("list_batches", { limit: 10 });
  batchCount = batches.length;
  ok("B", "되돌리기 기록: 명령 #N 배치", batches.some((b) => (b.label ?? "").startsWith("명령 #")), JSON.stringify(batches.map((b) => b.label)));
}

// C
{
  await card.locator("[data-testid=command-undo]").click();
  await waitFor(async () => (await card.locator("[data-testid=command-undone]").count()) > 0, 15000, 300);
  const msg = await card.locator("[data-testid=command-undone]").innerText();
  ok("C", "되돌리기 → 원래 자리", msg.includes("1개를 원래 자리로") && fs.existsSync(path.join(DESK, "견적서 9월분.xlsx")), msg);
  ok("C", "기록은 한 배치", batchCount >= 1);
}

// D
{
  await ask("견적서 계약 찾아줘");
  const names = await fileNames();
  ok("D", "찾기: 목록만", JSON.stringify(names) === JSON.stringify(["견적서 계약.pdf"]) && (await card.locator("[data-testid=command-apply]").count()) === 0, names.join(", "));
  await ask("지난달 견적서 엑셀 다 지워줘");
  const err = (await card.locator("[data-testid=command-error]").count()) ? await card.locator("[data-testid=command-error]").innerText() : "";
  ok("D", "지우기 명령은 거절", err.includes("옮기기와 찾기만"), err);
  await ask("옮겨줘");
  const err2 = (await card.locator("[data-testid=command-error]").count()) ? await card.locator("[data-testid=command-error]").innerText() : "";
  ok("D", "대상 없는 명령 안내", err2.includes("어떤 파일인지"), err2);
  await main.setViewportSize({ width: 960, height: 700 });
  await ask("지난달 견적서 엑셀 프로젝트로 옮겨줘");
  await card.screenshot({ path: `${SHOT}/m17-D-narrow.png` });
  ok("D", "좁은 창(960)에서도 목록", (await card.locator("[data-testid=command-file]").count()) === 2);
}

const failed = results.filter((r) => !r.pass);
console.log(`\n${results.length - failed.length}/${results.length} passed`);
fs.writeFileSync(path.join(S, "m17-results.json"), JSON.stringify(results, null, 2));
await browser.close();
process.exit(failed.length ? 1 : 0);
