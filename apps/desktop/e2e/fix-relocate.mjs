// Fix (0.11.2): the first example rules' absolute destinations
// (~/Documents/Tidy/…) ignored the organize location. At start the app
// rewrites them to follow it (backup kept) and offers moving what they
// already put there: home "적용하지 않은 미리보기 · 정리 위치로 옮기기" →
// preview → apply (one batch) → undo. Fixtures: fix-relocate.setup.mjs.
//
// Run: S=/tmp/tidy-e2e bash e2e/run-headless.sh fix-relocate
import { chromium } from "playwright";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { sleep, tab as tabOf, waitFor } from "./lib.mjs";

const T = process.env.T;
const S = process.env.S;
const DESK = path.join(T, "바탕 화면");
const ORG = path.join(T, "정리");
const RULES = path.join(T, "rules.toml");
const TIDY = path.join(os.homedir(), "Documents", "Tidy");
const ZIP = path.join(os.homedir(), "Downloads", "압축 파일", "내가 둔 파일.zip");
const SHOT = path.join(S, "shots");
fs.mkdirSync(SHOT, { recursive: true });

const results = [];
const ok = (step, what, pass, note = "") => {
  results.push({ step, what, pass, note });
  console.log(`${pass ? "PASS" : "FAIL"} [${step}] ${what} ${note}`);
};

// A. rules rewritten at start, backup kept
{
  const rules = fs.readFileSync(RULES, "utf8");
  ok("A", "rules.toml: 문서 → 문서/{yyyy}-{mm}, 압축 파일 → 압축 파일", rules.includes('dest = "문서/{yyyy}-{mm}"') && rules.includes('dest = "압축 파일"'), rules.replace(/\n/g, " | "));
  ok("A", "다른 규칙·주석은 그대로", rules.includes('dest = "로그"') && rules.includes("# 처음 버전 예시 규칙"));
  const bak = path.join(T, "rules.toml.bak-0.11.2");
  ok("A", "원래 rules.toml 백업", fs.existsSync(bak) && fs.readFileSync(bak, "utf8").includes("~/Documents/Tidy/문서"));
}

const browser = await chromium.launch();
const ctx = await browser.newContext({ viewport: { width: 1280, height: 860 } });
const main = await ctx.newPage();
main.on("dialog", (d) => void (d.type() === "prompt" ? d.accept(process.env.PROMPT_ANSWER ?? "") : d.accept()));
await main.goto("http://localhost:1420/#/main");
await main.waitForSelector(".topbar");
await sleep(1500);
const tab = (name) => tabOf(main, name);
const panel = main.locator(".restructure");

// B. home: the relocation preview waits for approval
{
  await tab("홈").click();
  const un = main.locator("[data-testid=unapplied-plans]");
  const shown = await waitFor(async () => (await un.count()) === 1 && (await un.innerText()).includes("정리 위치로 옮기기"), 20000);
  const t = shown ? await un.innerText() : "";
  ok("B", "홈: 적용하지 않은 미리보기 · 문서 · 정리 위치로 옮기기 2개", t.includes("정리 위치로 옮기기 2개"), t.replace(/\n/g, " | "));
  ok("B", "남의 파일(압축 파일 폴더, 기록 없음)은 제안 안 함", !t.includes("압축 파일"));
  ok("B", "미리보기만으로 무이동", fs.existsSync(path.join(TIDY, "문서", "2026-10", "견적서.pdf")));
  await main.screenshot({ path: `${SHOT}/fix-relocate-B-home.png` });
  await un.getByRole("button", { name: "이어서 보기" }).first().click();
  const open = await waitFor(async () => (await panel.locator(".plan").count()) === 1, 10000);
  const txt = open ? await panel.innerText() : "";
  ok("B", "정리하기: 정리 위치로 옮기기 2개 항목, 정리/문서/2026-10 로", txt.includes("정리 위치로 옮기기") && txt.includes("2개 항목"), txt.replace(/\n/g, " | ").slice(0, 300));
  await main.screenshot({ path: `${SHOT}/fix-relocate-B-plan.png`, fullPage: true });
}

// C. apply → files under 정리/문서/2026-10; a new pdf follows the organize location
{
  await panel.getByRole("button", { name: /선택한 \d+개 적용/ }).click();
  const moved = await waitFor(async () => (await panel.locator(".notice", { hasText: "옮겼습니다" }).count()) === 1, 30000);
  const want = ["견적서.pdf", "회의록.docx"].map((n) => path.join(ORG, "문서", "2026-10", n));
  ok("C", "적용 → 정리/문서/2026-10/견적서.pdf·회의록.docx", moved && want.every((p) => fs.existsSync(p)), want.filter((p) => !fs.existsSync(p)).join(","));
  ok("C", "문서\\Tidy에서는 빠짐", !fs.existsSync(path.join(TIDY, "문서", "2026-10", "견적서.pdf")));
  ok("C", "남의 zip은 그대로", fs.existsSync(ZIP));
  fs.writeFileSync(path.join(DESK, "새 보고서.pdf"), "새 파일");
  const d = new Date();
  const ym = `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}`;
  const neu = path.join(ORG, "문서", ym, "새 보고서.pdf");
  ok("C", `새 pdf → 정리/문서/${ym} (문서\\Tidy 아님)`, await waitFor(() => fs.existsSync(neu), 20000), neu);
  await tab("홈").click();
  await sleep(1500);
  ok("C", "홈: 미리보기 안내 사라짐", (await main.locator("[data-testid=unapplied-plans]").count()) === 0);
}

// D. undo the relocation batch → back in 문서\Tidy
{
  await tab("되돌리기").click();
  await sleep(1000);
  const row = main.locator("[data-testid=batch-row]").filter({ hasText: "정리 위치로 옮기기" }).first();
  ok("D", "되돌리기 목록에 '정리 위치로 옮기기' 배치", (await row.count()) === 1);
  await row.getByRole("button", { name: "되돌리기", exact: true }).click();
  const back = await waitFor(() => fs.existsSync(path.join(TIDY, "문서", "2026-10", "견적서.pdf")), 20000);
  ok("D", "되돌리면 문서\\Tidy로 돌아감", back);
}

// E. plan file (D:\정리 cleanup prepared from a folder listing): open → preview → apply → undo
{
  const dev = path.join(ORG, "기타", "개발");
  const parts = { "소스 코드": ["main.go", "a.go"], 소스코드: ["main.go"], "테스트 코드": ["x_test.go"], "빌드 스크립트": ["make.bash"] };
  for (const [d, fsn] of Object.entries(parts)) {
    fs.mkdirSync(path.join(dev, d), { recursive: true });
    for (const f of fsn) fs.writeFileSync(path.join(dev, d, f), `${d}/${f}`);
  }
  const td = path.join(ORG, "텍스트", "2024", "02");
  fs.mkdirSync(td, { recursive: true });
  for (const f of ["allnew.txt", "ascii.txt"]) fs.writeFileSync(path.join(td, f), f);
  const BASE = path.join(ORG, "기타", "Go 소스 모음");
  const bucket = { "소스 코드": "소스 코드", 소스코드: "소스 코드", "테스트 코드": "테스트", "빌드 스크립트": "빌드·도구" };
  const plan = {
    tidy_plan: 1,
    root: ORG,
    summary: "흩어진 Go 소스 조각을 한 곳으로",
    moves: [
      ...Object.keys(parts).map((d) => ({ from: path.join(dev, d), into: path.join(BASE, bucket[d]), reason: d })),
      { from: td, into: path.join(BASE, "테스트 데이터"), reason: "testdata" },
      { from: path.join(T, "바탕 화면"), into: BASE, reason: "root 밖 — 거부" },
    ],
  };
  const file = path.join(T, "plan.json");
  fs.writeFileSync(file, JSON.stringify(plan));
  await tab("정리하기").click();
  process.env.PROMPT_ANSWER = file;
  await panel.locator("[data-testid=import-plan]").click();
  const open = await waitFor(async () => (await panel.locator(".plan").count()) === 1 && (await panel.locator(".plan").innerText()).includes("제안 파일"), 15000);
  const txt = open ? await panel.innerText() : "";
  ok("E", "제안 파일 열기 → 미리보기 5개 항목(폴더째) + 거부 1개 (root 밖)", txt.includes("5개 항목") && txt.includes("받아들이지 않은 AI 제안 1개"), txt.replace(/\n/g, " | ").slice(0, 300));
  ok("E", "미리보기만으로 무이동", fs.existsSync(path.join(dev, "소스 코드", "main.go")));
  await main.screenshot({ path: `${SHOT}/fix-relocate-E-plan.png`, fullPage: true });
  await panel.getByRole("button", { name: /선택한 \d+개 적용/ }).click();
  const moved = await waitFor(async () => (await panel.locator(".notice", { hasText: "옮겼습니다" }).count()) === 1, 30000);
  const src = fs.readdirSync(path.join(BASE, "소스 코드")).sort();
  ok("E", "적용 → 소스 코드에 a.go·main.go·main (1).go (겹친 이름은 번호)", moved && src.length === 3 && src.includes("a.go") && src.includes("main.go"), src.join(","));
  ok("E", "테스트·빌드·도구·테스트 데이터", fs.existsSync(path.join(BASE, "테스트", "x_test.go")) && fs.existsSync(path.join(BASE, "빌드·도구", "make.bash")) && fs.existsSync(path.join(BASE, "테스트 데이터", "allnew.txt")));
  ok("E", "옮긴 뒤 빈 폴더는 정리됨 (기타/개발, 텍스트/2024까지)", !fs.existsSync(dev) && !fs.existsSync(path.join(ORG, "텍스트", "2024")) && fs.existsSync(path.join(ORG, "기타")));
  ok("E", "바탕 화면은 그대로 (root 밖)", fs.existsSync(path.join(T, "바탕 화면")));
  await tab("되돌리기").click();
  await sleep(1000);
  const row = main.locator("[data-testid=batch-row]").filter({ hasText: "제안 파일" }).first();
  await row.getByRole("button", { name: "되돌리기", exact: true }).click();
  const back = await waitFor(() => fs.existsSync(path.join(dev, "소스 코드", "main.go")) && fs.existsSync(path.join(td, "ascii.txt")), 20000);
  ok("E", "되돌리면 원래 폴더로", back);
}

await browser.close();
fs.rmSync(TIDY, { recursive: true, force: true });
fs.rmSync(path.dirname(ZIP), { recursive: true, force: true });
const failed = results.filter((r) => !r.pass);
console.log(`\n${results.length - failed.length}/${results.length} passed`);
process.exit(failed.length ? 1 : 0);
