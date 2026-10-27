// M14 GUI test: duplicate handling + hwp/hwpx body search (SPEC §14.9),
// driven through the dev bridge. "휴지통" is a plain folder
// (TIDY_E2E_TRASH = $T/휴지통) and "기존 파일 열기"/"휴지통 열기" are
// recorded instead of opened (TIDY_E2E_NO_OPEN), both set by run-headless.sh.
//
//   A. settings → 감시 폴더: "옮길 자리에 똑같은 파일이 이미 있으면" card
//      (3 choices, ⓘ) → rename → saved to config.toml
//   B. the same file arriving twice under each policy: skip (stays, activity
//      "똑같은 파일이 이미 …에 있어 그대로 둠" + 기존 파일 열기), rename
//      (보고서 (1).pdf), replace (old copy → trash); same name with other
//      content → always a number, nothing trashed
//   C. 확인 대기: "똑같은 파일이 이미 있습니다" + 3 buttons on that row only
//      (no 이동 button), also in the confirm popup; 그대로 두기 / 번호 붙여
//      이동 / 기존 것 휴지통 후 이동 each do their job
//   D. 정리하기 → 리포트: duplicate groups incl. a copy in 정리 위치 (kept by
//      default), "모든 그룹에 기본 선택 적용", change the kept file →
//      미리보기 (nothing moved yet) → 적용 → N files in the trash
//   E. 되돌리기: batch "중복 정리(휴지통)", trash rows, undo → "휴지통에서
//      복원하세요" + 휴지통 열기
//   F. hwp / hwpx body text is searchable (search window)
//
// Run: S=/tmp/tidy-e2e bash e2e/run-headless.sh m14
import { chromium } from "playwright";
import fs from "node:fs";
import path from "node:path";
import { addWatchFolder, bridge, gotoSettings, openToml, saveSettings, sleep, tab, waitFor } from "./lib.mjs";

const T = process.env.T;
const S = process.env.S;
const CFG = path.join(T, "config.toml");
const DESK = path.join(T, "바탕 화면");
const ORG = path.join(T, "정리");
const TRASH = path.join(T, "휴지통");
const SHOT = path.join(S, "shots");
const FIX = path.join(import.meta.dirname, "..", "..", "..", "crates", "tidy-core", "tests", "fixtures");
for (const d of [SHOT, DESK, path.join(ORG, "문서"), path.join(ORG, "메모"), path.join(ORG, "보관")]) fs.mkdirSync(d, { recursive: true });

const results = [];
const ok = (step, what, pass, note = "") => {
  results.push({ step, what, pass, note });
  console.log(`${pass ? "PASS" : "FAIL"} [${step}] ${what} ${note}`);
};
const exists = (p) => fs.existsSync(p);
const trashCount = () => (exists(TRASH) ? fs.readdirSync(TRASH).length : 0);
const opened = async () => (await bridge("debug_opened")) ?? [];

// Organized copies that are already there.
fs.writeFileSync(path.join(ORG, "문서", "보고서.pdf"), "SAME REPORT");
fs.writeFileSync(path.join(ORG, "메모", "회의.txt"), "회의 메모");
fs.writeFileSync(path.join(ORG, "메모", "안건.txt"), "안건 목록");
fs.writeFileSync(path.join(ORG, "메모", "결과.txt"), "결과 정리");

const RULES_TEXT =
  `[[rule]]\nname = "문서"\nmatch = { ext = ["pdf"] }\ndest = "문서"\n\n` +
  `[[rule]]\nname = "메모"\nmatch = { ext = ["txt"] }\ndest = "메모"\nconfirm = true\n`;

const browser = await chromium.launch();
const ctx = await browser.newContext({ viewport: { width: 1280, height: 960 } });
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

const setPolicy = async (value) => {
  await gotoSettings(main, "watch");
  await main.locator(`[data-testid=duplicate-policy] input[value=${value}]`).check();
  await saveSettings(main, 3500);
};
/** Wait until the watcher is running again after a save. */
const watching = async () => {
  await waitFor(async () => (await bridge("get_status")).watching.length > 0, 15000);
  await sleep(1500);
};

// A. settings card
{
  await gotoSettings(main, "watch");
  await addWatchFolder(main, DESK);
  process.env.PROMPT_ANSWER = ORG;
  await main.locator("[data-testid=common-root]").getByRole("button", { name: "찾아보기…" }).click();
  await sleep(500);
  const ta = await openToml(main);
  await ta.fill(RULES_TEXT);
  await sleep(800);
  await saveSettings(main, 3500);
  await gotoSettings(main, "watch");
  const card = main.locator("[data-testid=duplicate-policy]");
  const text = await card.innerText();
  ok(
    "A",
    "카드: '옮길 자리에 똑같은 파일이 이미 있으면' + 선택 3개 (기본 건너뜀)",
    text.includes("옮길 자리에 똑같은 파일이 이미 있으면") &&
      text.includes("그대로 두고 건너뜀") &&
      text.includes("번호를 붙여 둘 다 보관") &&
      text.includes("기존 것을 휴지통으로 보내고 새것으로") &&
      (await card.locator("input[value=skip]").isChecked()),
    text.replace(/\n/g, " | "),
  );
  const help = await card.locator("[data-help=duplicatePolicy] .help-i").getAttribute("title");
  ok("A", "ⓘ: 이름만 같고 내용이 다르면 언제나 번호", (help ?? "").includes("이름만 같고 내용이 다르면 언제나 번호를 붙입니다"), help ?? "");
  await main.screenshot({ path: `${SHOT}/m14-A-settings.png` });
  await card.screenshot({ path: `${SHOT}/m14-A-card.png` });
  await setPolicy("rename");
  ok("A", "저장 → config.toml [mover] duplicate_policy = \"rename\"", /\[mover\][\s\S]*duplicate_policy = "rename"/.test(fs.readFileSync(CFG, "utf8")));
  const s = await bridge("get_settings");
  ok("A", "설정 다시 읽기: rename", s.duplicatePolicy === "rename");
}

// B. the same file twice, three policies
{
  const desk = path.join(DESK, "보고서.pdf");
  const doc = (n) => path.join(ORG, "문서", n);

  await setPolicy("skip");
  await watching();
  fs.writeFileSync(desk, "SAME REPORT");
  await sleep(6000);
  await tab(main, "홈").click();
  const all = main.locator("[data-testid=activity-all]");
  if (!(await all.evaluate((d) => d.open))) await all.locator("summary").click();
  const row = main.locator("[data-testid=activity-feed] tr.duplicate").first();
  const shown = await waitFor(async () => (await row.count()) > 0, 10000);
  const rowText = shown ? await row.innerText() : "";
  ok("B", "건너뜀: 파일은 바탕화면에 그대로", exists(desk) && fs.readdirSync(path.join(ORG, "문서")).length === 1);
  ok(
    "B",
    "활동: '똑같은 파일이 이미 …\\문서\\에 있어 그대로 둠' + 기존 파일 열기",
    rowText.includes("똑같은 파일이 이미") && rowText.includes(`${path.join(ORG, "문서")}${path.sep}에 있어 그대로 둠`) && rowText.includes("기존 파일 열기"),
    rowText.replace(/\s+/g, " "),
  );
  if (shown) {
    await row.locator("[data-testid=open-existing]").click();
    await sleep(300);
    ok("B", "기존 파일 열기 → 탐색기에서 그 파일", (await opened()).includes(`reveal:${doc("보고서.pdf")}`));
  }
  await main.screenshot({ path: `${SHOT}/m14-B-activity.png` });
  fs.rmSync(desk);

  await setPolicy("rename");
  await watching();
  fs.writeFileSync(desk, "SAME REPORT");
  const renamed = await waitFor(() => exists(doc("보고서 (1).pdf")) && !exists(desk), 15000);
  ok("B", "번호: 보고서 (1).pdf 로 둘 다 보관", renamed && fs.readFileSync(doc("보고서.pdf"), "utf8") === "SAME REPORT");

  await setPolicy("replace");
  await watching();
  // The watcher ignores events on a path Tidy itself just vacated (10 s).
  await sleep(10000);
  fs.writeFileSync(desk, "SAME REPORT");
  const replaced = await waitFor(() => !exists(desk) && trashCount() === 1, 15000);
  ok("B", "휴지통: 기존 보고서.pdf → 휴지통, 새것이 그 자리", replaced && exists(doc("보고서.pdf")) && !exists(doc("보고서 (2).pdf")), `trash=${trashCount()}`);
  await sleep(10000);
  fs.writeFileSync(desk, "OTHER CONTENT");
  const numbered = await waitFor(() => exists(doc("보고서 (2).pdf")) && !exists(desk), 15000);
  ok("B", "이름만 같고 내용이 다르면 휴지통 정책에서도 번호 (덮어쓰기·휴지통 없음)", numbered && trashCount() === 1 && fs.readFileSync(doc("보고서.pdf"), "utf8") === "SAME REPORT");
  await setPolicy("skip");
  await watching();
}

// C. 확인 대기: the three answers
{
  const memo = (n) => path.join(ORG, "메모", n);
  for (const [n, body] of [
    ["회의.txt", "회의 메모"],
    ["안건.txt", "안건 목록"],
    ["결과.txt", "결과 정리"],
  ])
    fs.writeFileSync(path.join(DESK, n), body);
  const queued = await waitFor(async () => (await bridge("list_pending")).filter((p) => p.duplicateOf).length === 3, 20000);
  const items = await bridge("list_pending");
  ok("C", "확인 대기 3건, 모두 duplicateOf = 정리 위치의 같은 파일", queued && items.every((p) => p.duplicateOf === memo(p.name)), JSON.stringify(items.map((p) => [p.name, p.duplicateOf])));

  // Confirm popup shows the same notice.
  const c = await ctx.newPage();
  await c.setViewportSize({ width: 480, height: 420 });
  await c.goto("http://localhost:1420/#/confirm");
  const pop = await waitFor(async () => (await c.locator("[data-testid=dup-notice]").count()) === 1, 10000);
  const popText = pop ? await c.locator("[data-testid=dup-notice]").innerText() : "";
  ok(
    "C",
    "팝업: '똑같은 파일이 이미 있습니다' + 세 버튼, 이동 버튼 없음",
    pop &&
      popText.includes("똑같은 파일이 이미 있습니다") &&
      ["그대로 두기", "번호 붙여 이동", "기존 것 휴지통 후 이동"].every((b) => popText.includes(b)) &&
      (await c.locator(".ic-actions").getByRole("button", { name: "이동", exact: true }).count()) === 0,
    popText.replace(/\n/g, " | "),
  );
  if (pop) await c.screenshot({ path: `${SHOT}/m14-C-popup.png` });
  await c.close();

  await tab(main, "확인 대기").click();
  await main.waitForSelector("[data-testid=pending-table]");
  await sleep(500);
  const rowOf = (name) => main.locator("[data-testid=pending-row]").filter({ hasText: name });
  const r1 = rowOf("회의.txt");
  ok(
    "C",
    "표: 그 행에만 세 버튼, 이동 버튼 대신",
    (await main.locator("[data-testid=dup-notice]").count()) === 3 &&
      (await r1.getByRole("button", { name: "이동", exact: true }).count()) === 0 &&
      (await r1.getByRole("button", { name: "그대로 두기" }).count()) === 1,
  );
  await main.screenshot({ path: `${SHOT}/m14-C-pending.png` });

  await r1.getByRole("button", { name: "그대로 두기" }).click();
  await sleep(1500);
  const res = await main.locator("[data-testid=pending-result]").innerText().catch(() => "");
  ok("C", "그대로 두기: 파일은 그대로, 결과 '똑같은 파일이 이미 …에 있어 그대로 둠' + 기존 파일 열기", exists(path.join(DESK, "회의.txt")) && res.includes("똑같은 파일이 이미") && res.includes("그대로 둠") && res.includes("기존 파일 열기"), res);
  ok("C", "그대로 두기: 확인 대기에서 빠짐", !(await bridge("list_pending")).some((p) => p.name === "회의.txt"));

  await rowOf("안건.txt").getByRole("button", { name: "번호 붙여 이동" }).click();
  await sleep(1500);
  ok("C", "번호 붙여 이동: 안건 (1).txt", exists(memo("안건 (1).txt")) && exists(memo("안건.txt")) && !exists(path.join(DESK, "안건.txt")));

  const before = trashCount();
  await rowOf("결과.txt").getByRole("button", { name: "기존 것 휴지통 후 이동" }).click();
  await sleep(1500);
  ok("C", "기존 것 휴지통 후 이동: 휴지통 +1, 새것이 결과.txt", trashCount() === before + 1 && exists(memo("결과.txt")) && !exists(memo("결과 (1).txt")) && !exists(path.join(DESK, "결과.txt")), `trash ${before}→${trashCount()}`);
}

// D. report duplicate groups → keep / trash
{
  const X = Buffer.alloc(3000, 7);
  const Y = Buffer.alloc(5000, 9);
  fs.mkdirSync(path.join(DESK, "옛 자료"), { recursive: true });
  const g1 = [path.join(DESK, "사진 a.dat"), path.join(DESK, "사진 a 사본.dat"), path.join(ORG, "보관", "사진 a.dat")];
  const g2old = path.join(DESK, "옛 자료", "계약 b.dat");
  const g2new = path.join(DESK, "계약 b 최종.dat");
  for (const p of g1) fs.writeFileSync(p, X);
  fs.writeFileSync(g2old, Y);
  fs.writeFileSync(g2new, Y);
  const OLD = new Date("2025-01-02T10:00:00");
  fs.utimesSync(g2old, OLD, OLD);
  await sleep(2500);

  await tab(main, "정리하기").click();
  await main.waitForSelector("[data-testid=report-folder]");
  await main.locator("[data-testid=report-folder]").selectOption(DESK);
  await main.getByRole("button", { name: "리포트 만들기" }).click();
  const groups = main.locator("[data-testid=dup-group]");
  const built = await waitFor(async () => (await groups.count()) === 2, 30000);
  ok("D", "중복 그룹 2개 (정리 위치의 사본 포함)", built, `groups=${await groups.count()}`);
  const keptPath = async (g) => {
    const files = groups.nth(g).locator("[data-testid=dup-file]");
    for (let i = 0; i < (await files.count()); i++)
      if (await files.nth(i).locator("[data-testid=dup-keep]").isChecked()) return files.nth(i).locator("[data-testid=path-text]").getAttribute("data-path");
    return null;
  };
  const gi = async (needle) => {
    for (let i = 0; i < (await groups.count()); i++) if ((await groups.nth(i).innerText()).includes(needle)) return i;
    return -1;
  };
  const a = await gi("사진 a");
  const b = await gi("계약 b");
  ok("D", "기본 남김: 정리 위치 안 파일 (배지 '정리 위치')", (await keptPath(a)) === g1[2] && (await groups.nth(a).innerText()).includes("정리 위치"), String(await keptPath(a)));
  ok("D", "기본 남김: 가장 오래된 파일", (await keptPath(b)) === g2old, String(await keptPath(b)));

  await main.locator("[data-testid=dup-all]").check();
  await sleep(200);
  ok("D", "모든 그룹에 기본 선택 적용 → 두 그룹 모두 체크", (await main.locator("[data-testid=dup-pick]:checked").count()) === 2);
  // Keep the newer copy of group b instead.
  const bFiles = groups.nth(b).locator("[data-testid=dup-file]");
  for (let i = 0; i < (await bFiles.count()); i++)
    if ((await bFiles.nth(i).locator("[data-testid=path-text]").getAttribute("data-path")) === g2new) await bFiles.nth(i).locator("[data-testid=dup-keep]").check();
  ok("D", "남길 파일을 바꾸면 '모든 그룹에 기본 선택'이 풀림", !(await main.locator("[data-testid=dup-all]").isChecked()));
  await main.locator("[data-testid=dup-preview]").click();
  await sleep(300);
  const pv = await main.locator("[data-testid=dup-preview-table]").innerText().catch(() => "");
  ok("D", "미리보기: 휴지통으로 3개 · 용량 절약", pv.includes("휴지통으로 3개") && pv.includes("용량 절약") && (await main.locator("[data-testid=dup-preview-row]").count()) === 2, pv.split("\n")[0]);
  ok("D", "미리보기까지는 아무것도 옮기지 않음", [...g1, g2old, g2new].every(exists));
  await main.screenshot({ path: `${SHOT}/m14-D-preview.png`, fullPage: true });
  const before = trashCount();
  await main.locator("[data-testid=dup-apply]").click();
  const done = await waitFor(async () => (await main.locator("[data-testid=dup-result]").count()) === 1, 15000);
  const rt = done ? await main.locator("[data-testid=dup-result]").innerText() : "";
  ok("D", "적용: '3개를 휴지통으로 보냈습니다' + 휴지통 폴더에 3개", done && rt.includes("3개를 휴지통으로") && trashCount() === before + 3, `${rt} trash ${before}→${trashCount()}`);
  ok("D", "남긴 파일은 그대로, 나머지는 사라짐", exists(g1[2]) && exists(g2new) && !exists(g1[0]) && !exists(g1[1]) && !exists(g2old));
  const regrouped = await waitFor(async () => (await groups.count()) === 0, 30000);
  ok("D", "리포트 다시 만들기: 중복 없음", regrouped);
  await main.screenshot({ path: `${SHOT}/m14-D-applied.png` });
}

// E. 되돌리기: trash rows, undo points at the trash
{
  await tab(main, "되돌리기").click();
  const first = main.locator("[data-testid=batch-row]").first();
  await waitFor(async () => (await first.count()) === 1, 10000);
  ok("E", "맨 위 묶음: '중복 정리(휴지통)' 3개", (await first.innerText()).includes("중복 정리(휴지통)") && (await first.innerText()).includes("3개"), (await first.innerText()).replace(/\n/g, " | "));
  await first.getByRole("button", { name: "파일 보기" }).click();
  await sleep(500);
  const rows = main.locator("[data-testid=trash-row]");
  ok("E", "파일 보기: 🗑 … → 휴지통 (남긴 사본: …) 3줄", (await rows.count()) === 3 && (await rows.first().innerText()).includes("남긴 사본"));
  dialogs.length = 0;
  await first.getByRole("button", { name: "되돌리기" }).click();
  await sleep(1500);
  const msg = await main.locator("[data-testid=history-msg]").innerText().catch(() => "");
  ok("E", "되돌리기: Tidy가 되살릴 수 없다는 확인 → '휴지통에서 복원하세요' + 휴지통 열기", dialogs.some((d) => d.includes("되살릴 수 없습니다")) && msg.includes("휴지통에서 복원하세요") && msg.includes("휴지통 열기"), msg);
  await main.getByRole("button", { name: "휴지통 열기" }).click();
  await sleep(300);
  ok("E", "휴지통 열기 → 기록됨", (await opened()).includes("trash"));
  await main.screenshot({ path: `${SHOT}/m14-E-history.png` });
  const u = await bridge("undo_last");
  // The batch before it is C's "기존 것 휴지통 후 이동": the move comes back,
  // its trashed copy is pointed at.
  ok("E", "마지막 이동 되돌리기는 중복 정리 묶음을 건너뜀 (그 전 '휴지통 후 이동'을 되돌리고 휴지통 사본은 안내)", !!u && u.restored.length === 1 && u.restored[0][1].endsWith("결과.txt") && !u.inTrash.some((p) => p.includes(".dat")), JSON.stringify(u ?? null));
}

// F. hwp / hwpx search
{
  fs.copyFileSync(path.join(FIX, "한글 문서.hwp"), path.join(DESK, "점검 보고.hwp"));
  fs.copyFileSync(path.join(FIX, "한글 문서.hwpx"), path.join(DESK, "사업 계획.hwpx"));
  const indexed = await waitFor(async () => {
    const r = await bridge("search", { query: "응답 시간", limit: 10, customer: null });
    return r.some((h) => (h.path ?? "").includes("점검 보고.hwp"));
  }, 30000, 1000);
  const s = await ctx.newPage();
  await s.goto("http://localhost:1420/#/search");
  await s.waitForSelector(".search-input");
  const find = async (q, name) => {
    await s.locator(".search-input").fill(q);
    await sleep(1500);
    const names = await s.locator(".results li").allInnerTexts();
    return names.some((n) => n.includes(name)) ? names.find((n) => n.includes(name)) : `(없음: ${names.join(" / ").slice(0, 120)})`;
  };
  const h1 = await find("응답 시간", "점검 보고.hwp");
  ok("F", "hwp 본문 '응답 시간'(표 안) → 점검 보고.hwp", indexed && !h1.startsWith("(없음"), h1.replace(/\n/g, " "));
  const h2 = await find("정보보호팀", "사업 계획.hwpx");
  ok("F", "hwpx 본문 '정보보호팀'(표 안) → 사업 계획.hwpx", !h2.startsWith("(없음"), h2.replace(/\n/g, " "));
  await s.screenshot({ path: `${SHOT}/m14-F-search.png` });
  await s.close();
}

const failed = results.filter((r) => !r.pass);
console.log(`\n${results.length - failed.length}/${results.length} passed`);
fs.writeFileSync(path.join(S, "m14-results.json"), JSON.stringify(results, null, 2));
await browser.close();
process.exit(failed.length ? 1 : 0);
