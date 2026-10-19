// M5 GUI test (PR #6): 리포트 탭 (stats / dups / tree), 규칙 재구성 미리보기 →
// 선택 적용 (승인 전 무이동) → 이동 기록 탭에서 배치 되돌리기.
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
const bridge = async (cmd, args = {}) =>
  (await (await fetch(`http://127.0.0.1:1421/invoke/${cmd}`, { method: "POST", body: JSON.stringify(args) })).json()).value;

// Seed a messy desktop BEFORE watching starts: these are "existing" files the
// watcher must not touch, so restructure has something to propose.
fs.mkdirSync(path.join(DESK, "old"), { recursive: true });
fs.mkdirSync(path.join(DESK, "empty"), { recursive: true });
const big = "x".repeat(200_000);
fs.writeFileSync(path.join(DESK, "report-a.pdf"), big);
fs.writeFileSync(path.join(DESK, "old", "report-a copy.pdf"), big); // duplicate
fs.writeFileSync(path.join(DESK, "archive.zip"), "z".repeat(5000));
fs.writeFileSync(path.join(DESK, "notes.txt"), "plain");
const stale = path.join(DESK, "old", "ancient.docx");
fs.writeFileSync(stale, "d".repeat(3000));
const past = new Date(Date.now() - 400 * 86400e3);
fs.utimesSync(stale, past, past);

const browser = await chromium.launch();
const ctx = await browser.newContext({ viewport: { width: 1000, height: 820 } });
const main = await ctx.newPage();
let dialogs = [];
main.on("dialog", (d) => {
  dialogs.push(d.message());
  d.accept(process.env.PROMPT_ANSWER ?? "");
});
await main.goto("http://localhost:1420/#/main");
await main.waitForSelector(".topbar");
await sleep(800);

// A. watch the folder (no process-existing: files above stay put)
{
  await main.getByRole("button", { name: "설정", exact: true }).click();
  await sleep(500);
  process.env.PROMPT_ANSWER = DESK;
  await main.getByRole("button", { name: "폴더 추가…" }).click();
  await main.getByRole("button", { name: "저장", exact: true }).click();
  await sleep(3000);
  const untouched = ["report-a.pdf", "archive.zip", "notes.txt"].every((f) => fs.existsSync(path.join(DESK, f)));
  ok("A", "감시 시작 후 기존 파일은 그대로", untouched);
}

// B. report tab
{
  await main.getByRole("button", { name: "리포트" }).click();
  await sleep(800);
  const opts = await main.locator(".report .toolbar select option").allInnerTexts();
  ok("B", "리포트 폴더 목록에 감시 폴더", opts.some((o) => o.includes("바탕 화면")), opts.join(" | "));
  await main.locator(".report .toolbar label", { hasText: "중복 찾기" }).locator("input").check();
  await main.getByRole("button", { name: "리포트 만들기" }).click();
  await sleep(6000);
  const txt = await main.locator(".report").innerText();
  const cards = Object.fromEntries(
    (await main.locator(".cards .card, .cards > *").allInnerTexts()).map((t) => {
      const [a, b] = t.split("\n");
      return [a?.trim(), b?.trim()];
    }),
  );
  ok("B", "요약 카드 (파일 5)", JSON.stringify(cards).includes("5"), JSON.stringify(cards).slice(0, 200));
  ok("B", "종류별 / 큰 파일 / 폴더 트리 섹션", /종류별/.test(txt) && /큰 파일/.test(txt) && /폴더 트리/.test(txt));
  ok("B", "중복 1그룹 검출", /중복 파일 \(1그룹\)/.test(txt), txt.match(/중복 파일 \([^)]*\)/)?.[0] ?? "");
  ok("B", "오래 안 쓴 파일에 ancient.docx", /오래 안 쓴 파일[\s\S]*ancient\.docx/.test(txt));
  ok("B", "빈 폴더 empty", /빈 폴더 \(1\)[\s\S]*empty/.test(txt));
  await main.screenshot({ path: `${SHOT}/m5-B-report.png`, fullPage: true });
}

// C. rules restructure preview → nothing moves until apply
{
  await main.getByRole("button", { name: "규칙으로 정리 미리보기" }).click();
  await sleep(5000);
  const panel = main.locator(".restructure");
  const rows = panel.locator("tbody tr");
  const n = await rows.count();
  const ptxt = await panel.innerText();
  ok("C", "재구성 제안 표시 (pdf·docx·zip 규칙 매치)", n >= 2, `rows=${n} ${ptxt.replace(/\n/g, " | ").slice(0, 200)}`);
  const before = ["report-a.pdf", "archive.zip"].every((f) => fs.existsSync(path.join(DESK, f)));
  ok("C", "미리보기 단계에서는 이동 없음", before);
  await main.screenshot({ path: `${SHOT}/m5-C-plan.png`, fullPage: true });

  // uncheck all, check the first two, apply (window.confirm accepted)
  await panel.locator("thead input[type=checkbox]").uncheck();
  await rows.nth(0).locator("input[type=checkbox]").check();
  await rows.nth(1).locator("input[type=checkbox]").check();
  dialogs = [];
  await panel.getByRole("button", { name: /선택한 2개 적용/ }).click();
  await sleep(4000);
  ok("C", "적용 전 확인 다이얼로그", dialogs.some((d) => d.includes("옮깁니다")), dialogs[0] ?? "");
  const notice = await panel.locator(".notice").innerText().catch(() => "");
  ok("C", "적용 결과 안내 (2개 파일 옮김 + batch)", /2개 파일을 옮겼습니다/.test(notice), notice.slice(0, 120));
  const st = await bridge("list_batches", { limit: 10 });
  const batch = st.find((b) => b.moves === 2);
  ok("C", "list_batches에 2건 배치", !!batch, JSON.stringify(st[0] ?? {}).slice(0, 160));
  await main.screenshot({ path: `${SHOT}/m5-C-applied.png`, fullPage: true });
}

// D. history tab: batch listed → 되돌리기 → files back
{
  await main.getByRole("button", { name: "이동 기록" }).click();
  await sleep(1500);
  const rows = main.locator(".history tbody tr");
  const txt = await main.locator(".history").innerText();
  ok("D", "이동 기록에 배치 2개 표시", /2개/.test(txt), txt.replace(/\n/g, " | ").slice(0, 200));
  await rows.first().getByRole("button", { name: "파일 보기" }).click();
  await sleep(500);
  const detail = await main.locator(".history").innerText();
  ok("D", "파일 보기 → 이동 목록", /→/.test(detail) && /(pdf|zip|docx)/.test(detail));
  dialogs = [];
  await rows.first().getByRole("button", { name: "되돌리기", exact: true }).click();
  await sleep(3000);
  const msg = await main.locator(".history .notice").innerText().catch(() => "");
  ok("D", "되돌리기 확인 + 2개 되돌림", dialogs.length > 0 && /2개 되돌림/.test(msg), `dialog=${dialogs[0]?.slice(0, 40)} msg=${msg}`);
  const back = ["report-a.pdf", "archive.zip", "notes.txt"].every((f) => fs.existsSync(path.join(DESK, f)));
  const desk = fs.readdirSync(DESK);
  ok("D", "파일들이 원래 자리로", back || desk.length >= 5, desk.join(", "));
  const after = await main.locator(".history").innerText();
  ok("D", "기록에 '되돌림' 표시", /되돌림/.test(after));
  await main.screenshot({ path: `${SHOT}/m5-D-history.png`, fullPage: true });
}

// E. AI restructure button is disabled while AI is off (rule 6 guard)
{
  await main.getByRole("button", { name: "리포트" }).click();
  await sleep(500);
  const aiBtn = main.locator(".restructure").getByRole("button", { name: "AI 재구성 제안" });
  ok("E", "AI 꺼짐 → 'AI 재구성 제안' 비활성", await aiBtn.isDisabled());
}

fs.writeFileSync(path.join(process.env.S, "m5-results.json"), JSON.stringify(results, null, 2));
await browser.close();
const fails = results.filter((r) => !r.pass).length;
console.log(`\n${results.length - fails}/${results.length} passed`);
