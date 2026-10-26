// M12 GUI test: folder-unit work-context curation, path context, customer
// view (SPEC §14.7), driven through the dev bridge with the fake LLM
// (e2e/fake-llm.mjs, answers "[n] 폴더: …" lines with kind "folder").
// Links / "open folder" are recorded instead of opened (TIDY_E2E_NO_OPEN).
//
//   A. settings: desktop watched, collected into 정리; AI (fake); curate on
//   B. "바탕화면에서 뽑기": the first-level folder name "가온누리원 로그" is
//      sent as a customer hint → 가온누리원 goes into the dictionary
//   C. home: "새 폴더 1개 — 지금 정리" → 정리하기 + cost notice (폴더 1개)
//   D. preview: ONE folder row (📁, 파일 31개, inner tree) + top-level file
//      rows; the prompt carries paths; folder date editable; nothing moved
//   E. apply → 정리/가온누리원/날짜_로그분석/개발/가온누리원 로그/tools/go/…
//   F. 고객사별 보기 (click opens the folder) · search filter by customer ·
//      도움말 "고객사 자료는 어디 있나요?"
//   G. 되돌리기 → the folder is back on the desktop, all 30 .go files
//   H. "이 폴더는 파일별로 나눠서" → the folder's files become items
//   I. watching: a new file in a sub-folder (and a new folder) is never
//      moved; a new top-level file is curated; a folder still being copied
//      is left out; popup "대기열에 두기"
//
// Run: S=/tmp/tidy-e2e bash e2e/run-headless.sh m12
import { chromium } from "playwright";
import { spawn } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

const T = process.env.T;
const S = process.env.S;
const DESK = path.join(T, "바탕 화면");
const ORG = path.join(T, "정리");
const UNIT = path.join(DESK, "가온누리원 로그");
const GO = path.join(UNIT, "tools", "go");
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
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const bridge = async (cmd, args = {}) => {
  const r = await (await fetch(`http://127.0.0.1:1421/invoke/${cmd}`, { method: "POST", body: JSON.stringify(args) })).json();
  if (r.error) throw new Error(`${cmd}: ${r.error}`);
  return r.value;
};
const waitFor = async (fn, ms, step = 500) => {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    if (await fn()) return true;
    await sleep(step);
  }
  return false;
};
const stats = async () => (await fetch(`${LLM}/stats`)).json();
/** Every file under `dir` (relative, "/"-separated). */
const walk = (dir, base = dir) =>
  fs.existsSync(dir)
    ? fs.readdirSync(dir, { withFileTypes: true }).flatMap((e) => {
        const p = path.join(dir, e.name);
        return e.isDirectory() ? walk(p, base) : [path.relative(base, p).split(path.sep).join("/")];
      })
    : [];

// ── fake LLM (fast) ─────────────────────────────────────────────────────
const llm = spawn(process.execPath, [path.join(import.meta.dirname, "fake-llm.mjs"), String(LLM_PORT)], {
  stdio: ["ignore", "inherit", "inherit"],
  env: { ...process.env, FAKE_DELAY_MS: "50" },
});
process.on("exit", () => llm.kill());
await sleep(800);

// ── fixtures (before watching) ──────────────────────────────────────────
fs.mkdirSync(GO, { recursive: true });
fs.mkdirSync(ORG, { recursive: true });
fs.writeFileSync(path.join(UNIT, "README.md"), "가온누리원 선거 로그 분석 도구\n사용법: go run ./tools/go");
const GO_FILES = [];
for (let i = 1; i <= 30; i++) {
  const n = `f${String(i).padStart(2, "0")}.go`;
  fs.writeFileSync(path.join(GO, n), `package main // 누리원 로그 분석 도구 ${i}`);
  GO_FILES.push(n);
}
const TOP = ["누리원_회의록_20261021.txt", "GARA_과금_20261014_로그.log", "메모.txt"];
for (const n of TOP) fs.writeFileSync(path.join(DESK, n), `업무 자료: ${n}`);
// The folder's own date = its newest file (2026-10-20); folders too, so
// the unit is settled ("not being copied").
const OLD = new Date("2026-10-20T12:00:00");
for (const p of [...GO_FILES.map((n) => path.join(GO, n)), path.join(UNIT, "README.md"), GO, path.dirname(GO), UNIT])
  fs.utimesSync(p, OLD, OLD);
fs.writeFileSync(CONTEXT, '[[entity]]\nname = "가람정보"\naliases = ["GARA"]\n');

// ── browser ─────────────────────────────────────────────────────────────
const browser = await chromium.launch();
const ctx = await browser.newContext({ viewport: { width: 1280, height: 960 } });
await ctx.addInitScript(() =>
  localStorage.setItem("tidy.settings.sections", JSON.stringify({ watch: true, rules: false, context: true, ai: true, security: false, advanced: false })),
);
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
const tab = (name) => main.locator(".topbar nav").getByRole("button", { name });
const save = async () => {
  await main.getByRole("button", { name: "저장", exact: true }).click();
  await sleep(3000);
};
const panel = main.locator(".restructure");
const folderRow = () => panel.locator("[data-testid=context-row][data-kind=folder]");
const fileRows = () => panel.locator("[data-testid=context-row][data-kind=file]");

// A. settings
{
  await tab("설정").click();
  await sleep(800);
  process.env.PROMPT_ANSWER = DESK;
  await main.getByRole("button", { name: "폴더 추가…", exact: true }).click();
  await sleep(500);
  const row = main.locator(".settings tbody tr:not(.subrow)").first();
  await row.locator("select[aria-label='정리 방식']").selectOption("collect");
  await row.locator("input.organize-root").fill(ORG);
  const ai = main.locator("[data-testid=ai-section]");
  await ai.locator("label.row", { hasText: "AI 사용" }).locator("input[type=checkbox]").check();
  await ai.locator("select").first().selectOption("openai");
  await sleep(300);
  const arows = ai.locator("label.row");
  await arows.filter({ hasText: "모델" }).first().locator("input").fill("fake-slow");
  await arows.filter({ hasText: "주소" }).first().locator("input").fill(`${LLM}/v1`);
  await arows.filter({ hasText: "키 이름" }).first().locator("input").fill("");
  await save();
  const ready = await waitFor(async () => (await bridge("get_status")).aiState === "ready", 20000);
  ok("A", "AI 준비됨 (가짜 서버)", ready, (await bridge("get_status")).aiDetail ?? "");
  // Watch-time curation needs the AI on first.
  await main.locator("[data-testid=curate-enabled]").check();
  await save();
  const cfg = fs.readFileSync(CFG, "utf8");
  ok("A", "config: 바탕 화면 → 정리 (모으기), [curate] enabled", /mode = "collect"/.test(cfg) && /\[curate\][\s\S]*enabled = true/.test(cfg));
  ok("A", "config: folder_settle_secs 기본 60", !/folder_settle_secs/.test(cfg) || /folder_settle_secs = 60/.test(cfg));
}

// B. 바탕화면에서 뽑기: first-level folder names are customer hints
{
  await main.locator("[data-testid=extract-context]").click();
  const shown = await waitFor(async () => (await main.locator("[data-testid=candidates]").count()) === 1, 30000);
  const st = await stats();
  ok("B", "요청에 '감시 폴더 바로 아래 폴더: 가온누리원 로그'", st.lastUser.includes("## 감시 폴더 바로 아래 폴더") && st.lastUser.includes("- 가온누리원 로그"), st.lastUser.split("\n").slice(0, 3).join(" | "));
  const cand = main.locator("[data-testid=candidate-entity]").filter({ has: main.locator("input[value='가온누리원']") });
  ok("B", "후보: 가온누리원 (폴더 이름에서)", shown && (await cand.count()) === 1, (await main.locator("[data-testid=candidates]").innerText().catch(() => "")).replace(/\n/g, " | ").slice(0, 200));
  await main.screenshot({ path: `${SHOT}/m12-B-extract.png` });
  await main.getByRole("button", { name: "고른 것 사전에 넣기" }).click();
  await sleep(500);
  await save();
  ok("B", "context.toml에 가온누리원", fs.readFileSync(CONTEXT, "utf8").includes('name = "가온누리원"'));
}

// C. home: "새 폴더 1개 — 지금 정리"
{
  await bridge("watch_overview", { refresh: true });
  await tab("홈").click();
  const nf = main.locator("[data-testid=new-folders]");
  const shown = await waitFor(async () => (await nf.count()) === 1, 30000);
  ok("C", "홈: 새 폴더 1개 — 지금 정리", shown && (await nf.innerText()).includes("새 폴더 1개"), (await main.locator("[data-testid=organize-card]").innerText().catch(() => "")).replace(/\n/g, " | "));
  await main.screenshot({ path: `${SHOT}/m12-C-home.png` });
  dialogs.length = 0;
  await nf.getByRole("button", { name: "지금 정리" }).click();
  const planned = await waitFor(async () => (await panel.locator("[data-testid=context-plan]").count()) === 1, 60000);
  ok("C", "지금 정리 → 정리하기 탭 + 업무 맥락 미리보기", planned && ((await tab("정리하기").getAttribute("class")) ?? "").includes("on"));
  ok("C", "비용 안내: 항목 4개(폴더 1개는 통째로…)", dialogs.some((d) => d.includes("항목 4개") && d.includes("폴더 1개")), dialogs[0]?.replace(/\n/g, " ").slice(0, 120));
}

// D. preview: one folder item + file items
{
  ok("D", "폴더 항목 1줄 + 파일 항목 3줄", (await folderRow().count()) === 1 && (await fileRows().count()) === 3, `folder=${await folderRow().count()} files=${await fileRows().count()}`);
  const fr = folderRow();
  ok("D", "폴더 줄: 📁 가온누리원 로그 · 파일 31개", (await fr.innerText()).includes("📁") && (await fr.locator("[data-testid=folder-count]").innerText()).includes("파일 31개"), (await fr.locator("[data-testid=folder-count]").innerText()));
  const u = (await stats()).lastUser;
  ok("D", "프롬프트: 폴더 요약 줄 (경로·파일 수·확장자)", /\] 폴더: 가온누리원 로그 \| 경로: 가온누리원 로그 \| 파일 31개 .*확장자: go 30/.test(u), u.split("\n").find((l) => l.includes("] 폴더:"))?.slice(0, 160));
  ok("D", "프롬프트: 파일 줄에 폴더 경로", /\] 파일: 누리원_회의록_20261021\.txt \| 폴더: /.test(u));
  const dest = () => fr.locator("td.path").nth(1).getAttribute("title");
  const want = (d) => path.join(ORG, "가온누리원", `${d}_로그분석`, "개발", "가온누리원 로그");
  ok("D", "옮길 곳: 정리/가온누리원/20261020_로그분석/개발/가온누리원 로그 (날짜 = 최근 파일)", (await dest()) === want("20261020"), await dest());
  await fr.locator("[data-testid=folder-toggle]").click();
  const tree = await panel.locator("[data-testid=folder-inner] pre").innerText().catch(() => "");
  ok("D", "펼치면 안의 구조 (tools/go)", tree.includes("tools") && tree.includes("go"), tree.split("\n").slice(0, 4).join(" | "));
  await fr.locator("[data-testid=ctx-date]").fill("2026-10-19");
  const edited = await waitFor(async () => (await dest()) === want("20261019"), 10000);
  ok("D", "폴더 날짜 고치기 → 20261019_로그분석", edited, await dest());
  // Collected into 정리 (outside the desktop): the preview counts them.
  const out = await panel.locator(".plan .warn", { hasText: "밖으로 나가는 파일" }).innerText().catch(() => "");
  ok("D", "미리보기: 정리 위치로 나가는 파일 34개 (폴더 31 + 3)", out.includes("34개"), out);
  ok("D", "업무 맥락 정리에는 하위 폴더 포함이 쓰이지 않음 (툴팁)", ((await panel.locator("label", { hasText: "하위 폴더 포함" }).getAttribute("title")) ?? "").includes("업무 맥락 정리는 하위 폴더를 통째로"));
  ok("D", "미리보기만으로 무이동", GO_FILES.every((n) => fs.existsSync(path.join(GO, n))));
  await main.screenshot({ path: `${SHOT}/m12-D-preview.png`, fullPage: true });
}

// E. apply: the folder keeps its inner structure under the work context
const DEST = path.join(ORG, "가온누리원", "20261019_로그분석", "개발", "가온누리원 로그");
{
  dialogs.length = 0;
  await panel.getByRole("button", { name: /선택한 \d+개 적용/ }).click();
  const moved = await waitFor(async () => (await panel.locator(".notice", { hasText: "옮겼습니다" }).count()) === 1, 30000);
  const inner = walk(DEST);
  ok("E", "적용 → …/개발/가온누리원 로그/tools/go/*.go 30개 + README", moved && inner.filter((p) => /^tools\/go\/f\d\d\.go$/.test(p)).length === 30 && inner.includes("README.md"), `${inner.length} files`);
  ok("E", "바탕 화면의 폴더는 비워져 정리됨", !fs.existsSync(UNIT));
  const n = (await panel.locator(".notice", { hasText: "옮겼습니다" }).innerText()).match(/(\d+)개 파일/)?.[1];
  ok("E", "한 배치로 34개 파일 이동 (폴더 31 + 파일 3)", n === "34", n);
  ok("E", "최상위 파일: 누리원_회의록 → 가온누리원, GARA → 가람정보", walk(path.join(ORG, "가온누리원")).some((p) => p.endsWith("누리원_회의록_20261021.txt")) && walk(path.join(ORG, "가람정보")).some((p) => p.endsWith("GARA_과금_20261014_로그.log")));
  await main.screenshot({ path: `${SHOT}/m12-E-applied.png` });
}

// F. 고객사별 보기 · search filter · help
{
  await tab("홈").click();
  const card = main.locator("[data-testid=customer-card]");
  await waitFor(async () => (await card.count()) === 1, 15000);
  const row = card.locator("[data-testid=customer-row][data-name=가온누리원]");
  const text = await row.innerText().catch(() => "");
  ok("F", "고객사별 보기: 가온누리원 · 이슈 폴더 2개 · 파일 32개 · 마지막 작업", text.includes("이슈 폴더 2개") && text.includes("파일 32개") && text.includes("마지막 작업"), text.replace(/\n/g, " "));
  ok("F", "고객사별 보기: 가람정보도", (await card.locator("[data-testid=customer-row][data-name=가람정보]").count()) === 1);
  await main.screenshot({ path: `${SHOT}/m12-F-customers.png` });
  await row.getByRole("button").first().click();
  await sleep(500);
  const o = await bridge("debug_opened");
  ok("F", "클릭 → 탐색기로 고객사 폴더 열기", o.at(-1) === path.join(ORG, "가온누리원"), o.at(-1));

  const all = await bridge("search", { query: "로그", limit: 50, customer: null });
  const only = await bridge("search", { query: "로그", limit: 50, customer: "가온누리원" });
  ok("F", "검색 '로그': 모든 고객사 → 가람정보 파일도", all.some((h) => h.path.includes(path.join(ORG, "가람정보"))), `${all.length} hits`);
  ok("F", "검색 '로그' + 고객사 가온누리원 → 가온누리원 폴더만", only.length > 0 && only.every((h) => h.path.startsWith(path.join(ORG, "가온누리원"))), `${only.length} hits`);
  const s = await ctx.newPage();
  await s.goto("http://localhost:1420/#/search");
  await s.waitForSelector("[data-testid=customer-filter]", { timeout: 15000 }).catch(() => {});
  const opts = await s.locator("[data-testid=customer-filter] option").allInnerTexts();
  ok("F", "검색창 고객사 필터: 모든 고객사 + 사전", opts[0] === "모든 고객사" && opts.includes("가온누리원") && opts.includes("가람정보"), opts.join(","));
  await s.locator("[data-testid=customer-filter]").selectOption("가온누리원");
  await s.locator(".search-input").fill("로그");
  await waitFor(async () => (await s.locator(".results li[data-i]").count()) > 0, 10000);
  const dirs = await s.locator(".results li[data-i] .r-dir").allInnerTexts();
  ok("F", "검색창: 필터 적용 결과는 가온누리원만", dirs.length > 0 && dirs.every((d) => d.includes("가온누리원")), `${dirs.length} hits`);
  await s.screenshot({ path: `${SHOT}/m12-F-search.png` });
  await s.close();
  await main.getByRole("button", { name: "도움말 ▾" }).click();
  await main.getByRole("menuitem", { name: "고객사 자료는 어디 있나요?" }).click();
  await sleep(500);
  const h = await bridge("debug_opened");
  ok("F", "도움말: 고객사 자료는 어디 있나요? → 설명서 절", decodeURIComponent(h.at(-1) ?? "").endsWith("docs/manual.md#고객사-자료는-어디-있나요"), decodeURIComponent(h.at(-1) ?? ""));
}

// G. undo: back where it was
{
  await tab("되돌리기").click();
  await sleep(1000);
  const row = main.locator(".grid tbody tr").filter({ hasText: "업무 맥락" }).first();
  dialogs.length = 0;
  await row.getByRole("button", { name: "되돌리기", exact: true }).click();
  const back = await waitFor(() => GO_FILES.every((n) => fs.existsSync(path.join(GO, n))) && TOP.every((n) => fs.existsSync(path.join(DESK, n))), 20000);
  ok("G", "되돌리기 → 바탕 화면/가온누리원 로그/tools/go 30개 + 최상위 파일", back && fs.existsSync(path.join(UNIT, "README.md")));
  ok("G", "정리 위치에 남은 파일 없음", walk(DEST).length === 0, walk(DEST).join(","));
}

// H. "이 폴더는 파일별로 나눠서"
{
  // Undo just re-created the folder: it counts as "just changed" for
  // folder_settle_secs (60 s). Pretend a minute has passed.
  ok("H", "되돌린 직후 폴더는 '방금 바뀐 폴더'로 잠시 제외", (await bridge("curate_estimate", { path: DESK })).unsettled === 1);
  for (const p of [GO, path.dirname(GO), UNIT]) fs.utimesSync(p, OLD, OLD);
  await tab("정리하기").click();
  await sleep(500);
  await main.locator("[data-testid=report-folder]").selectOption(DESK);
  await sleep(500);
  dialogs.length = 0;
  await panel.locator("[data-testid=curate-plan]").click();
  await waitFor(async () => (await folderRow().count()) === 1, 60000);
  ok("H", "다시 분석: 캐시로 AI 안 부름 (비용 없음)", dialogs.some((d) => d.includes("비용 없음")), dialogs[0]?.replace(/\n/g, " ").slice(0, 100));
  dialogs.length = 0;
  await folderRow().locator("[data-testid=folder-split]").click();
  const split = await waitFor(async () => (await folderRow().count()) === 0 && (await fileRows().count()) === 34, 60000);
  ok("H", "파일별로 나누기 → 폴더 줄 없이 파일 34줄", split, `folder=${await folderRow().count()} files=${await fileRows().count()}`);
  ok("H", "나누기 전 확인", dialogs.some((d) => d.includes("파일 31개를 하나씩")));
  const f01 = panel.locator("[data-testid=context-row][data-name='f01.go'] td.path").nth(1);
  const to = (await f01.getAttribute("title").catch(() => "")) ?? "";
  ok("H", "안의 파일도 경로 맥락으로 가온누리원", to.startsWith(path.join(ORG, "가온누리원")), to);
  ok("H", "나누기만으로 무이동", fs.existsSync(path.join(GO, "f01.go")));
  await main.screenshot({ path: `${SHOT}/m12-H-split.png` });
  await panel.getByRole("button", { name: "닫기" }).click();
}

// I. watching: sub-folder files never move; top-level files are curated
{
  await tab("설정").click();
  await sleep(800);
  await main.locator(".settings tbody tr:not(.subrow)").first().locator("td").nth(1).locator("input[type=checkbox]").check();
  await save();
  ok("I", "감시: 하위 폴더 포함 켬 (그래도 하위 폴더 파일은 안 옮김)", /recursive = true/.test(fs.readFileSync(CFG, "utf8")));
  await sleep(1500);
  fs.writeFileSync(path.join(GO, "new_tool.go"), "package main // 새 도구");
  fs.mkdirSync(path.join(DESK, "새 프로젝트"));
  fs.writeFileSync(path.join(DESK, "새 프로젝트", "누리원_계획.txt"), "새 폴더 안의 파일");
  fs.writeFileSync(path.join(DESK, "누리원_장애_20261023.txt"), "누리원 장애 보고");
  const top = await waitFor(
    async () =>
      !fs.existsSync(path.join(DESK, "누리원_장애_20261023.txt")) ||
      (await bridge("list_pending")).some((p) => p.name === "누리원_장애_20261023.txt"),
    30000,
  );
  ok("I", "새 최상위 파일 → 업무 맥락으로 처리 (이동 또는 확인 대기)", top);
  await sleep(4000);
  ok("I", "하위 폴더의 새 파일은 그대로", fs.existsSync(path.join(GO, "new_tool.go")));
  ok("I", "새 폴더 안의 파일도 그대로 (폴더마다 AI 팝업 없음)", fs.existsSync(path.join(DESK, "새 프로젝트", "누리원_계획.txt")));
  const est = await bridge("curate_estimate", { path: DESK });
  ok("I", "복사 중(방금 바뀐) 폴더는 이번 정리에서 뺌", est.unsettled >= 1 && est.folders === 0, JSON.stringify({ files: est.files, folders: est.folders, unsettled: est.unsettled }));
  const c = await ctx.newPage();
  await c.setViewportSize({ width: 480, height: 340 });
  await c.goto("http://localhost:1420/#/confirm");
  const pop = await waitFor(async () => (await c.locator("[data-testid=popup-defer]").count()) === 1, 10000);
  const btn = c.locator("[data-testid=popup-defer]");
  ok("I", "확인 팝업: '대기열에 두기' + 툴팁", pop && (await btn.innerText()) === "대기열에 두기" && (await btn.getAttribute("title")) === "결정을 미룹니다. 확인 대기 탭에 남습니다.", pop ? await btn.innerText() : "(팝업에 항목 없음)");
  if (pop) await c.screenshot({ path: `${SHOT}/m12-I-popup.png` });
  await c.close();
}

const failed = results.filter((r) => !r.pass);
console.log(`\n${results.length - failed.length}/${results.length} passed`);
fs.writeFileSync(path.join(S, "m12-results.json"), JSON.stringify(results, null, 2));
await browser.close();
llm.kill();
process.exit(failed.length ? 1 : 0);
