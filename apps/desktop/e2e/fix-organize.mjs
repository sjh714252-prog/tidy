// Fix (0.11.1): "바탕화면 정리가 안 됨" — the full first-organize flow as a
// user meets it: desktop with mixed existing files, collected into 정리,
// AI (fake) + work context on. Home → 지금 정리 → preview → apply →
// 업무 맥락 → apply; then new files while watching (확인 대기).
//
// Run: S=/tmp/tidy-e2e bash e2e/run-headless.sh fix-organize
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

// ── fixtures: a real-looking desktop ───────────────────────────────────
fs.mkdirSync(ORG, { recursive: true });
const FILES = [
  "가람정보_견적서_20261014.pdf", "가람정보_회의록_20261015.docx", "GARA_과금_20261016.xlsx",
  "누리원_계약서_20261001.pdf", "누리원_요구사항_20261002.hwp",
  "보고서_최종.docx", "회의자료.pptx", "예산안.xlsx", "참고문헌.pdf", "메모.txt",
  "스크린샷 2026-10-20 101010.png", "IMG_0001.jpg", "사진.jpeg",
  "설치파일.zip", "data.csv", "노트.md",
];
for (const n of FILES) fs.writeFileSync(path.join(DESK, n), `업무 자료: ${n}`);
fs.writeFileSync(CONTEXT, '[[entity]]\nname = "가람정보"\naliases = ["GARA"]\n\n[[entity]]\nname = "누리원"\n');
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

// B. home: the card says how existing files get moved
{
  await bridge("watch_overview", { refresh: true });
  await tab("홈").click();
  await sleep(3000);
  const card = await main.locator("[data-testid=organize-card]").innerText();
  ok("B", "홈 카드: 16개 + '미리보기 → [적용]을 눌러야 옮겨집니다' 안내", card.includes("16") && card.includes("[적용]을 눌러야 옮겨집니다"), card.replace(/\n/g, " | ").slice(0, 200));
  await main.screenshot({ path: `${SHOT}/fix-B-home.png`, fullPage: true });
}

// C. 지금 정리, cost notice declined: the empty rules preview explains and offers 업무 맥락
{
  dismiss = true;
  dialogs.length = 0;
  await main.locator("[data-testid=organize-now]").first().click();
  await waitFor(async () => (await panel.locator(".plan").count()) === 1 && dialogs.length > 0, 30000);
  await sleep(500);
  dismiss = false;
  ok("C", "규칙 0개 → 바로 업무 맥락 비용 안내", dialogs.some((d) => d.includes("AI로 분석")), dialogs[0]?.replace(/\n/g, " ").slice(0, 80));
  const hint = await panel.locator("[data-testid=curate-hint]").innerText().catch(() => "");
  ok("C", "취소하면: '규칙으로 옮길 파일이 없습니다 · 파일 16개는 그대로 남습니다' + 업무 맥락 버튼", hint.includes("규칙으로 옮길 파일이 없습니다") && hint.includes("파일 16개") && (await panel.locator("[data-testid=curate-rest]").count()) === 1, hint.replace(/\n/g, " | "));
  await main.screenshot({ path: `${SHOT}/fix-C-empty-rules.png` });
}

// D. 업무 맥락 → preview (nothing moved) → home shows the unapplied preview
{
  dialogs.length = 0;
  await panel.locator("[data-testid=curate-rest]").click();
  const planned = await waitFor(async () => (await panel.locator("[data-testid=context-plan]").count()) === 1, 60000);
  ok("D", "업무 맥락 미리보기", planned);
  const notice = await panel.locator("[data-testid=no-move-notice]").innerText();
  ok("D", "미리보기 안내: '미리보기입니다 … [선택한 N개 적용]을 눌러야'", notice.includes("미리보기입니다") && notice.includes("눌러야"), notice);
  ok("D", "'적용하면 (미리보기)' 트리 제목", (await panel.locator("h4", { hasText: "적용하면 (미리보기)" }).count()) === 1);
  // The apply bar is on screen without scrolling to the end of a long table.
  await panel.locator("[data-testid=context-plan]").scrollIntoViewIfNeeded();
  const box = await panel.locator("[data-testid=apply-bar]").boundingBox();
  ok("D", "적용 버튼 줄이 화면 안에 보임 (sticky)", !!box && box.y >= 0 && box.y + box.height <= 860 + 1, JSON.stringify(box));
  await main.screenshot({ path: `${SHOT}/fix-D-preview.png` });
  ok("D", "미리보기만으로 무이동", walk(ORG).length === 0 && deskFiles().length === 16);
  await tab("홈").click();
  const un = main.locator("[data-testid=unapplied-plans]");
  const shown = await waitFor(async () => (await un.count()) === 1, 10000);
  const t = shown ? await un.innerText() : "";
  ok("D", "홈: '적용하지 않은 미리보기 · 바탕 화면 · 업무 맥락 16개'", t.includes("적용하지 않은 미리보기") && t.includes("업무 맥락 16개"), t.replace(/\n/g, " | "));
  await main.screenshot({ path: `${SHOT}/fix-D-home-unapplied.png` });
  ok("D", "홈: 미리보기 [버리기] 버튼 (0.12.4)", (await un.locator("[data-testid=discard-plan]").count()) === 1);
  await un.getByRole("button", { name: "이어서 보기" }).click();
  const back = await waitFor(async () => (await panel.locator("[data-testid=context-plan]").count()) === 1, 10000);
  ok("D", "이어서 보기 → 같은 미리보기", back);
}

// E. apply → the tree appears in 정리, the desktop is empty, the reminder is gone
{
  await panel.getByRole("button", { name: /선택한 \d+개 옮기기/ }).click();
  const moved = await waitFor(async () => (await panel.locator(".notice", { hasText: "옮겼습니다" }).count()) === 1, 30000);
  const org = walk(ORG);
  ok("E", "적용 → 정리/가람정보/…, 정리/기타/… 에 16개", moved && org.length === 16 && org.some((p) => p.startsWith("가람정보/")), org.slice(0, 3).join(", "));
  ok("E", "바탕 화면 비어 있음", deskFiles().length === 0, deskFiles().join(","));
  ok("E", "적용 뒤 안내: '이 제안은 적용되었습니다'", (await panel.locator("[data-testid=no-move-notice]").innerText()).includes("적용되었습니다"));
  await tab("홈").click();
  await sleep(1500);
  ok("E", "홈: 적용하지 않은 미리보기 안내 사라짐", (await main.locator("[data-testid=unapplied-plans]").count()) === 0);
  await main.screenshot({ path: `${SHOT}/fix-E-home.png`, fullPage: true });
}


await browser.close();
const failed = results.filter((r) => !r.pass);
console.log(`\n${results.length - failed.length}/${results.length} passed`);
process.exit(failed.length ? 1 : 0);
