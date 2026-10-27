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
import { addWatchFolder, gotoSettings, moreItem, openToml, openWatchEdit, saveSettings, statusText, tab } from "./lib.mjs";
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
  const bar = await main.locator("[data-testid=statusbar]").innerText();
  const settingsOn = await main.locator("button.tab.on").innerText();
  // The startup "navigate: settings" event predates this page (bridge replays
  // only events after connect), so the tab itself is checked in the real window.
  // M8: before setup the status bar says watching starts after the wizard.
  ok(2, "첫 실행: 감시 전 (마법사 안내) 표시", bar.includes("마법사를 마치면"), `tab=${settingsOn} bar=${bar}`);
  await gotoSettings(main, "watch");
  await main.screenshot({ path: `${SHOT}/02-first-run.png` });
}

// 3. settings: add folder (prompt), append rule, save
{
  await addWatchFolder(main, DESK);
  const pathVal = await main.locator("[data-testid=watch-card]").first().getAttribute("data-path");
  ok(3, "폴더 추가 → 폴더 카드 생성", pathVal === DESK, pathVal);
  const ta = await openToml(main);
  const cur = await ta.inputValue();
  // Prepended: rules are first-match, and the example already has an exe rule.
  await ta.fill(
    // M8: no example rules are applied any more; the txt rule is explicit.
    `[[rule]]\nname = "설치 확인"\nmatch = { ext = ["exe"] }\ndest = '${T}/설치 파일'\nconfirm = true\n\n` +
      `[[rule]]\nname = "Tidy 테스트"\nmatch = { ext = ["txt"], name_regex = "^tidy-test" }\ndest = "~/Documents/Tidy/테스트 결과/{yyyy}/{mm}"\n\n` +
      `[[rule]]\nname = "바로가기 함정"\nmatch = { ext = ["lnk", "url"] }\ndest = '${T}/함정'\n\n` +
      cur,
  );
  await sleep(800);
  const check = await main.locator("[data-testid=rules-check]").innerText().catch(() => "");
  ok(3, "규칙 검사 표시", check.includes("설치 확인"), check);
  await saveSettings(main);
  const msg = await main.locator("[data-testid=save-message]").innerText().catch(() => "");
  const bar = await statusText(main);
  const title = (await main.locator("[data-testid=watch-summary]").getAttribute("title")) ?? "";
  ok(3, "저장 → 상태바에 감시 폴더", bar.includes("감시 1곳") && title.includes("바탕 화면"), `bar=${bar} msg=${msg}`);
  await main.screenshot({ path: `${SHOT}/03-settings-saved.png` });
}

// 4. auto move of a txt (example rule "문서": ext txt → ~/문서/{yyyy} ...): use whatever the example says
{
  await tab(main, "홈").click();
  fs.writeFileSync(path.join(DESK, "tidy-test 보안점검.txt"), "수탁자 보안점검 결과");
  await sleep(5000);
  const today = await main.locator("[data-testid=today-card]").innerText();
  await main.getByRole("button", { name: "전체 보기" }).click();
  await sleep(300);
  const feed = await main.locator("[data-testid=activity-feed]").innerText().catch(() => "");
  const moved = today.includes("보안점검") && /이동 1/.test(today) && feed.includes("이동") && feed.includes("보안점검");
  ok(4, "txt 자동 이동이 홈(오늘 한 일 + 전체 활동)에 표시", moved, today.split("\n").slice(0, 6).join(" | "));
  await main.screenshot({ path: `${SHOT}/04-activity-moved.png` });
  // 0.7.1: shortcuts stay put even though a rule matches .lnk/.url (keep_shortcuts on by default)
  fs.writeFileSync(path.join(DESK, "회사 포털.url"), "[InternetShortcut]\nURL=https://example.com\n");
  fs.writeFileSync(path.join(DESK, "한글.lnk"), "L");
  await sleep(5000);
  const kept = fs.existsSync(path.join(DESK, "회사 포털.url")) && fs.existsSync(path.join(DESK, "한글.lnk")) && !fs.existsSync(path.join(T, "함정"));
  ok(4, "바로가기(.url/.lnk)는 규칙이 있어도 제자리", kept);
  const cfgTxt = fs.readFileSync(path.join(T, "config.toml"), "utf8");
  ok(4, "config.toml keep_shortcuts = true", /keep_shortcuts *= *true/.test(cfgTxt));
  await gotoSettings(main, "advanced");
  const ks = main.locator("[data-testid=keep-shortcuts]");
  ok(4, "설정 고급에 '바로가기는 그대로 두기' 켜짐", await ks.isChecked());
  await ks.uncheck();
  await saveSettings(main);
  fs.writeFileSync(path.join(DESK, "회사 포털2.url"), "[InternetShortcut]\nURL=https://example.com\n");
  await sleep(5000);
  ok(4, "끄면 바로가기도 규칙대로 이동", fs.existsSync(path.join(T, "함정", "회사 포털2.url")));
  await gotoSettings(main, "advanced");
  await main.locator("[data-testid=keep-shortcuts]").check();
  await saveSettings(main);
  await tab(main, "홈").click();
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
  await moreItem(c, "건너뜀(이번만)");
  await sleep(2000);
  ok(6, "건너뜀 → 파일 제자리", fs.existsSync(path.join(DESK, "setup 2.exe")));
  // 다른 곳…
  fs.writeFileSync(path.join(DESK, "setup 3.exe"), "");
  await sleep(4000);
  process.env.PROMPT_ANSWER = path.join(T, "다른 곳");
  await c.getByRole("button", { name: "다른 곳…", exact: true }).click();
  await sleep(2500);
  ok(6, "다른 곳… → 다른 곳/setup 3.exe", fs.existsSync(path.join(T, "다른 곳", "setup 3.exe")));
  // 옮길 수 없는 곳(파일인 경로) → 오류가 팝업에 남고 항목은 대기열에 그대로
  fs.writeFileSync(path.join(DESK, "setup 3b.exe"), "");
  await sleep(4000);
  const blocker = path.join(T, "막힌 곳");
  fs.writeFileSync(blocker, "not a dir");
  process.env.PROMPT_ANSWER = blocker;
  await c.getByRole("button", { name: "다른 곳…", exact: true }).click();
  await sleep(2500);
  const errTxt = await c.locator(".confirm .error").innerText().catch(() => "");
  const stErr = await bridge("get_status");
  ok(6, "이동 실패 → 팝업에 이유 표시 + 대기열 유지", /옮기지 못했습니다|옮길 수 없습니다/.test(errTxt) && stErr.pendingCount === 1 && fs.existsSync(path.join(DESK, "setup 3b.exe")), `err="${errTxt.slice(0, 80)}" pending=${stErr.pendingCount}`);
  await c.screenshot({ path: `${SHOT}/06-confirm-error.png` });
  await moreItem(c, "건너뜀(이번만)");
  await sleep(2000);
  // 무시
  fs.writeFileSync(path.join(DESK, "setup 4.exe"), "");
  await sleep(4000);
  const hint = await c.locator(".confirm .hint").innerText().catch(() => "");
  ok(6, "팝업에 건너뜀/무시 차이 안내", /건너뜀 = 이번만/.test(hint) && /무시 = /.test(hint), hint);
  await moreItem(c, "무시(앞으로 묻지 않음)");
  await sleep(2000);
  const st2 = await bridge("get_status");
  ok(6, "무시 → 대기 0, 파일 제자리", st2.pendingCount === 0 && fs.existsSync(path.join(DESK, "setup 4.exe")), `pending=${st2.pendingCount}`);
  await c.close();
  // 무시한 파일은 설정 ⑥에 보이고, 해제하면 다시 정리 대상
  const ign = await bridge("list_ignored");
  ok(6, "무시 목록에 setup 4.exe", ign.some((p) => p.endsWith("setup 4.exe")), JSON.stringify(ign));
  await gotoSettings(main, "advanced");
  const box = main.locator("[data-testid=ignored-files]");
  ok(6, "설정 → 고급 무시한 파일 1개 표시", /1개/.test(await box.innerText()) && /setup 4\.exe/.test(await box.innerText()));
  await box.getByRole("button", { name: "해제" }).first().click();
  await sleep(500);
  ok(6, "해제 → 목록 비움", /없음/.test(await box.innerText()) && (await bridge("list_ignored")).length === 0);
}

// 7. quiet mode + queue bulk actions
{
  await gotoSettings(main, "advanced");
  await main.getByRole("checkbox", { name: /조용 모드/ }).check();
  await saveSettings(main);
  for (const n of [5, 6, 7]) fs.writeFileSync(path.join(DESK, `setup ${n}.exe`), "");
  await sleep(5000);
  await tab(main, "확인 대기").click();
  await sleep(1000);
  const rows = main.locator("[data-testid=pending-row]");
  ok(7, "대기열 3건", (await rows.count()) === 3, `rows=${await rows.count()}`);
  await main.screenshot({ path: `${SHOT}/07-pending.png` });
  await rows.nth(0).getByRole("checkbox", { name: "선택" }).check();
  await rows.nth(1).getByRole("checkbox", { name: "선택" }).check();
  const counter = await main.locator("[data-testid=pending-count]").innerText();
  ok(7, "상단 '대기 3건 · 선택 2'", counter === "대기 3건 · 선택 2", counter);
  await main.getByRole("button", { name: "선택한 항목 제안 위치로 이동" }).click();
  await sleep(2500);
  const left = await rows.count();
  const movedN = ["5", "6", "7"].filter((n) => fs.existsSync(path.join(T, "설치 파일", `setup ${n}.exe`))).length;
  ok(7, "2건 일괄 이동", left === 1 && movedN === 2, `left=${left} moved=${movedN}`);
  await moreItem(rows.first(), "건너뜀(이번만)");
  await sleep(1500);
  ok(7, "1건 건너뜀 → 대기열 비움", (await rows.count()) === 0);
}

// 7b. bulk undo of recent automatic moves (user-confirmed ones stay)
{
  // two files the txt rule moves automatically
  fs.writeFileSync(path.join(DESK, "tidy-test 자동1.txt"), "x");
  fs.writeFileSync(path.join(DESK, "tidy-test 자동2.txt"), "x");
  await sleep(5000);
  ok("7b", "규칙으로 자동 이동됨", !fs.existsSync(path.join(DESK, "tidy-test 자동1.txt")) && !fs.existsSync(path.join(DESK, "tidy-test 자동2.txt")));
  const userMoved = fs.existsSync(path.join(T, "설치 파일", "setup 5.exe"));
  await tab(main, "되돌리기").click();
  await sleep(1200);
  const bulk = main.locator("[data-testid=bulk-undo]");
  const btn = bulk.getByRole("button", { name: /되돌리기/ });
  const label = await btn.innerText();
  ok("7b", "최근 1시간 규칙·AI 이동 개수 표시", /규칙·AI 이동 \d+개 되돌리기/.test(label) && Number(label.match(/(\d+)개/)[1]) >= 2, label);
  await btn.click();
  await sleep(3000);
  const back = fs.existsSync(path.join(DESK, "tidy-test 자동1.txt")) && fs.existsSync(path.join(DESK, "tidy-test 자동2.txt"));
  ok("7b", "자동 이동 전부 되돌림", back);
  ok("7b", "직접 확인한 이동은 그대로", userMoved === fs.existsSync(path.join(T, "설치 파일", "setup 5.exe")));
  const after = await btn.innerText();
  ok("7b", "되돌린 뒤 개수 0", /0개/.test(after) || (await btn.isDisabled()), after);
  await main.screenshot({ path: `${SHOT}/07b-bulk-undo.png` });
}

// 8. undo last
{
  await tab(main, "홈").click();
  await main.getByRole("button", { name: "전체 보기" }).click();
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
  let bar = await main.locator("[data-testid=statusbar]").innerText();
  ok(9, "일시정지 표시", bar.includes("일시정지"), bar);
  fs.writeFileSync(path.join(DESK, "paused-note.txt"), "x");
  await sleep(4000);
  ok(9, "일시정지 중 파일 미이동", fs.existsSync(path.join(DESK, "paused-note.txt")));
  await main.getByRole("button", { name: "감시 재개" }).click();
  await sleep(1000);
  bar = await main.locator("[data-testid=statusbar]").innerText();
  ok(9, "재개 표시", !bar.includes("일시정지"), bar);
}

// 11. comments preserved + bad hotkey
{
  const ta = await openToml(main);
  await ta.fill("# 내가 쓴 주석\n" + (await ta.inputValue()));
  await saveSettings(main);
  await main.reload();
  await main.waitForSelector(".topbar");
  await sleep(1000);
  const after = await (await openToml(main)).inputValue();
  ok(11, "주석 보존", after.startsWith("# 내가 쓴 주석"), after.split("\n")[0]);
  const cfg = fs.readFileSync(path.join(T, "config.toml"), "utf8");
  ok(11, "config.toml 생성됨", cfg.includes("바탕 화면"), cfg.split("\n").slice(0, 4).join(" | "));
  await gotoSettings(main, "advanced");
  await main.getByRole("textbox", { name: "검색 단축키" }).fill("Not+A+Key");
  await saveSettings(main);
  const err = await main.locator("[data-testid=savebar] .error").innerText().catch(() => "");
  await main.screenshot({ path: `${SHOT}/11-hotkey.png` });
  const bar = await statusText(main);
  ok(11, "잘못된 단축키 → 등록 실패 표시", (bar + err).includes("실패") || (bar + err).includes("단축키"), `bar=${bar} err=${err}`);
}

// 12. exclude sub-folders (M10.5): files inside stay put, config has `exclude`, UI chip removes it
{
  await gotoSettings(main, "watch");
  const EX1 = path.join(DESK, "작업중");
  const EX2 = path.join(DESK, "보관");
  fs.mkdirSync(EX1, { recursive: true });
  fs.mkdirSync(EX2, { recursive: true });
  // recursive on so the sub-folder would otherwise be organized
  const card = await openWatchEdit(main, 0);
  await card.getByRole("checkbox", { name: /하위 폴더 안의 파일도 정리/ }).check();
  const exRow = card.locator("[data-testid=exclude-row]");
  process.env.PROMPT_ANSWER = EX1;
  await exRow.getByRole("button", { name: "+ 추가" }).click();
  await sleep(200);
  process.env.PROMPT_ANSWER = EX2;
  await exRow.getByRole("button", { name: "+ 추가" }).click();
  await sleep(200);
  ok(12, "제외 폴더 2개 칩 표시", (await exRow.locator(".chip").count()) === 2, await exRow.innerText());
  await saveSettings(main);
  const cfg = fs.readFileSync(path.join(T, "config.toml"), "utf8");
  ok(12, "config.toml에 exclude 두 항목", /exclude\s*=\s*\[[^\]]*작업중[^\]]*보관/.test(cfg), cfg.split("\n").filter((l) => /exclude/.test(l)).join(" | "));
  await tab(main, "홈").click();
  fs.writeFileSync(path.join(EX1, "tidy-test 제외.txt"), "x");
  fs.writeFileSync(path.join(EX2, "tidy-test 제외2.txt"), "x");
  fs.writeFileSync(path.join(DESK, "tidy-test 포함.txt"), "x");
  await sleep(5000);
  ok(12, "제외 폴더 안 파일은 그대로", fs.existsSync(path.join(EX1, "tidy-test 제외.txt")) && fs.existsSync(path.join(EX2, "tidy-test 제외2.txt")));
  ok(12, "제외 밖 파일은 이동", !fs.existsSync(path.join(DESK, "tidy-test 포함.txt")));
  // remove one via the chip's × and save → config keeps the other only
  await gotoSettings(main, "watch");
  const line = await main.locator("[data-testid=watch-summary-line]").first().innerText();
  ok(12, "폴더 카드 요약 줄 '하위 폴더 포함 · … · 제외 2개'", line.includes("하위 폴더 포함") && line.includes("제외 2개"), line);
  const card2 = await openWatchEdit(main, 0);
  await card2.getByRole("button", { name: `제외 해제 ${EX1}` }).click();
  await saveSettings(main);
  const cfg2 = fs.readFileSync(path.join(T, "config.toml"), "utf8");
  ok(12, "칩 × → 저장 → 하나만 남음", !/작업중/.test(cfg2) && /보관/.test(cfg2), cfg2.split("\n").filter((l) => /exclude/.test(l)).join(" | "));
  await main.screenshot({ path: `${SHOT}/12-exclude.png` });
}

fs.writeFileSync(path.join(process.env.S, "m3-results.json"), JSON.stringify(results, null, 2));
await browser.close();
const fails = results.filter((r) => !r.pass).length;
console.log(`\n${results.length - fails}/${results.length} passed`);
