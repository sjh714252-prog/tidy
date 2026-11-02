// 0.12.0 (SPEC §14.18): 업무 맥락 — every watch folder at once, folders that
// already exist in the organize root reused. Two watch folders (바탕 화면,
// 다운로드) → 정리, where "가람정보/20261018_과금이슈" already exists:
//   A. 설정 › 업무 맥락 shows 가져올 곳 2곳 → 만들 곳 정리(이미 있는 폴더 1개)
//      and the ①②③ order; "감시 폴더 2곳에서 뽑기" sends the existing
//      folders to the AI and says which folders it looked at.
//   B. ② "전체 훑어 폴더 구성안 만들기" → 정리하기 (모든 감시 폴더) → cost
//      notice → one plan over both folders: 폴더 구성안 with 가람정보 기존 /
//      하늘통신 새 폴더, rows tagged 기존 폴더 / 새 폴더. Nothing moved.
//   C. 옮기기 → the 20261020 과금 log lands in the existing 20261018 folder,
//      the 다운로드 file under 가람정보 (new issue), 하늘통신 new.
//   D. 되돌리기 → everything back.
//
// Run: S=/tmp/tidy-e2e bash e2e/run-headless.sh fix-context
import { chromium } from "playwright";
import { spawn } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { addWatchFolder, bridge, gotoSettings, saveSettings, sleep, tab as tabOf, waitFor } from "./lib.mjs";

const T = process.env.T;
const S = process.env.S;
const DESK = path.join(T, "바탕 화면");
const DOWN = path.join(T, "다운로드");
const ORG = path.join(T, "정리");
const CFG = path.join(T, "config.toml");
const CONTEXT = path.join(T, "context.toml");
const SHOT = path.join(S, "shots");
fs.mkdirSync(SHOT, { recursive: true });
const LLM_PORT = 18095;
const LLM = `http://127.0.0.1:${LLM_PORT}`;

const results = [];
const ok = (step, what, pass, note = "") => {
  results.push({ step, what, pass, note });
  console.log(`${pass ? "PASS" : "FAIL"} [${step}] ${what} ${note}`);
};
const walk = (dir, base = dir) =>
  fs.existsSync(dir)
    ? fs.readdirSync(dir, { withFileTypes: true }).flatMap((e) => {
        const p = path.join(dir, e.name);
        return e.isDirectory() ? walk(p, base) : [path.relative(base, p).split(path.sep).join("/")];
      })
    : [];
const files = (dir) => fs.readdirSync(dir, { withFileTypes: true }).filter((e) => e.isFile()).map((e) => e.name);
const stats = async () => (await fetch(`${LLM}/stats`)).json();

const llm = spawn(process.execPath, [path.join(import.meta.dirname, "fake-llm.mjs"), String(LLM_PORT)], {
  stdio: ["ignore", "inherit", "inherit"],
  env: { ...process.env, FAKE_DELAY_MS: "20" },
});
process.on("exit", () => llm.kill());
await sleep(800);

// ── fixtures ──────────────────────────────────────────────────────────
fs.mkdirSync(DOWN, { recursive: true });
fs.mkdirSync(path.join(ORG, "가람정보", "20261018_과금이슈", "로그"), { recursive: true });
fs.writeFileSync(path.join(ORG, "가람정보", "20261018_과금이슈", "로그", "예전 과금.log"), "old");
const DESK_FILES = ["가람정보_과금_20261020.log", "하늘통신_전환_20261001.txt"];
for (const n of DESK_FILES) fs.writeFileSync(path.join(DESK, n), `업무 자료: ${n}`);
fs.writeFileSync(path.join(DOWN, "GARA_장애_20261105.txt"), "장애 보고");
fs.writeFileSync(CONTEXT, '[[entity]]\nname = "가람정보"\naliases = ["GARA"]\n');
const OLD = new Date("2026-10-20T12:00:00");
for (const p of [...DESK_FILES.map((n) => path.join(DESK, n)), path.join(DOWN, "GARA_장애_20261105.txt")])
  fs.utimesSync(p, OLD, OLD);

const browser = await chromium.launch();
const ctx = await browser.newContext({ viewport: { width: 1280, height: 900 } });
const main = await ctx.newPage();
const dialogs = [];
main.on("dialog", (d) => {
  dialogs.push(d.message());
  if (d.type() === "prompt") void d.accept(process.env.PROMPT_ANSWER ?? "");
  else void d.accept();
});
await main.goto("http://localhost:1420/#/main");
await main.waitForSelector(".topbar");
await sleep(1000);
const tab = (name) => tabOf(main, name);
const save = () => saveSettings(main, 3000);
const panel = main.locator(".restructure");

// Setup: two watch folders → 정리 (모으기), AI on (fake model).
{
  await gotoSettings(main, "watch");
  await addWatchFolder(main, DESK);
  await addWatchFolder(main, DOWN);
  process.env.PROMPT_ANSWER = ORG;
  await main.locator("[data-testid=common-root]").getByRole("button", { name: "찾아보기…" }).click();
  await sleep(500);
  await save();
  await gotoSettings(main, "ai");
  const ai = main.locator("[data-testid=ai-section]");
  await ai.locator("label.row", { hasText: "AI 사용" }).locator("input[type=checkbox]").check();
  await ai.locator("select").first().selectOption("openai");
  await sleep(300);
  const arows = ai.locator("label.row");
  await arows.filter({ hasText: "모델" }).first().locator("input").fill("fake-slow");
  await arows.filter({ hasText: "주소" }).first().locator("input").fill(`${LLM}/v1`);
  await arows.filter({ hasText: "키 이름" }).first().locator("input").fill("");
  await save();
  await waitFor(async () => (await bridge("get_status")).aiState === "ready", 20000);
  const cfg = fs.readFileSync(CFG, "utf8");
  ok("0", "config: 감시 2곳 → 정리 모으기", (cfg.match(/mode = "collect"/g) ?? []).length === 2, cfg.replace(/\n/g, " | ").slice(0, 300));
}

// A. 설정 › 업무 맥락: 가져올 곳 → 만들 곳, ①②③, 감시 폴더 2곳에서 뽑기
{
  await gotoSettings(main, "context");
  const route = await main.locator("[data-testid=context-route]").innerText();
  ok("A", "가져올 곳 2곳 → 만들 곳 정리 · 이미 있는 폴더 1개", route.includes("바탕 화면") && route.includes("다운로드") && route.includes("이미 있는 폴더 1개"), route.replace(/\n/g, " | "));
  const steps = await main.locator(".context-flow > li h3").allInnerTexts();
  ok("A", "① 사전 ② 처음 전체 훑기 ③ 새 파일 순서", steps.length === 3 && steps[0].startsWith("①") && steps[1].includes("전체 훑기") && steps[2].includes("새로 생기는"), steps.join(" / "));
  ok("A", "폴더 틀·미분류 기준은 세부 설정에 접힘", !(await main.locator("[data-testid=context-advanced]").evaluate((e) => e.open)));
  const ex = main.locator("[data-testid=extract-context]");
  ok("A", "버튼 이름: 감시 폴더 2곳에서 뽑기", (await ex.innerText()).includes("감시 폴더 2곳에서 뽑기"), await ex.innerText());
  await ex.click();
  await waitFor(async () => (await main.locator("[data-testid=candidates]").count()) === 1, 30000);
  const cand = await main.locator("[data-testid=candidates]").innerText();
  ok("A", "후보 상자에 훑은 감시 폴더 2곳", cand.includes("감시 폴더 2곳"), cand.split("\n")[0]);
  const last = (await stats()).lastUser;
  ok("A", "AI에 정리 위치의 기존 폴더를 함께 보냄", last.includes("정리 위치에 이미 있는 폴더") && last.includes("가람정보: 20261018_과금이슈"), last.slice(0, 200));
  await main.locator("[data-testid=candidates]").getByRole("button", { name: "닫기" }).click();
  await main.screenshot({ path: `${SHOT}/context-A-settings.png`, fullPage: true });
}

// B. ② 전체 훑어 폴더 구성안 만들기 → 정리하기(모든 감시 폴더) → 구성안
{
  dialogs.length = 0;
  await main.locator("[data-testid=survey-all]").click();
  const planned = await waitFor(async () => (await panel.locator("[data-testid=context-plan]").count()) === 1, 60000);
  ok("B", "정리하기로 가서 비용 안내 → 업무 맥락 미리보기", planned && dialogs.some((d) => d.includes("AI로 분석")), dialogs.join(" / ").slice(0, 200));
  ok("B", "정리할 폴더 = 모든 감시 폴더", (await main.locator("[data-testid=report-folder]").inputValue()) === "*all-watches*");
  const rows = await panel.locator("[data-testid=context-row]").count();
  ok("B", "두 감시 폴더의 항목 3개가 한 제안에", rows === 3, String(rows));
  const box = main.locator("[data-testid=structure]");
  const sum = await box.locator("summary").innerText();
  ok("B", "폴더 구성안: 기존 폴더 다시 씀 + 새 폴더", /이미 있는 폴더 \d+곳/.test(sum) && /새 폴더 [1-9]\d*곳/.test(sum), sum);
  const dirOf = (name) => box.locator("[data-testid=structure-dir]", { hasText: name }).first();
  ok("B", "가람정보 = 기존", (await dirOf("가람정보").getAttribute("data-exists")) === "1");
  ok("B", "20261018_과금이슈 = 기존(20일 파일도 이 폴더로)", (await dirOf("20261018_과금이슈").getAttribute("data-exists")) === "1");
  ok("B", "하늘통신 = 새 폴더", (await dirOf("하늘통신").getAttribute("data-exists")) === "0");
  const row = (n) => panel.locator(`[data-testid=context-row][data-name='${n}']`);
  ok("B", "과금 로그 행: 기존 폴더", (await row("가람정보_과금_20261020.log").locator("[data-testid=ctx-existing]").count()) === 1);
  ok("B", "하늘통신 행: 새 폴더: 하늘통신", (await row("하늘통신_전환_20261001.txt").locator("[data-testid=ctx-new]").innerText()).includes("하늘통신"));
  const from = await row("GARA_장애_20261105.txt").locator("td.path").first().innerText();
  ok("B", "다운로드 항목은 '다운로드/…'로 표시", from.startsWith("다운로드/"), from);
  ok("B", "미리보기만으로 무이동", files(DESK).length === 2 && files(DOWN).length === 1 && walk(ORG).length === 1);
  await main.screenshot({ path: `${SHOT}/context-B-structure.png`, fullPage: true });
}

// C. 옮기기 → 기존 폴더에 합침
{
  await panel.getByRole("button", { name: /선택한 \d+개 옮기기/ }).click();
  const done = await waitFor(async () => (await panel.locator("[data-testid=applied-box]").count()) === 1, 30000);
  const org = walk(ORG);
  ok("C", "옮김 3개 + 예전 파일", done && org.length === 4, org.join(", "));
  ok("C", "20일 과금 로그 → 이미 있던 20261018_과금이슈", org.includes("가람정보/20261018_과금이슈/로그/가람정보_과금_20261020.log"));
  ok("C", "다운로드 장애 → 가람정보/20261105_장애", org.some((p) => p.startsWith("가람정보/20261105_장애/")), org.join(", "));
  ok("C", "하늘통신 → 새 폴더", org.some((p) => p.startsWith("하늘통신/")));
  ok("C", "감시 폴더 비움", files(DESK).length === 0 && files(DOWN).length === 0);
  await main.screenshot({ path: `${SHOT}/context-C-applied.png`, fullPage: true });
}

// D. 되돌리기
{
  await panel.getByRole("button", { name: "이 정리 되돌리기" }).click();
  const back = await waitFor(async () => files(DESK).length === 2 && files(DOWN).length === 1, 15000);
  ok("D", "되돌리기 → 원래 자리, 예전 파일은 그대로", back && fs.existsSync(path.join(ORG, "가람정보", "20261018_과금이슈", "로그", "예전 과금.log")));
}

await browser.close();
const failed = results.filter((r) => !r.pass);
console.log(`\n${results.length - failed.length}/${results.length} passed`);
process.exit(failed.length ? 1 : 0);
