// Fix (0.14.0): "빈 폴더를 지우는 기능 필요" — folders emptied by earlier
// organizing stayed on the desktop / in Downloads. Home lists them
// ("빈 폴더 N개"), the user unticks one and removes the rest; a folder
// with a file, a shortcut or a just-made one stays; undo makes them again.
//
// Run: S=/tmp/tidy-e2e bash e2e/run-headless.sh fix-empty
import { chromium } from "playwright";
import fs from "node:fs";
import path from "node:path";
import { addWatchFolder, bridge, gotoSettings, saveSettings, sleep, tab as tabOf, waitFor } from "./lib.mjs";

const T = process.env.T;
const S = process.env.S;
const DESK = path.join(T, "바탕 화면");
const ORG = path.join(T, "정리");
const TRASH = path.join(T, "휴지통");
const SHOT = path.join(S, "shots");
fs.mkdirSync(SHOT, { recursive: true });

const results = [];
const ok = (step, what, pass, note = "") => {
  results.push({ step, what, pass, note });
  console.log(`${pass ? "PASS" : "FAIL"} [${step}] ${what} ${note}`);
};
const at = (...p) => path.join(DESK, ...p);
const exists = (p) => fs.existsSync(p);

// ── fixtures: what earlier organizing left behind ──────────────────────
fs.mkdirSync(at("java", "lib", "ext"), { recursive: true });
fs.mkdirSync(at("files"), { recursive: true });
fs.writeFileSync(at("files", "desktop.ini"), "[.ShellClassInfo]");
fs.mkdirSync(at("고객", "빈 하위"), { recursive: true });
fs.writeFileSync(at("고객", "a.txt"), "고객 자료");
fs.mkdirSync(at("바로가기만"), { recursive: true });
fs.writeFileSync(at("바로가기만", "앱.lnk"), "lnk");
fs.mkdirSync(path.join(ORG, "문서", "2026-10"), { recursive: true });
const OLD = new Date("2026-10-20T12:00:00");
// Deepest first: making a child touches its parent's time.
for (const d of [
  at("java", "lib", "ext"), at("java", "lib"), at("java"), at("files"), at("고객", "빈 하위"), at("고객"),
  at("바로가기만"), path.join(ORG, "문서", "2026-10"), path.join(ORG, "문서"),
]) fs.utimesSync(d, OLD, OLD);
fs.mkdirSync(at("새 폴더")); // just made: waits

const browser = await chromium.launch();
const ctx = await browser.newContext({ viewport: { width: 1280, height: 900 } });
const main = await ctx.newPage();
main.on("dialog", (d) => void (d.type() === "prompt" ? d.accept(process.env.PROMPT_ANSWER ?? "") : d.accept()));
await main.goto("http://localhost:1420/#/main");
await main.waitForSelector(".topbar");
await sleep(1000);
const tab = (name) => tabOf(main, name);

// A. desktop → 정리 (collect)
{
  await gotoSettings(main, "watch");
  await addWatchFolder(main, DESK);
  process.env.PROMPT_ANSWER = ORG;
  await main.locator("[data-testid=common-root]").getByRole("button", { name: "찾아보기…" }).click();
  await sleep(500);
  await saveSettings(main, 3000);
  ok("A", "감시 폴더 바탕 화면 → 정리", /mode = "collect"/.test(fs.readFileSync(path.join(T, "config.toml"), "utf8")));
}

// B. home lists the empty folders (top-most only), not the others
const box = main.locator("[data-testid=empty-folders]");
{
  await bridge("watch_overview", { refresh: true });
  await tab("홈").click();
  const seen = await waitFor(async () => (await box.count()) === 1, 15000);
  const o = await bridge("watch_overview", {});
  const names = (o.empty ?? []).map((e) => e.name).sort();
  ok("B", "홈: 빈 폴더 4개 (java·files·빈 하위·문서)", seen && names.join(",") === ["files", "java", "문서", "빈 하위"].sort().join(","), names.join(","));
  ok("B", "파일·바로가기 든 폴더, 방금 만든 폴더는 빠짐", !names.includes("고객") && !names.includes("바로가기만") && !names.includes("새 폴더"));
  ok("B", "java: 빈 하위 폴더 2개 포함", (o.empty ?? []).find((e) => e.name === "java")?.subFolders === 2);
  ok("B", "버튼: 빈 폴더 4개 정리", (await box.locator("[data-testid=remove-empty]").innerText()).includes("4개"));
  await main.screenshot({ path: `${SHOT}/fix-empty-B-home.png`, fullPage: true });
}

// C. untick "files", remove the rest
let batch = null;
{
  await box.getByRole("button", { name: "살펴보기" }).click();
  await box.locator("li", { hasText: "files" }).locator("[data-testid=empty-check]").uncheck();
  ok("C", "체크 해제 → 3개", (await box.locator("[data-testid=remove-empty]").innerText()).includes("3개"));
  await main.screenshot({ path: `${SHOT}/fix-empty-C-list.png`, fullPage: true });
  await box.locator("[data-testid=remove-empty]").click();
  const gone = await waitFor(async () => !exists(at("java")), 10000);
  ok("C", "java·고객/빈 하위·정리/문서 지움", gone && !exists(at("고객", "빈 하위")) && !exists(path.join(ORG, "문서")));
  ok("C", "files·고객/a.txt·바로가기만·새 폴더 그대로", exists(at("files", "desktop.ini")) && exists(at("고객", "a.txt")) && exists(at("바로가기만", "앱.lnk")) && exists(at("새 폴더")));
  const left = await waitFor(async () => (await box.innerText().catch(() => "")).includes("빈 폴더 1개"), 15000);
  ok("C", "홈: 남은 빈 폴더 1개(files)", left);
  const bs = await bridge("list_batches", { limit: 5 });
  batch = bs[0]?.batch_id ?? bs[0]?.batchId;
  ok("C", "한 배치로 기록", !!batch, JSON.stringify(bs[0] ?? {}).slice(0, 160));
}

// D. "files" too: desktop.ini goes to the trash (folder TIDY_E2E_TRASH)
{
  await box.locator("[data-testid=remove-empty]").click();
  const gone = await waitFor(async () => !exists(at("files")), 10000);
  ok("D", "files 지움 + desktop.ini는 휴지통으로", gone && exists(path.join(TRASH, "desktop.ini")));
  const hidden = await waitFor(async () => (await box.count()) === 0, 15000);
  ok("D", "빈 폴더 없음 → 안내 사라짐", hidden);
}

// E. undo the first batch: the folders are back
{
  await bridge("undo_batch", { batchId: batch });
  ok("E", "되돌리기: java/lib/ext·고객/빈 하위·정리/문서/2026-10 다시 생김",
    exists(at("java", "lib", "ext")) && exists(at("고객", "빈 하위")) && exists(path.join(ORG, "문서", "2026-10")));
  await tab("되돌리기").click();
  await sleep(1500);
  await main.screenshot({ path: `${SHOT}/fix-empty-E-history.png`, fullPage: true });
}

await browser.close();
const failed = results.filter((r) => !r.pass);
console.log(`\n${results.length - failed.length}/${results.length} passed`);
process.exit(failed.length ? 1 : 0);
