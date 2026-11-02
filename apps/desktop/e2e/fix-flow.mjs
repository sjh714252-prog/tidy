// Fix (0.11.3): 정리하기 screen — one next step. A desktop the AI is unsure
// about (every item 미분류, like 지훈's proposal #18): 지금 정리 → the
// preview says why nothing is ticked, offers "AI 추측대로 넣기" /
// "기타/미분류에 종류별로 모으기" right there and in the apply bar → apply.
//
// Run: S=/tmp/tidy-e2e bash e2e/run-headless.sh fix-flow
import { chromium } from "playwright";
import { spawn } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { addWatchFolder, bridge, gotoSettings, saveSettings, sleep, tab as tabOf, waitFor } from "./lib.mjs";

const T = process.env.T;
const S = process.env.S;
const DESK = path.join(T, "바탕 화면");
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
const deskFiles = () => fs.readdirSync(DESK, { withFileTypes: true }).filter((e) => e.isFile()).map((e) => e.name);

const llm = spawn(process.execPath, [path.join(import.meta.dirname, "fake-llm.mjs"), String(LLM_PORT)], {
  stdio: ["ignore", "inherit", "inherit"],
  env: { ...process.env, FAKE_DELAY_MS: "20" },
});
process.on("exit", () => llm.kill());
await sleep(800);

// ── fixtures: a desktop the AI is unsure about (like 지훈's #18) ──────
fs.mkdirSync(ORG, { recursive: true });
const FILES = ["가람정보_애매_메모.txt", "애매한 보고서.html", "client_secret_애매.json", "애매 회의.txt"];
for (const n of FILES) fs.writeFileSync(path.join(DESK, n), `업무 자료: ${n}`);
fs.writeFileSync(CONTEXT, '[[entity]]\nname = "가람정보"\naliases = ["GARA"]\n');
const OLD = new Date("2026-10-20T12:00:00");
for (const n of FILES) fs.utimesSync(path.join(DESK, n), OLD, OLD);

const browser = await chromium.launch();
const ctx = await browser.newContext({ viewport: { width: 1280, height: 860 } });
const main = await ctx.newPage();
const dialogs = [];
let dismiss = false;
main.on("dialog", (d) => {
  dialogs.push(d.message());
  if (d.type() === "prompt") void d.accept(process.env.PROMPT_ANSWER ?? "");
  else if (dismiss && d.type() === "confirm") void d.dismiss();
  else void d.accept();
});
await main.goto("http://localhost:1420/#/main");
await main.waitForSelector(".topbar");
await sleep(1000);
const tab = (name) => tabOf(main, name);
const save = () => saveSettings(main, 3000);
const panel = main.locator(".restructure");

// A. settings like 지훈's: desktop → 정리 (collect), AI on, work context on
{
  await gotoSettings(main, "watch");
  await addWatchFolder(main, DESK);
  process.env.PROMPT_ANSWER = ORG;
  await main.locator("[data-testid=common-root]").getByRole("button", { name: "찾아보기…" }).click();
  await sleep(500);
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
  await gotoSettings(main, "context");
  await main.locator("[data-testid=curate-enabled]").check();
  await save();
  ok("A", "config: 바탕 화면 → 정리 모으기 + 업무 맥락 켬, 규칙 없음", /mode = "collect"/.test(fs.readFileSync(CFG, "utf8")));
}

// B. 홈 → 지금 정리 → (비용 안내 수락) → 업무 맥락 미리보기: 모두 미분류
{
  await bridge("watch_overview", { refresh: true });
  await tab("홈").click();
  await sleep(3000);
  dialogs.length = 0;
  await main.locator("[data-testid=organize-now]").first().click();
  const planned = await waitFor(async () => (await panel.locator("[data-testid=context-plan]").count()) === 1, 60000);
  ok("B", "지금 정리 → 비용 안내 → 업무 맥락 미리보기", planned && dialogs.some((d) => d.includes("AI로 분석")));
  const steps = await main.locator("[data-testid=organize-steps] li.now").innerText();
  ok("B", "단계 표시: 2 확인하고 고르기", steps.includes("확인하고 고르기"), steps);
  const head = await panel.locator("[data-testid=plan-headline]").innerText();
  ok("B", "한 줄 요약: 옮길 곳 정해진 항목 없음 + 확신 못 한 4개", head.includes("아직 옮길 곳이 정해진 항목이 없습니다") && head.includes("4개"), head);
  const box = await panel.locator("[data-testid=unsure-box]").innerText();
  ok("B", "미분류 안내 상자 + 버튼 두 개", box.includes("확신하지 못한 항목 4개") && (await panel.locator("[data-testid=unsure-box] [data-testid=settle-guess]").count()) === 1 && (await panel.locator("[data-testid=unsure-box] [data-testid=settle-kind]").count()) === 1, box.replace(/\n/g, " | "));
  const apply = panel.getByRole("button", { name: /선택한 \d+개 옮기기/ });
  ok("B", "옮기기 버튼은 0개로 비활성 + 이유와 버튼이 같은 줄에", (await apply.innerText()).includes("0개") && (await apply.isDisabled()) && (await panel.locator("[data-testid=apply-bar] [data-testid=apply-empty-hint]").isVisible()) && (await panel.locator("[data-testid=apply-bar] [data-testid=settle-kind]").isVisible()));
  ok("B", "다른 방법 버튼들은 접혀 있음", !(await panel.getByRole("button", { name: "규칙으로 정리 미리보기" }).isVisible()) && !(await panel.locator("[data-testid=import-plan]").isVisible()));
  ok("B", "미리보기만으로 무이동", walk(ORG).length === 0 && deskFiles().length === 4);
  await main.screenshot({ path: `${SHOT}/flow-B-unsure.png` });
}

// C. AI 추측대로 넣기 → 체크되고 옮기기 버튼이 켜짐 (아직 무이동)
{
  await panel.locator("[data-testid=unsure-box] [data-testid=settle-guess]").click();
  const apply = panel.getByRole("button", { name: /선택한 \d+개 옮기기/ });
  const on = await waitFor(async () => (await apply.isEnabled()) && (await apply.innerText()).includes("4개"), 15000);
  ok("C", "AI 추측대로 → 선택한 4개 옮기기 활성", on, await apply.innerText());
  ok("C", "미분류 상자 사라짐", (await panel.locator("[data-testid=unsure-box]").count()) === 0);
  const tags = await panel.locator("[data-testid=context-plan] .tag", { hasText: "AI 추측" }).count();
  ok("C", "행마다 'AI 추측' 표시", tags === 4, String(tags));
  const garam = await panel.locator("[data-testid=context-row][data-name='가람정보_애매_메모.txt'] td.path").nth(1).innerText();
  ok("C", "가람정보 추측은 가람정보/… 로", garam.includes("/정리/가람정보/"), garam);
  ok("C", "아직 무이동", walk(ORG).length === 0);
  await main.screenshot({ path: `${SHOT}/flow-C-guess.png` });
}

// D. 옮기기 → 정리 위치로, 완료 상자에 되돌리기 + 다시 미리보기
{
  await panel.getByRole("button", { name: /선택한 \d+개 옮기기/ }).click();
  const done = await waitFor(async () => (await panel.locator("[data-testid=applied-box]").count()) === 1, 30000);
  const org = walk(ORG);
  ok("D", "옮김: 정리 아래 4개, 바탕 화면 비어 있음", done && org.length === 4 && deskFiles().length === 0, org.join(", "));
  const steps = await main.locator("[data-testid=organize-steps] li.now").innerText();
  ok("D", "단계 표시: 3 옮기기", steps.includes("옮기기"), steps);
  ok("D", "완료 상자: 되돌리기 + 남은 파일로 다시 미리보기", (await panel.locator("[data-testid=organize-again]").count()) === 1);
  await main.screenshot({ path: `${SHOT}/flow-D-applied.png` });
}

// E. 새 파일 → 다시 미리보기 → 종류별로 모으기 → 기타/미분류/종류
{
  fs.writeFileSync(path.join(DESK, "애매한 표.csv"), "a,b");
  fs.utimesSync(path.join(DESK, "애매한 표.csv"), OLD, OLD);
  dialogs.length = 0;
  await panel.locator("[data-testid=organize-again]").click();
  const planned = await waitFor(async () => (await panel.locator("[data-testid=unsure-box]").count()) === 1, 60000);
  ok("E", "다시 미리보기 → 미분류 1개", planned);
  await panel.locator("[data-testid=unsure-box] [data-testid=settle-kind]").click();
  const apply = panel.getByRole("button", { name: /선택한 \d+개 옮기기/ });
  await waitFor(async () => apply.isEnabled(), 15000);
  const to = await panel.locator("[data-testid=context-row] td.path").nth(1).innerText();
  ok("E", "종류별로 → 기타/미분류/데이터", to.includes("/기타/미분류/데이터/"), to);
  await apply.click();
  await waitFor(async () => (await panel.locator("[data-testid=applied-box]").count()) === 1, 30000);
  ok("E", "정리/기타/미분류/데이터/애매한 표.csv", fs.existsSync(path.join(ORG, "기타", "미분류", "데이터", "애매한 표.csv")), walk(ORG).join(", "));
  await main.screenshot({ path: `${SHOT}/flow-E-kind.png` });
}

await browser.close();
const failed = results.filter((r) => !r.pass);
console.log(`\n${results.length - failed.length}/${results.length} passed`);
process.exit(failed.length ? 1 : 0);
