// M13 GUI test: screen overhaul (SPEC §14.8), driven through the dev bridge.
// Links and "open folder" are recorded instead of opened (TIDY_E2E_NO_OPEN,
// set by run-headless.sh).
//
//   A. settings: 6 pages routed as #/main/settings/<key> (direct URL, menu,
//      reload), left menu ≥ 900 px, horizontal tabs below
//   B. watch folders: 4 cards with default aliases, one edit panel at a time,
//      alias / recursive / exclude chip → save → config.toml; common 정리
//      위치 → every folder collects there
//   C. rules: one-line rows, one open at a time; absolute destination with a
//      common root → ⚠ line + "상대 경로로" → relative → rules.toml
//   D. status bar "감시 4곳 → 정리 위치 <PathText>"; PathText middle ellipsis,
//      full path in the title, click opens the folder
//   E. home: 4 folder cards in one row (4 columns) → 지금 정리 → 정리하기 with
//      the rules preview (nothing moves); 오늘 한 일 after a move
//   F. 확인 대기: table at 1280, cards at 760; the ⋯ menu's 5 items each do
//      their job; the confirm popup uses the same card + menu
//   G. 되돌리기: bulk-undo card first; 정리하기 folder dropdown shows aliases
//   H. minimum window 960×640 (tauri.conf.json) and no horizontal scroll on
//      any tab at that size
//
// Run: S=/tmp/tidy-e2e bash e2e/run-headless.sh m13
import { chromium } from "playwright";
import fs from "node:fs";
import path from "node:path";
import { addWatchFolder, bridge, gotoSettings, moreItem, openRule, openToml, openWatchEdit, ruleCard, saveSettings, sleep, statusText, tab, waitFor, watchCard } from "./lib.mjs";

const T = process.env.T;
const S = process.env.S;
const CFG = path.join(T, "config.toml");
const RULES = path.join(path.dirname(CFG), "rules.toml");
const DESK = path.join(T, "바탕 화면");
const DOWN = path.join(T, "Downloads");
const WORK = path.join(T, "업무자료");
const LONG = path.join(T, "아주 긴 경로", "부서 공유 폴더", "2026년 하반기", "프로젝트_백업_2026년_10월_최종본");
const ORG = path.join(T, "정리");
const OTHER = path.join(T, "다른 곳");
const ABS = path.join(T, "압축 보관");
const SHOT = path.join(S, "shots");
for (const d of [SHOT, DESK, DOWN, WORK, LONG, OTHER, path.join(WORK, "개인")]) fs.mkdirSync(d, { recursive: true });

const results = [];
const ok = (step, what, pass, note = "") => {
  results.push({ step, what, pass, note });
  console.log(`${pass ? "PASS" : "FAIL"} [${step}] ${what} ${note}`);
};

// Files that are there before watching (the home cards' backlog).
fs.writeFileSync(path.join(DESK, "보고서 A.pdf"), "a");
fs.writeFileSync(path.join(DESK, "보고서 B.pdf"), "b");
fs.writeFileSync(path.join(DOWN, "자료.zip"), "zip");
fs.writeFileSync(path.join(WORK, "계약서.pdf"), "c");
fs.writeFileSync(path.join(LONG, "백업 목록.pdf"), "d");

const RULES_TEXT =
  `[[rule]]\nname = "문서"\nmatch = { ext = ["pdf"] }\ndest = "문서"\n\n` +
  `[[rule]]\nname = "설치 확인"\nmatch = { ext = ["exe"] }\ndest = "설치"\nconfirm = true\n\n` +
  `[[rule]]\nname = "압축"\nmatch = { ext = ["zip"] }\ndest = '${ABS}'\n`;

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
const opened = async () => (await bridge("debug_opened")) ?? [];
const noHScroll = () => main.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1);

// A. settings routes
{
  const keys = ["watch", "rules", "context", "ai", "security", "advanced"];
  const direct = [];
  for (const k of keys) {
    await main.goto(`http://localhost:1420/#/main/settings/${k}`);
    await sleep(600);
    direct.push((await main.locator(`[data-testid=settings-${k}]`).count()) === 1 && (await main.locator(`[data-testid=settings-nav-${k}]`).getAttribute("aria-current")) === "page");
  }
  ok("A", "주소 #/main/settings/<key> 6개 → 해당 페이지 + 메뉴 표시", direct.every(Boolean), direct.join(","));
  await gotoSettings(main, "rules");
  ok("A", "메뉴 클릭 → 주소 바뀜", main.url().endsWith("#/main/settings/rules"), main.url());
  await main.reload();
  await main.waitForSelector(".topbar");
  await sleep(800);
  ok("A", "새로고침해도 같은 페이지", (await main.locator("[data-testid=settings-rules]").count()) === 1);
  ok("A", "설정 화면엔 상태바 없음 (한 화면 한 작업)", (await main.locator("[data-testid=statusbar]").count()) === 0);
  const wide = await main.locator(".settings-nav").boundingBox();
  ok("A", "1280: 왼쪽 세로 메뉴", wide && wide.height > wide.width, JSON.stringify(wide));
  await main.setViewportSize({ width: 760, height: 900 });
  await sleep(500);
  const narrow = await main.locator(".settings-nav").boundingBox();
  ok("A", "760: 위쪽 가로 탭", narrow && narrow.width > narrow.height * 3, JSON.stringify(narrow));
  ok("A", "760: 가로 스크롤 없음", await noHScroll());
  await main.screenshot({ path: `${SHOT}/m13-A-settings-narrow.png` });
  await main.setViewportSize({ width: 1280, height: 900 });
  await sleep(300);
}

// B. watch folders: aliases, edit panel, common root, save → config
{
  await gotoSettings(main, "watch");
  for (const d of [DESK, DOWN, WORK, LONG]) await addWatchFolder(main, d);
  const cards = main.locator("[data-testid=watch-card]");
  ok("B", "감시 폴더 카드 4장", (await cards.count()) === 4);
  ok("B", "새로 추가한 폴더는 편집이 펼쳐짐 (한 번에 하나)", (await main.locator("[data-testid=watch-card][data-open=true]").count()) === 1 && (await watchCard(main, 3).getAttribute("data-open")) === "true");
  const c0 = await openWatchEdit(main, 0);
  ok("B", "다른 카드 편집 → 앞의 편집은 접힘", (await main.locator("[data-testid=watch-card][data-open=true]").count()) === 1 && (await c0.getAttribute("data-path")) === DESK);
  await c0.getByRole("button", { name: "접기" }).click();
  await sleep(200);
  const aliases = await main.locator("[data-testid=watch-alias]").allInnerTexts();
  ok("B", "기본 별칭: 바탕화면 · 다운로드 · 업무자료 · 마지막 폴더 이름", aliases.join(",") === "바탕화면,다운로드,업무자료,프로젝트_백업_2026년_10월_최종본", aliases.join(","));

  const w = await openWatchEdit(main, 2);
  await w.getByRole("textbox", { name: "별칭" }).fill("업무");
  await w.getByRole("checkbox", { name: /하위 폴더 안의 파일도 정리/ }).check();
  process.env.PROMPT_ANSWER = path.join(WORK, "개인");
  await w.getByRole("button", { name: "+ 추가" }).click();
  await sleep(400);
  const chip = await w.locator("[data-testid=exclude-row] .chip").innerText().catch(() => "");
  ok("B", "제외 칩 추가 (상대 이름으로 표시)", chip.includes("개인"), chip);
  await main.screenshot({ path: `${SHOT}/m13-B-watch-edit.png`, fullPage: true });

  process.env.PROMPT_ANSWER = ORG;
  await main.locator("[data-testid=common-root]").getByRole("button", { name: "찾아보기…" }).click();
  await sleep(500);
  ok("B", "공통 정리 위치 표시", (await main.locator("[data-testid=common-root-value]").innerText()) === ORG);
  const sums = await main.locator("[data-testid=watch-summary-line]").allInnerTexts();
  ok("B", "접힌 카드 요약: … · 정리 위치 <공통> · 제외 없음", sums.length === 3 && sums.every((t) => t.includes(`정리 위치 ${ORG}`) && t.includes("제외 없음")), sums.join(" | "));

  const ta = await openToml(main);
  await ta.fill(RULES_TEXT);
  await sleep(1200);
  await saveSettings(main, 3000);
  const cfg = fs.readFileSync(CFG, "utf8");
  const blocks = cfg.split("[[watch]]").slice(1);
  ok("B", "config: [[watch]] 4개, 모두 collect → 정리", blocks.length === 4 && blocks.every((b) => /mode = "collect"/.test(b) && b.includes(ORG)), `${blocks.length}`);
  const work = blocks.find((b) => b.includes(WORK)) ?? "";
  ok("B", "config: 업무자료 alias = \"업무\", recursive, exclude 개인", /alias = "업무"/.test(work) && /recursive = true/.test(work) && work.includes(path.join(WORK, "개인")), work.replace(/\n/g, " | ").slice(0, 300));
  ok("B", "config: 별칭을 안 바꾼 폴더엔 alias 없음", blocks.filter((b) => /alias =/.test(b)).length === 1);
  await gotoSettings(main, "watch");
  const aliases2 = await main.locator("[data-testid=watch-alias]").allInnerTexts();
  ok("B", "저장 후 별칭 '업무'", aliases2[2] === "업무", aliases2.join(","));
  await main.screenshot({ path: `${SHOT}/m13-B-watch.png`, fullPage: true });
}

// C. rules: accordion + absolute destination warning
{
  await gotoSettings(main, "rules");
  const rows = main.locator("[data-testid=rule-card]");
  ok("C", "규칙 3개, 모두 한 줄로 접힘", (await rows.count()) === 3 && (await main.locator("[data-testid=rule-card][data-open=true]").count()) === 0);
  await openRule(main, 0);
  await openRule(main, 1);
  const open = await rows.evaluateAll((els) => els.map((e) => e.getAttribute("data-open")));
  ok("C", "아코디언: 하나를 펼치면 다른 하나는 접힘", open.join(",") === "false,true,false", open.join(","));
  await ruleCard(main, 1).getByRole("button", { name: "접기", exact: true }).click();
  await sleep(200);
  const line = ruleCard(main, 2).locator("[data-testid=abs-dest-line]");
  const lt = await line.innerText().catch(() => "");
  ok("C", "절대 경로 규칙: 접힌 줄에 ⚠ (주황)", (await line.count()) === 1 && lt.includes("⚠"), lt);
  const color = await line.evaluate((e) => getComputedStyle(e).color).catch(() => "");
  ok("C", "⚠ 색 = 주황 토큰", /rgb\((1[6-9]\d|2\d\d), (6\d|7\d|8\d|9\d|1[0-2]\d), \d{1,2}\)/.test(color), color);
  const warn = await main.locator("[data-testid=abs-rules-warning]").innerText().catch(() => "");
  ok("C", "저장바: 규칙 1개가 정리 위치를 무시", warn.includes("규칙 1개"), warn);
  await main.screenshot({ path: `${SHOT}/m13-C-rules.png`, fullPage: true });
  await ruleCard(main, 2).locator("[data-testid=make-relative]").click();
  await sleep(800);
  ok("C", "상대 경로로 → ⚠ 사라짐", (await line.count()) === 0);
  await saveSettings(main, 3000);
  const r = fs.readFileSync(RULES, "utf8");
  ok("C", "rules.toml: dest = \"압축 보관\"", r.includes('dest = "압축 보관"'), r.split("\n").filter((l) => l.startsWith("dest")).join(" | "));
}

// D. status bar + PathText
{
  const bar = await statusText(main);
  ok("D", "상태바: 감시 4곳 → 정리 위치", bar.includes("감시 4곳") && bar.includes("정리 위치"), bar.replace(/\n/g, " | "));
  const sp = main.locator("[data-testid=watch-summary] [data-testid=path-text]");
  ok("D", "상태바 정리 위치 = PathText(전체 경로는 title)", (await sp.getAttribute("data-path")) === ORG && (await sp.getAttribute("title")) === ORG);
  const longCard = main.locator(`[data-testid=backlog-folder][data-path="${LONG}"]`);
  await waitFor(async () => (await longCard.count()) === 1, 20000);
  const pt = longCard.locator("[data-testid=path-text]");
  const shown = await pt.innerText();
  ok("D", "긴 경로: 가운데 줄임(…) + 끝 폴더 이름 보임", shown.includes("…") && shown.endsWith("최종본") && shown.length < LONG.length, shown);
  ok("D", "긴 경로: title = 전체 경로", (await pt.getAttribute("title")) === LONG);
  await pt.click();
  await sleep(500);
  ok("D", "PathText 클릭 → 폴더 열기", (await opened()).at(-1) === LONG, (await opened()).at(-1));
}

// E. home: 4 folder cards → 지금 정리
{
  const cards = main.locator("[data-testid=backlog-folder]");
  await waitFor(async () => (await main.locator("[data-testid=backlog-count]").count()) === 4, 20000);
  const boxes = [];
  for (let i = 0; i < 4; i++) boxes.push(await cards.nth(i).boundingBox());
  ok("E", "1280: 폴더 카드 4장 한 줄 (4열)", boxes.every((b) => b && Math.abs(b.y - boxes[0].y) < 2) && boxes[3].x > boxes[2].x, boxes.map((b) => Math.round(b?.x ?? -1)).join(","));
  const al = await cards.locator(".alias").allInnerTexts();
  ok("E", "카드 별칭: 바탕화면 · 다운로드 · 업무 · …", al[0] === "바탕화면" && al[1] === "다운로드" && al[2] === "업무", al.join(","));
  const counts = await main.locator("[data-testid=backlog-count]").allInnerTexts();
  ok("E", "미정리 수: 2 · 1 · 1 · 1", counts.map((c) => c.replace(/[^\d]/g, "")).join(",") === "2,1,1,1", counts.join(" | ").replace(/\n/g, " "));
  ok("E", "오늘 한 일 · 고객사별 보기 카드", (await main.locator("[data-testid=today-card]").count()) === 1);
  ok("E", "전체 활동은 접힌 채 아래", !(await main.locator("[data-testid=activity-all]").evaluate((d) => d.open)));
  await main.screenshot({ path: `${SHOT}/m13-E-home.png` });
  await main.setViewportSize({ width: 760, height: 900 });
  await sleep(500);
  const b0 = await cards.nth(0).boundingBox();
  const b2 = await cards.nth(2).boundingBox();
  ok("E", "760: 카드가 다음 줄로 (열 수 줄어듦)", b0 && b2 && b2.y > b0.y + 10, `${Math.round(b0?.y ?? 0)} / ${Math.round(b2?.y ?? 0)}`);
  ok("E", "760: 가로 스크롤 없음", await noHScroll());
  await main.screenshot({ path: `${SHOT}/m13-E-home-narrow.png`, fullPage: true });
  await main.setViewportSize({ width: 1280, height: 900 });
  await sleep(300);

  await main.locator(`[data-testid=backlog-folder][data-path="${DESK}"]`).getByRole("button", { name: "지금 정리" }).click();
  const planned = await waitFor(async () => (await main.locator("[data-testid=plan-item]").count()) === 2, 15000);
  ok("E", "지금 정리 → 정리하기 탭 + 규칙 미리보기 2개", planned && (await tab(main, "정리하기").getAttribute("aria-current")) === "page");
  ok("E", "정리할 폴더 = 바탕 화면 (드롭다운에 별칭)", (await main.locator("[data-testid=report-folder]").inputValue()) === DESK && (await main.locator("[data-testid=report-folder] option:checked").innerText()).startsWith("바탕화면"));
  ok("E", "미리보기만으로는 무이동", fs.existsSync(path.join(DESK, "보고서 A.pdf")));
  const dest = await main.locator("[data-testid=plan-item]").first().locator("[data-testid=path-text]").last().getAttribute("data-path");
  ok("E", "옮길 곳 = 정리/문서/…", dest?.startsWith(path.join(ORG, "문서")), dest);
  await main.screenshot({ path: `${SHOT}/m13-E-report.png` });
}

// F. 확인 대기: table ↔ cards, the 5-item ⋯ menu, popup
{
  const names = ["a-건너뜀.exe", "b-무시.exe", "c-다른곳.exe", "d-이름.exe", "e-대기.exe"];
  for (const n of names) fs.writeFileSync(path.join(DESK, n), n);
  const queued = await waitFor(async () => (await bridge("list_pending")).length === 5, 20000);
  ok("F", "옮기기 전에 묻기 규칙 → 대기 5건", queued, `${(await bridge("list_pending")).length}`);
  await tab(main, "확인 대기").click();
  await sleep(1000);
  ok("F", "1280: 5열 표", (await main.locator("[data-testid=pending-table]").count()) === 1 && (await main.locator("[data-testid=pending-row]").count()) === 5 && (await main.locator("[data-testid=pending-cards]").count()) === 0);
  const cols = await main.locator("[data-testid=pending-table] .t-head > span").count();
  ok("F", "표 머리 5칸 (선택·파일·제안 위치·제안·확신도·동작)", cols === 5, `${cols}`);
  ok("F", "툴바: 대기 5건 · 선택 0", (await main.locator("[data-testid=pending-count]").innerText()) === "대기 5건 · 선택 0");
  await main.screenshot({ path: `${SHOT}/m13-F-pending-wide.png` });
  await main.setViewportSize({ width: 760, height: 900 });
  await sleep(600);
  ok("F", "760: 카드로 바뀜", (await main.locator("[data-testid=pending-cards]").count()) === 1 && (await main.locator("[data-testid=pending-card]").count()) === 5 && (await main.locator("[data-testid=pending-table]").count()) === 0);
  ok("F", "760: 가로 스크롤 없음", await noHScroll());
  await main.screenshot({ path: `${SHOT}/m13-F-pending-narrow.png` });
  await main.setViewportSize({ width: 1280, height: 900 });
  await sleep(600);

  const row = (n) => main.locator("[data-testid=pending-row]", { hasText: n });
  await row(names[0]).getByRole("button", { name: "다른 선택" }).click();
  await sleep(200);
  const items = await main.getByRole("menuitem").allInnerTexts();
  ok("F", "⋯ 메뉴 5개", items.join("|") === "다른 곳으로…|이름 바꿔 이동|건너뜀(이번만)|무시(앞으로 묻지 않음)|대기열에 두기", items.join("|"));
  await main.keyboard.press("Escape");
  await sleep(200);
  const pendingNames = async () => (await bridge("list_pending")).map((p) => p.name);

  await moreItem(row(names[0]), "건너뜀(이번만)");
  await waitFor(async () => !(await pendingNames()).includes(names[0]), 5000);
  ok("F", "건너뜀(이번만) → 대기열에서 빠지고 파일은 그대로", !(await pendingNames()).includes(names[0]) && fs.existsSync(path.join(DESK, names[0])));

  await moreItem(row(names[1]), "무시(앞으로 묻지 않음)");
  await waitFor(async () => !(await pendingNames()).includes(names[1]), 5000);
  ok("F", "무시 → 대기열에서 빠지고 파일은 그대로", !(await pendingNames()).includes(names[1]) && fs.existsSync(path.join(DESK, names[1])));

  process.env.PROMPT_ANSWER = OTHER;
  await moreItem(row(names[2]), "다른 곳으로…");
  const other = await waitFor(() => fs.existsSync(path.join(OTHER, names[2])), 8000);
  ok("F", "다른 곳으로… → 고른 폴더로 이동", other);

  await moreItem(row(names[3]), "이름 바꿔 이동");
  await sleep(300);
  await row(names[3]).getByRole("textbox", { name: "새 이름" }).fill("설치파일-새이름.exe");
  await row(names[3]).getByRole("button", { name: "확인" }).click();
  const renamed = await waitFor(() => fs.existsSync(path.join(ORG, "설치", "설치파일-새이름.exe")), 8000);
  ok("F", "이름 바꿔 이동 → 제안 위치에 새 이름으로", renamed);

  await moreItem(row(names[4]), "대기열에 두기");
  await sleep(800);
  ok("F", "대기열에 두기 → 그대로 남음", (await pendingNames()).includes(names[4]) && (await main.locator(".pending-page .notice").innerText().catch(() => "")).includes("그대로"));

  const c = await ctx.newPage();
  await c.setViewportSize({ width: 460, height: 440 });
  await c.goto("http://localhost:1420/#/confirm");
  await c.waitForSelector("[data-testid=pending-card]");
  await sleep(400);
  ok("F", "확인 팝업 = 같은 카드 (이동 · 다른 곳… · ⋯)", (await c.locator("[data-testid=pending-card]").count()) === 1 && (await c.getByRole("button", { name: "다른 곳…" }).count()) === 1);
  await c.getByRole("button", { name: "다른 선택" }).click();
  await sleep(200);
  const pItems = await c.getByRole("menuitem").allInnerTexts();
  ok("F", "팝업 ⋯ 메뉴도 5개", pItems.length === 5 && pItems.includes("대기열에 두기"), pItems.join("|"));
  await c.screenshot({ path: `${SHOT}/m13-F-popup.png` });
  await c.keyboard.press("Escape");
  ok("F", "팝업 가로 스크롤 없음 (460)", await c.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1));
  await c.close();

  await gotoSettings(main, "advanced");
  const ign = await main.locator("[data-testid=ignored-files]").innerText();
  ok("F", "설정 → 고급 '무시한 파일'에 표시", ign.includes(names[1]), ign.replace(/\n/g, " | ").slice(0, 160));
}

// G. 되돌리기 + 오늘 한 일
{
  await tab(main, "되돌리기").click();
  await sleep(1000);
  const first = await main.locator(".page > .card, .page > section").first().getAttribute("data-testid");
  ok("G", "되돌리기: 일괄 되돌리기 카드가 맨 위", first === "bulk-undo", first);
  ok("G", "이동 기록 행", (await main.locator("[data-testid=batch-row]").count()) >= 2);
  await main.screenshot({ path: `${SHOT}/m13-G-history.png` });
  await tab(main, "홈").click();
  await sleep(1200);
  const today = await main.locator("[data-testid=today-counts]").innerText();
  ok("G", "오늘 한 일: 이동 2 · 확인 대기 1", /이동 2/.test(today) && /확인 대기 1/.test(today), today);
  ok("G", "오늘 한 일 행 표시", (await main.locator("[data-testid=today-row]").count()) >= 2);
}

// H. minimum window size
{
  const conf = JSON.parse(fs.readFileSync(path.join(import.meta.dirname, "..", "src-tauri", "tauri.conf.json"), "utf8"));
  const mw = conf.app.windows.find((w) => w.label === "main");
  ok("H", "tauri.conf.json: 최소 960×640", mw?.minWidth === 960 && mw?.minHeight === 640, `${mw?.minWidth}×${mw?.minHeight}`);
  await main.setViewportSize({ width: 960, height: 640 });
  await sleep(500);
  const bad = [];
  for (const t of ["홈", "확인 대기", "정리하기", "되돌리기"]) {
    await tab(main, t).click();
    await sleep(600);
    if (!(await noHScroll())) bad.push(t);
  }
  for (const k of ["watch", "rules", "context", "ai", "security", "advanced"]) {
    await gotoSettings(main, k);
    if (!(await noHScroll())) bad.push(`설정/${k}`);
  }
  ok("H", "960×640: 모든 탭·설정 페이지 가로 스크롤 없음", bad.length === 0, bad.join(","));
  await tab(main, "홈").click();
  await sleep(500);
  const nav = await main.locator(".topbar nav").boundingBox();
  ok("H", "960: 탭 5개 한 줄", nav && nav.height < 48, JSON.stringify(nav));
  await main.screenshot({ path: `${SHOT}/m13-H-min.png` });
}

const failed = results.filter((r) => !r.pass);
console.log(`\n${results.length - failed.length}/${results.length} passed`);
fs.writeFileSync(path.join(S, "m13-results.json"), JSON.stringify(results, null, 2));
await browser.close();
process.exit(failed.length ? 1 : 0);
