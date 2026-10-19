// M3 GUI test (PR #4 steps 2–9, 11) driven through the UI at localhost:1420
// against the running tidy-desktop (dev bridge on :1421).
import { chromium } from "playwright";
import fs from "node:fs";
import path from "node:path";

const T = process.env.T;
const DESK = path.join(T, "바탕 화면");
const SHOT = path.join(process.env.S, "shots");
fs.mkdirSync(SHOT, { recursive: true });
const results = [];
const ok = (step, what, pass, note = "") => {
  results.push({ step, what, pass, note });
  console.log(`${pass ? "PASS" : "FAIL"} [${step}] ${what} ${note}`);
};
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
// M8: the rules TOML editor sits in the collapsed "고급" section.
const openAdvanced = async (page) => {
  const d = page.locator("details.advanced");
  if (!(await d.evaluate((e) => e.open))) await d.locator("summary").click();
};
const bridge = async (cmd, args = {}) =>
  (await (await fetch(`http://127.0.0.1:1421/invoke/${cmd}`, { method: "POST", body: JSON.stringify(args) })).json()).value;

const browser = await chromium.launch();
const ctx = await browser.newContext({ viewport: { width: 960, height: 680 } });
const main = await ctx.newPage();
main.on("dialog", (d) => d.accept(process.env.PROMPT_ANSWER ?? ""));
await main.goto("http://localhost:1420/#/main");
await main.waitForSelector(".topbar");
await sleep(1500);

// 2. first run → settings tab, "감시 중인 폴더 없음"
{
  const bar = await main.locator(".statusbar").innerText();
  const settingsOn = await main.locator("button.tab.on").innerText();
  // The startup "navigate: settings" event predates this page (bridge replays
  // only events after connect), so the tab itself is checked in the real window.
  // M8: before setup the status bar says watching starts after the wizard.
  ok(2, "첫 실행: 감시 전 (마법사 안내) 표시", bar.includes("마법사를 마치면"), `tab=${settingsOn} bar=${bar}`);
  await main.getByRole("button", { name: "설정", exact: true }).click();
  await sleep(800);
  await main.screenshot({ path: `${SHOT}/02-first-run.png` });
}

// 3. settings: add folder (prompt), append rule, save
{
  process.env.PROMPT_ANSWER = DESK;
  await main.getByRole("button", { name: "폴더 추가…" }).click();
  await sleep(300);
  const pathVal = await main.locator("input.wide").first().inputValue();
  ok(3, "폴더 추가 → 경로 행 생성", pathVal === DESK, pathVal);
  await openAdvanced(main);
  const ta = main.locator("textarea.rules");
  const cur = await ta.inputValue();
  // Prepended: rules are first-match, and the example already has an exe rule.
  await ta.fill(
    // M8: no example rules are applied any more; the txt rule is explicit.
    `[[rule]]\nname = "설치 확인"\nmatch = { ext = ["exe"] }\ndest = '${T}/설치 파일'\nconfirm = true\n\n` +
      `[[rule]]\nname = "Tidy 테스트"\nmatch = { ext = ["txt"], name_regex = "^tidy-test" }\ndest = "~/Documents/Tidy/테스트 결과/{yyyy}/{mm}"\n\n` +
      cur,
  );
  await sleep(800);
  const check = await main.locator(".settings .ok.small").innerText().catch(() => "");
  ok(3, "규칙 검사 표시", check.includes("설치 확인"), check);
  await main.getByRole("button", { name: "저장", exact: true }).click();
  await sleep(2500);
  const bar = await main.locator(".statusbar").innerText();
  const msg = await main.locator(".savebar .ok").innerText().catch(() => "");
  ok(3, "저장 → 상태바에 감시 폴더", bar.includes("바탕 화면"), `bar=${bar} msg=${msg}`);
  await main.screenshot({ path: `${SHOT}/03-settings-saved.png` });
}

// 4. auto move of a txt (example rule "문서": ext txt → ~/문서/{yyyy} ...): use whatever the example says
{
  await main.getByRole("button", { name: "활동" }).click();
  fs.writeFileSync(path.join(DESK, "tidy-test 보안점검.txt"), "수탁자 보안점검 결과");
  await sleep(5000);
  const feed = await main.locator(".activity").innerText();
  const moved = feed.includes("이동") && feed.includes("보안점검");
  ok(4, "txt 자동 이동이 활동 탭에 표시", moved, feed.split("\n").slice(0, 6).join(" | "));
  await main.screenshot({ path: `${SHOT}/04-activity-moved.png` });
}

// 5. search window
let movedPath = null;
{
  const s = await ctx.newPage();
  await s.goto("http://localhost:1420/#/search");
  await s.waitForSelector(".search-input");
  await s.locator(".search-input").fill("보안점검");
  await sleep(1500);
  const names = await s.locator(".results li").allInnerTexts();
  const hit = names.find((n) => n.includes("보안점검"));
  const marks = await s.locator(".results li mark").count();
  ok(5, "검색 결과 + 하이라이트", !!hit && marks > 0, `hits=${names.length} marks=${marks} ${hit ?? ""}`);
  movedPath = (await s.locator(".preview .muted").first().innerText()).trim();
  ok(5, "결과 경로가 새 위치", movedPath.includes("보안점검") && !movedPath.startsWith(DESK), movedPath);
  await s.locator(".search-input").press("Escape");
  await sleep(300);
  const cleared = await s.locator(".search-input").inputValue();
  ok(5, "Esc → 검색어 지움", cleared === "", `"${cleared}"`);
  await s.screenshot({ path: `${SHOT}/05-search.png` });
  await s.close();
}

// 6. confirm popup for an .exe
{
  const c = await ctx.newPage();
  c.on("dialog", (d) => d.accept(process.env.PROMPT_ANSWER ?? ""));
  await c.goto("http://localhost:1420/#/confirm");
  await sleep(800);
  fs.writeFileSync(path.join(DESK, "setup 1.exe"), "");
  await sleep(5000);
  const txt = await c.locator(".confirm").innerText();
  ok(6, "팝업에 setup 1.exe + 제안 위치", txt.includes("setup 1.exe") && txt.includes("설치 파일"), txt.replace(/\n/g, " | ").slice(0, 160));
  const st = await bridge("get_status");
  ok(6, "pendingCount = 1", st.pendingCount === 1, `pendingCount=${st.pendingCount}`);
  await c.screenshot({ path: `${SHOT}/06-confirm.png` });
  // 이동
  await c.getByRole("button", { name: "이동", exact: true }).click();
  await sleep(2500);
  const movedExe = fs.existsSync(path.join(T, "설치 파일", "setup 1.exe"));
  ok(6, "이동 → 설치 파일/setup 1.exe", movedExe);
  // 건너뜀
  fs.writeFileSync(path.join(DESK, "setup 2.exe"), "");
  await sleep(4000);
  await c.getByRole("button", { name: "건너뜀", exact: true }).click();
  await sleep(2000);
  ok(6, "건너뜀 → 파일 제자리", fs.existsSync(path.join(DESK, "setup 2.exe")));
  // 다른 곳…
  fs.writeFileSync(path.join(DESK, "setup 3.exe"), "");
  await sleep(4000);
  process.env.PROMPT_ANSWER = path.join(T, "다른 곳");
  await c.getByRole("button", { name: "다른 곳…", exact: true }).click();
  await sleep(2500);
  ok(6, "다른 곳… → 다른 곳/setup 3.exe", fs.existsSync(path.join(T, "다른 곳", "setup 3.exe")));
  // 무시
  fs.writeFileSync(path.join(DESK, "setup 4.exe"), "");
  await sleep(4000);
  await c.getByRole("button", { name: "무시", exact: true }).click();
  await sleep(2000);
  const st2 = await bridge("get_status");
  ok(6, "무시 → 대기 0, 파일 제자리", st2.pendingCount === 0 && fs.existsSync(path.join(DESK, "setup 4.exe")), `pending=${st2.pendingCount}`);
  await c.close();
}

// 7. quiet mode + queue bulk actions
{
  await main.getByRole("button", { name: "설정", exact: true }).click();
  await sleep(500);
  await main.locator("label.row", { hasText: "조용 모드" }).locator("input[type=checkbox]").check();
  await main.getByRole("button", { name: "저장", exact: true }).click();
  await sleep(2500);
  for (const n of [5, 6, 7]) fs.writeFileSync(path.join(DESK, `setup ${n}.exe`), "");
  await sleep(5000);
  await main.getByRole("button", { name: "대기열" }).click();
  await sleep(1000);
  const rows = main.locator(".pending tbody tr");
  ok(7, "대기열 3건", (await rows.count()) === 3, `rows=${await rows.count()}`);
  await main.screenshot({ path: `${SHOT}/07-pending.png` });
  await rows.nth(0).locator("input[type=checkbox]").check();
  await rows.nth(1).locator("input[type=checkbox]").check();
  await main.getByRole("button", { name: "제안 위치로 이동" }).click();
  await sleep(2500);
  const left = await main.locator(".pending tbody tr").count();
  const movedN = ["5", "6", "7"].filter((n) => fs.existsSync(path.join(T, "설치 파일", `setup ${n}.exe`))).length;
  ok(7, "2건 일괄 이동", left === 1 && movedN === 2, `left=${left} moved=${movedN}`);
  await main.locator(".pending tbody tr").first().getByRole("button", { name: "건너뜀", exact: true }).click();
  await sleep(1500);
  ok(7, "1건 건너뜀 → 대기열 비움", (await main.locator(".pending tbody tr").count()) === 0);
}

// 8. undo last
{
  await main.getByRole("button", { name: "활동" }).click();
  await main.getByRole("button", { name: "마지막 이동 되돌리기" }).click();
  await sleep(2500);
  const notice = await main.locator(".activity .notice").innerText().catch(() => "");
  const back = ["5", "6", "7"].filter((n) => fs.existsSync(path.join(DESK, `setup ${n}.exe`))).length;
  const inst = ["5", "6", "7"].filter((n) => fs.existsSync(path.join(T, "설치 파일", `setup ${n}.exe`))).length;
  ok(8, "되돌리기 → 일괄 이동 2건 모두 복귀", notice.startsWith("2건") && inst === 0, `notice=${notice} back=${back} left_in_dest=${inst}`);
  const s = await ctx.newPage();
  await s.goto("http://localhost:1420/#/search");
  await s.locator(".search-input").fill("setup 5");
  await sleep(1500);
  const p = (await s.locator(".preview .muted").first().innerText().catch(() => "")).trim();
  ok(8, "검색에 원래 경로", p.startsWith(DESK), p);
  await s.close();
}

// 9. pause / resume
{
  await main.getByRole("button", { name: "일시정지" }).click();
  await sleep(1000);
  let bar = await main.locator(".statusbar").innerText();
  ok(9, "일시정지 표시", bar.includes("일시정지"), bar);
  fs.writeFileSync(path.join(DESK, "paused-note.txt"), "x");
  await sleep(4000);
  ok(9, "일시정지 중 파일 미이동", fs.existsSync(path.join(DESK, "paused-note.txt")));
  await main.getByRole("button", { name: "감시 재개" }).click();
  await sleep(1000);
  bar = await main.locator(".statusbar").innerText();
  ok(9, "재개 표시", !bar.includes("일시정지"), bar);
}

// 11. comments preserved + bad hotkey
{
  await main.getByRole("button", { name: "설정", exact: true }).click();
  await sleep(500);
  await openAdvanced(main);
  const ta = main.locator("textarea.rules");
  await ta.fill("# 내가 쓴 주석\n" + (await ta.inputValue()));
  await main.getByRole("button", { name: "저장", exact: true }).click();
  await sleep(2500);
  await main.reload();
  await main.waitForSelector(".topbar");
  await main.getByRole("button", { name: "설정", exact: true }).click();
  await sleep(1000);
  await openAdvanced(main);
  const after = await main.locator("textarea.rules").inputValue();
  ok(11, "주석 보존", after.startsWith("# 내가 쓴 주석"), after.split("\n")[0]);
  const cfg = fs.readFileSync(path.join(T, "config.toml"), "utf8");
  ok(11, "config.toml 생성됨", cfg.includes("바탕 화면"), cfg.split("\n").slice(0, 4).join(" | "));
  const hk = main.locator("label.row", { hasText: "검색 단축키" }).locator("input");
  await hk.fill("Not+A+Key");
  await main.getByRole("button", { name: "저장", exact: true }).click();
  await sleep(2500);
  const bar = await main.locator(".statusbar").innerText();
  const err = await main.locator(".savebar .error").innerText().catch(() => "");
  ok(11, "잘못된 단축키 → 등록 실패 표시", (bar + err).includes("실패") || (bar + err).includes("단축키"), `bar=${bar} err=${err}`);
  await main.screenshot({ path: `${SHOT}/11-hotkey.png` });
}

fs.writeFileSync(path.join(process.env.S, "m3-results.json"), JSON.stringify(results, null, 2));
await browser.close();
const fails = results.filter((r) => !r.pass).length;
console.log(`\n${results.length - fails}/${results.length} passed`);
