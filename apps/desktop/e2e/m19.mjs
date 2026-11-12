// M19 (SPEC §14.20) "정리 위치 점검": home [정리 위치 점검] → 정리하기 with
// the organize root chosen → preview (split customer / same issue merged,
// 기타\고객사 moved up, loose exe by the rules, code project untouched) →
// apply (one batch) → review again: nothing left → undo. Fixtures:
// m19.setup.mjs.
//
// Run: S=/tmp/tidy-e2e bash e2e/run-headless.sh m19
import { chromium } from "playwright";
import fs from "node:fs";
import path from "node:path";
import { sleep, tab as tabOf, waitFor } from "./lib.mjs";

const T = process.env.T;
const S = process.env.S;
const ORG = path.join(T, "정리");
const SHOT = path.join(S, "shots");
fs.mkdirSync(SHOT, { recursive: true });
const at = (rel) => fs.existsSync(path.join(ORG, rel));

const results = [];
const ok = (step, what, pass, note = "") => {
  results.push({ step, what, pass, note });
  console.log(`${pass ? "PASS" : "FAIL"} [${step}] ${what} ${note}`);
};

const browser = await chromium.launch();
const ctx = await browser.newContext({ viewport: { width: 1280, height: 860 } });
const main = await ctx.newPage();
main.on("dialog", (d) => void d.accept());
await main.goto("http://localhost:1420/#/main");
await main.waitForSelector(".topbar");
await sleep(1500);
const tab = (name) => tabOf(main, name);
const panel = main.locator(".restructure");

// A. home: one line per organize root → 정리하기 with a review preview
{
  await tab("홈").click();
  const row = main.locator("[data-testid=review-row]");
  const shown = await waitFor(async () => (await row.count()) === 1, 15000);
  ok("A", "홈: 정리 위치 점검 줄", shown && (await row.innerText()).includes("정리"), shown ? (await row.innerText()).replace(/\n/g, " | ") : "");
  await main.screenshot({ path: `${SHOT}/m19-A-home.png` });
  await row.locator("[data-testid=review-open]").first().click();
  const open = await waitFor(async () => (await panel.locator(".plan").count()) === 1, 15000);
  const txt = open ? await panel.innerText() : "";
  ok("A", "정리하기: 정리 위치 점검 · 5개 항목", txt.includes("정리 위치 점검") && txt.includes("5개 항목"), txt.replace(/\n/g, " | ").slice(0, 400));
  ok("A", "이유: 같은 이슈 폴더 합치기 · 기타 아래 고객사 폴더 · 규칙", txt.includes("같은 이슈 폴더 합치기") && txt.includes("아래에 있던 고객사 폴더") && txt.includes("맨 위 파일(규칙)"));
  ok("A", "코드 프로젝트는 제안에 없음", !txt.includes("Cargo.toml") && !txt.includes("tidy/src"));
  const sel = main.locator("[data-testid=report-folder]");
  ok("A", "정리할 폴더 = 정리 위치 점검 선택지", (await sel.inputValue()) === ORG && (await main.locator("[data-testid=report-folder-review]").count()) === 1);
  ok("A", "'정리 결과가 모이는 곳' 경고 대신 점검", (await main.locator("[data-testid=organize-root-warning]").count()) === 0);
  ok("A", "미리보기만으로 무이동", at("KT 스마트메시지/20261016_과금/c.txt") && at("설치.exe"));
  await main.screenshot({ path: `${SHOT}/m19-A-plan.png`, fullPage: true });
}

// B. apply → one folder per customer / issue, emptied folders gone
{
  await panel.getByRole("button", { name: /선택한 \d+개 옮기기/ }).click();
  const moved = await waitFor(async () => (await panel.locator(".notice", { hasText: "옮겼습니다" }).count()) === 1, 30000);
  const want = ["20261014_과금/a1.log", "20261014_과금/b.log", "20261014_과금/c.txt", "20261101_장애/d.txt"].map((f) => `KT스마트메시지/${f}`);
  ok("B", "KT스마트메시지/20261014_과금 하나로 + 20261101_장애", moved && want.every(at), want.filter((f) => !at(f)).join(","));
  ok("B", "합쳐진 폴더는 지워짐", !at("KT 스마트메시지") && !at("KT스마트메시지/20261020_과금") && !at("기타/가람정보"));
  ok("B", "기타/가람정보/메모.txt → 가람정보/메모.txt, 설치.exe → 설치파일", at("가람정보/메모.txt") && at("설치파일/설치.exe"));
  ok("B", "실제 주제·코드 프로젝트는 그대로", at("기타/개발/예제.go") && at("tidy/Cargo.toml") && at("tidy/src/main.rs"));
  await main.screenshot({ path: `${SHOT}/m19-B-applied.png`, fullPage: true });
}

// C. review again: nothing left
{
  await panel.locator("[data-testid=organize-again]").click();
  const clean = await waitFor(async () => (await panel.locator("[data-testid=review-clean]").count()) === 1, 15000);
  const t = clean ? await panel.locator("[data-testid=review-clean]").innerText() : "";
  ok("C", "다시 점검 → 합칠 폴더·접을 폴더·옮길 파일이 없습니다", t.includes("없습니다"), t);
  await main.screenshot({ path: `${SHOT}/m19-C-clean.png` });
}

// D. undo the batch → back as it was
{
  await tab("되돌리기").click();
  await sleep(1000);
  const row = main.locator("[data-testid=batch-row]").filter({ hasText: "정리 위치 점검" }).first();
  ok("D", "되돌리기 목록에 '정리 위치 점검' 배치", (await row.count()) === 1);
  await row.getByRole("button", { name: "되돌리기", exact: true }).click();
  const back = await waitFor(() => at("KT 스마트메시지/20261016_과금/c.txt") && at("KT스마트메시지/20261020_과금/b.log") && at("기타/가람정보/메모.txt") && at("설치.exe"), 20000);
  ok("D", "되돌리면 원래 폴더로", back);
}

await browser.close();
const failed = results.filter((r) => !r.pass);
console.log(`\n${results.length - failed.length}/${results.length} passed`);
process.exit(failed.length ? 1 : 0);
