// 0.18.1: while a long job holds the app lock (applying a big preview),
// the main window must not look dead — also after F5:
//   A. status bar, 정리하기 folder list + enabled button, 확인 대기,
//      찾기·되돌리기, 홈 오늘 한 일 all load while the job runs
//   B. the job bar shows on every tab with progress (n / total, %) and
//      comes back after a reload
//   C. it goes away when the job ends
// Fixtures: fix-busy.setup.mjs. The job is the dev bridge's `debug_job`.
//
// Run: S=/tmp/tidy-e2e bash e2e/run-headless.sh fix-busy
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

// One move in the log, so 오늘 한 일 / 되돌리기 have something to show.
await waitFor(async () => (await bridge("get_status")).watching.length > 0, 20000);
const MEMO = path.join(ORG, "문서", "회의 메모.txt");
let moved = false;
for (let i = 0; i < 3 && !moved; i++) {
  // Written again if the first event fell between watcher (re)starts.
  if (!fs.existsSync(MEMO)) fs.writeFileSync(path.join(DESK, "회의 메모.txt"), `회의 메모 ${i}`);
  moved = await waitFor(() => fs.existsSync(MEMO), 10000);
}
ok("0", "규칙 이동", moved);

const browser = await chromium.launch();
const ctx = await browser.newContext({ viewport: { width: 1180, height: 760 } });
const main = await ctx.newPage();
main.on("dialog", (d) => void d.accept());
await main.goto("http://localhost:1420/#/main");
await main.waitForSelector(".topbar");
await sleep(1000);
const tab = (name) => tabOf(main, name);
const bar = main.locator("[data-testid=jobbar]");

// A long job: 25 s, 500 items.
await bridge("debug_job", { ms: 25000, total: 500 });
ok("B", "작업 막대가 뜸", await waitFor(async () => (await bar.count()) > 0, 5000, 200));

// F5 while it runs.
await main.reload();
const t0 = Date.now();
await main.waitForSelector(".topbar");
{
  const sb = await waitFor(async () => (await main.locator("[data-testid=statusbar]").count()) > 0, 4000, 100);
  ok("A", "새로고침 뒤 상태줄이 바로 나옴", sb, `${Date.now() - t0}ms`);
  ok("B", "새로고침 뒤에도 작업 막대", await waitFor(async () => (await bar.count()) > 0, 3000, 100));
  await sleep(1500);
  const txt = (await bar.innerText()).replace(/\n/g, " ");
  ok("B", "무엇·몇 개째·몇 %·얼마나", /미리보기 적용 중/.test(txt) && /\/ 500개/.test(txt) && /%/.test(txt) && /초째/.test(txt), txt);
  const v = Number(await main.locator("[data-testid=job-progress]").getAttribute("value"));
  ok("B", "진행 막대 값이 늘어남", v > 0, String(v));
  await main.screenshot({ path: path.join(SHOT, "fix-busy-home.png") });
}
{
  await tab("정리하기").click();
  const btn = main.locator("[data-testid=organize-start]");
  await waitFor(async () => (await btn.count()) > 0, 3000, 100);
  const enabled = await waitFor(async () => !(await btn.isDisabled()), 3000, 100);
  ok("A", "정리하기: 폴더 목록이 오고 버튼이 켜짐", enabled, await btn.innerText());
  ok("B", "정리하기 탭에도 작업 막대", (await bar.count()) > 0);
  await main.screenshot({ path: path.join(SHOT, "fix-busy-organize.png") });
}
{
  await tab("확인 대기").click();
  ok("A", "확인 대기 화면이 뜸", await waitFor(async () => (await main.locator("[data-testid=pending-count], .empty").count()) > 0, 3000, 100));
  await tab("찾기·되돌리기").click();
  const rows = main.locator("[data-testid=find-row]");
  ok("A", "찾기·되돌리기: 최근 정리한 파일", await waitFor(async () => (await rows.count()) > 0, 3000, 100));
  await tab("홈").click();
  ok(
    "A",
    "홈 오늘 한 일",
    await waitFor(async () => (await main.locator("[data-testid=today-card]").innerText()).includes("회의 메모"), 3000, 100),
  );
  await tab("설정").click();
  ok("B", "설정 탭에도 작업 막대", (await bar.count()) > 0);
  await tab("홈").click();
}

// C. ends
ok("C", "끝나면 작업 막대가 사라짐", await waitFor(async () => (await bar.count()) === 0, 30000, 500));

await browser.close();
const failed = results.filter((r) => !r.pass);
console.log(`\n${results.length - failed.length}/${results.length} passed`);
process.exit(failed.length ? 1 : 0);
