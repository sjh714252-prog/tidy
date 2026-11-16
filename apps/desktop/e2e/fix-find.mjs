// 0.18.0 (SPEC §14.24): 찾기·되돌리기 tab — find a file Tidy moved by part of
// its name and open it (or its folder) where it is now; put one file back.
// Fixtures: fix-find.setup.mjs. 열기·폴더 열기 are recorded (TIDY_E2E_NO_OPEN).
//
// Run: S=/tmp/tidy-e2e bash e2e/run-headless.sh fix-find
import { chromium } from "playwright";
import fs from "node:fs";
import path from "node:path";
import { bridge, sleep, tab as tabOf, waitFor } from "./lib.mjs";

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
const opened = async () => (await bridge("debug_opened")) ?? [];

const browser = await chromium.launch();
const ctx = await browser.newContext({ viewport: { width: 1280, height: 860 } });
const main = await ctx.newPage();
main.on("dialog", (d) => void d.accept());
await main.goto("http://localhost:1420/#/main");
await main.waitForSelector(".topbar");
await sleep(1500);
const tab = (name) => tabOf(main, name);

// A. two files arrive on the desktop → the rule moves them to 정리/문서
const QUOTE = path.join(ORG, "문서", "가람정보 견적서.pdf");
const MEMO = path.join(ORG, "문서", "회의 메모.txt");
{
  fs.writeFileSync(path.join(DESK, "가람정보 견적서.pdf"), "%PDF-1.4 견적");
  fs.writeFileSync(path.join(DESK, "회의 메모.txt"), "가람정보 회의 메모");
  const moved = await waitFor(() => fs.existsSync(QUOTE) && fs.existsSync(MEMO), 30000);
  ok("A", "규칙이 두 파일을 정리/문서로 옮김", moved);
}

// B. tab renamed; empty box = 최근 정리한 파일
const box = main.locator("[data-testid=find-files]");
const rows = box.locator("[data-testid=find-row]");
{
  ok("B", "탭 이름 '찾기·되돌리기'", (await main.locator("[data-testid=tab-history]").innerText()).trim() === "찾기·되돌리기");
  await tab("찾기·되돌리기").click();
  await waitFor(async () => (await rows.count()) === 2, 10000);
  const t = await box.innerText();
  ok("B", "빈 칸: 최근 정리한 파일 2개", t.includes("최근 정리한 파일") && (await rows.count()) === 2, t.replace(/\n/g, " | "));
  ok("B", "지금 위치 = 정리/문서, 원래 = 바탕 화면", t.includes(path.join(ORG, "문서")) && t.includes(DESK));
  await main.screenshot({ path: `${SHOT}/fix-find-B-recent.png` });
}

// C. type part of a name → the moved file; 열기 / 폴더 열기 / Enter
{
  await box.locator("[data-testid=find-input]").fill("견적");
  await waitFor(async () => (await rows.count()) === 1 && (await box.innerText()).includes("1개 찾음"), 10000);
  const t = await rows.first().innerText();
  ok("C", "'견적' → 가람정보 견적서.pdf 하나", (await rows.count()) === 1 && t.includes("가람정보 견적서.pdf"), t.replace(/\n/g, " | "));
  await rows.first().getByRole("button", { name: "열기", exact: true }).click();
  await sleep(400);
  ok("C", "열기 → 지금 위치의 파일", (await opened()).at(-1) === `open:${QUOTE}`, (await opened()).at(-1));
  await rows.first().getByRole("button", { name: "폴더 열기", exact: true }).click();
  await sleep(400);
  ok("C", "폴더 열기 → 탐색기에서 그 파일 선택", (await opened()).at(-1) === `reveal:${QUOTE}`, (await opened()).at(-1));
  await box.locator("[data-testid=find-input]").press("Control+Enter");
  await sleep(400);
  const n = (await opened()).length;
  await box.locator("[data-testid=find-input]").press("Enter");
  await sleep(400);
  ok("C", "입력 칸에서 Enter = 열기", (await opened()).length === n + 1 && (await opened()).at(-1) === `open:${QUOTE}`);
  await main.screenshot({ path: `${SHOT}/fix-find-C-found.png` });
}

// D. a folder name finds what is in it; nothing found says so
{
  await box.locator("[data-testid=find-input]").fill("문서");
  await waitFor(async () => (await rows.count()) === 2, 10000);
  ok("D", "폴더 이름 '문서' → 그 안의 두 파일", (await rows.count()) === 2);
  await box.locator("[data-testid=find-input]").fill("없는낱말xyz");
  await waitFor(async () => (await box.innerText()).includes("찾은 파일이 없습니다"), 10000);
  ok("D", "없으면 '찾은 파일이 없습니다'", (await rows.count()) === 0);
}

// E. 원래 자리로 (one file) → back on the desktop, row says so, history too
{
  await box.locator("[data-testid=find-input]").fill("메모");
  await waitFor(async () => (await rows.count()) >= 1, 10000);
  await rows.first().getByRole("button", { name: "원래 자리로", exact: true }).click();
  const back = await waitFor(() => fs.existsSync(path.join(DESK, "회의 메모.txt")) && !fs.existsSync(MEMO), 10000);
  ok("E", "원래 자리로 → 바탕 화면으로 돌아감", back);
  await waitFor(async () => (await rows.first().innerText()).includes("원래 자리로 되돌림"), 5000);
  const t = await rows.first().innerText();
  ok("E", "줄에 '원래 자리로 되돌림' + 지금 위치 = 바탕 화면", t.includes("원래 자리로 되돌림") && t.includes(DESK), t.replace(/\n/g, " | "));
  await sleep(3000);
  ok("E", "되돌린 파일은 다시 옮기지 않음", fs.existsSync(path.join(DESK, "회의 메모.txt")));
}

// F. 이동 기록 › 파일 보기: 열기·폴더 열기 at where the file is now
{
  // Each file came as its own batch: open the one whose example is the quote.
  const row = main.locator("[data-testid=batch-row]").filter({ hasText: "가람정보 견적서.pdf" }).first();
  await row.getByRole("button", { name: "파일 보기" }).click();
  const mv = main.locator("[data-testid=move-row]");
  await waitFor(async () => (await mv.count()) >= 1, 5000);
  const quoteRow = mv.filter({ hasText: "가람정보 견적서.pdf" });
  ok("F", "파일 보기 줄에 열기·폴더 열기", (await quoteRow.getByRole("button", { name: "폴더 열기" }).count()) === 1);
  await quoteRow.getByRole("button", { name: "폴더 열기" }).click();
  await sleep(400);
  ok("F", "기록의 폴더 열기 → 지금 위치", (await opened()).at(-1) === `reveal:${QUOTE}`, (await opened()).at(-1));
}

// G. 960×640: no sideways scroll
{
  await main.setViewportSize({ width: 960, height: 640 });
  await box.locator("[data-testid=find-input]").fill("");
  await sleep(800);
  const noH = await main.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1);
  ok("G", "960×640: 가로 스크롤 없음", noH);
  await main.screenshot({ path: `${SHOT}/fix-find-G-narrow.png` });
}

await browser.close();
const failed = results.filter((r) => !r.pass);
console.log(`\n${results.length - failed.length}/${results.length} passed`);
process.exit(failed.length ? 1 : 0);
