// 0.14.0 (SPEC §14.20): "파일 이동 전에 항상 묻고 이동" — 설정 › 감시 폴더
// "옮기기 전에 항상 묻기" on → a file a rule would move on its own waits in
// 확인 대기 (desktop untouched) → [이동] moves it. Off again → moves as before.
//
// Run: S=/tmp/tidy-e2e bash e2e/run-headless.sh fix-ask
import { chromium } from "playwright";
import fs from "node:fs";
import path from "node:path";
import { addWatchFolder, bridge, gotoSettings, openToml, saveSettings, sleep, tab, waitFor } from "./lib.mjs";

const T = process.env.T;
const DESK = path.join(T, "바탕 화면");
const DOCS = path.join(T, "문서함");
const CFG = path.join(T, "config.toml");
const SHOT = path.join(process.env.S, "shots");
fs.mkdirSync(SHOT, { recursive: true });
const results = [];
const ok = (step, what, pass, note = "") => {
  results.push({ step, what, pass, note });
  console.log(`${pass ? "PASS" : "FAIL"} [${step}] ${what} ${note}`);
};

const browser = await chromium.launch();
const ctx = await browser.newContext({ viewport: { width: 1280, height: 860 } });
const main = await ctx.newPage();
main.on("dialog", (d) => void d.accept(process.env.PROMPT_ANSWER ?? ""));
await main.goto("http://localhost:1420/#/main");
await main.waitForSelector(".topbar");
await sleep(1000);

// A. folder + a rule that moves txt on its own; "항상 묻기" off by default
{
  await gotoSettings(main, "watch");
  await addWatchFolder(main, DESK);
  const ta = await openToml(main);
  await ta.fill(`[[rule]]\nname = "문서"\nmatch = { ext = ["txt"] }\ndest = '${DOCS}'\n`);
  await sleep(500);
  await saveSettings(main, 3000);
  await gotoSettings(main, "watch");
  const box = main.locator("[data-testid=ask-before-move] input[type=checkbox]");
  ok("A", "설정 › 감시 폴더에 '옮기기 전에 항상 묻기' 카드 (기본 꺼짐)", (await box.count()) === 1 && !(await box.isChecked()));
  fs.writeFileSync(path.join(DESK, "꺼짐.txt"), "x");
  const moved = await waitFor(() => fs.existsSync(path.join(DOCS, "꺼짐.txt")), 15000);
  ok("A", "꺼져 있으면 규칙대로 바로 이동 (예전 그대로)", moved);
}

// B. turn it on → saved in config.toml
{
  await gotoSettings(main, "watch");
  await main.locator("[data-testid=ask-before-move] input[type=checkbox]").check();
  await main.locator("[data-testid=ask-before-move]").screenshot({ path: `${SHOT}/fix-ask-B-card.png` });
  await saveSettings(main, 3000);
  ok("B", "저장 → config.toml [mover] ask_before_move = true", /ask_before_move *= *true/.test(fs.readFileSync(CFG, "utf8")));
  await main.screenshot({ path: `${SHOT}/fix-ask-B-settings.png` });
}

// C. a new file the rule matches: waits in 확인 대기, nothing moves
{
  await gotoSettings(main, "advanced");
  await main.getByRole("checkbox", { name: /조용 모드/ }).check();
  await saveSettings(main, 3000);
  fs.writeFileSync(path.join(DESK, "견적서.txt"), "견적");
  const queued = await waitFor(async () => (await bridge("get_status")).pendingCount === 1, 15000);
  await sleep(1500);
  ok("C", "규칙에 맞는 새 파일 → 확인 대기 1건", queued);
  ok("C", "아직 옮기지 않음 (바탕 화면에 그대로)", fs.existsSync(path.join(DESK, "견적서.txt")) && !fs.existsSync(path.join(DOCS, "견적서.txt")));
  await tab(main, "홈").click();
  await sleep(1500);
  const card = await main.locator("[data-testid=organize-card]").innerText().catch(() => "");
  ok("C", "홈 안내: '새 파일도 혼자 옮기지 않고 확인 대기에서 먼저 묻습니다'", card.includes("혼자 옮기지 않고"), card.slice(0, 160).replace(/\n/g, " | "));
  await tab(main, "확인 대기").click();
  await sleep(1000);
  const rows = main.locator("[data-testid=pending-row]");
  const txt = (await rows.count()) ? await rows.first().innerText() : "";
  ok("C", "확인 대기 행: 견적서.txt → 문서함", txt.includes("견적서") && txt.includes("문서함"), txt.replace(/\n/g, " | "));
  await main.screenshot({ path: `${SHOT}/fix-ask-C-pending.png` });
  await rows.first().getByRole("button", { name: "이동", exact: true }).click();
  const done = await waitFor(() => fs.existsSync(path.join(DOCS, "견적서.txt")), 10000);
  ok("C", "[이동] → 그때 옮김", done && !fs.existsSync(path.join(DESK, "견적서.txt")));
  const today = await bridge("today_moves", { since: Math.floor(new Date().setHours(0, 0, 0, 0) / 1000) });
  ok("C", "이동 기록 남음 (오늘 한 일·되돌리기)", JSON.stringify(today).includes("견적서"));
}

await browser.close();
const failed = results.filter((r) => !r.pass);
console.log(`\n${results.length - failed.length}/${results.length} passed`);
process.exit(failed.length ? 1 : 0);
