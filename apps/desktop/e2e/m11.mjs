// M11 GUI test: UI simplification, help/about/update, organize status
// (SPEC §14.6), driven through the dev bridge. A slow fake LLM
// (e2e/fake-llm.mjs, ~1 s per file, ~300-character reasons) and a fake
// GitHub releases API (in this file) stand in for the network; links are
// recorded instead of opened (TIDY_E2E_NO_OPEN, set by run-headless.sh).
//
//   A. settings: three watch folders (desktop in place, downloads collected
//      into 정리, 업무), rules with an absolute destination → card + save
//      bar warning, "상대 경로로 바꾸기"; check_updates toggle; AI (slow fake)
//   B. status bar summary (감시 중 3곳 · 정리 위치 3곳 · 새 파일 자동 정리 중 ·
//      AI 사용(model)) → 감시 현황 panel, one row per folder
//   C. 정리 현황 card: backlog per folder → 지금 정리 → 정리하기 with the
//      rules preview (no-move notice, curate button highlighted) → apply →
//      되돌리기 tab undo → files back
//   D. 정리 위치 warning: organizing the collect root asks twice
//   E. tabs renamed (홈 / 확인 대기 / 정리하기 / 되돌리기 / 설정); 도움말 menu:
//      manual, issue (version prefilled), 정보 (version = package.json)
//   F. update check: newer (받기 → setup.exe, status bar dot) / latest /
//      network error
//   G. 업무 맥락으로 정리 on 30 files with the slow AI: succeeds (~30 s)
//   H. an answer cut off at max_tokens: the batch is split and retried
//
// Run: S=/tmp/tidy-e2e bash e2e/run-headless.sh m11
import { chromium } from "playwright";
import { spawn } from "node:child_process";
import fs from "node:fs";
import http from "node:http";
import path from "node:path";

const T = process.env.T;
const S = process.env.S;
const DESK = path.join(T, "바탕 화면");
const DOWN = path.join(T, "다운로드");
const WORK = path.join(T, "업무");
const ORG = path.join(T, "정리");
const ABS = path.join(T, "절대", "설치파일");
const CFG = path.join(T, "config.toml");
const RULES = path.join(T, "rules.toml");
const SHOT = path.join(S, "shots");
fs.mkdirSync(SHOT, { recursive: true });
const LLM_PORT = 18095;
const GH_PORT = 18097;
const LLM = `http://127.0.0.1:${LLM_PORT}`;
const VERSION = JSON.parse(fs.readFileSync(new URL("../package.json", import.meta.url), "utf8")).version;

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
const opened = async () => bridge("debug_opened");

// ── fakes ───────────────────────────────────────────────────────────────
const llm = spawn(process.execPath, [path.join(import.meta.dirname, "fake-llm.mjs"), String(LLM_PORT)], {
  stdio: ["ignore", "inherit", "inherit"],
});
process.on("exit", () => llm.kill());

/** GitHub `releases/latest`: "newer" | "latest" | "error". */
let ghMode = "latest";
const release = (v) => ({
  tag_name: `v${v}`,
  html_url: `https://github.com/sjh714252-prog/tidy/releases/tag/v${v}`,
  assets: [
    { name: `Tidy_${v}_x64_ko-KR.msi`, browser_download_url: `https://example.invalid/Tidy_${v}_x64_ko-KR.msi` },
    { name: `Tidy_${v}_x64-setup.exe`, browser_download_url: `https://example.invalid/Tidy_${v}_x64-setup.exe` },
    { name: `tidy_${v}_amd64.AppImage`, browser_download_url: `https://example.invalid/tidy_${v}_amd64.AppImage` },
  ],
});
const gh = http
  .createServer((req, res) => {
    if (ghMode === "error") {
      res.writeHead(500);
      return res.end("down");
    }
    res.writeHead(200, { "content-type": "application/json" });
    res.end(JSON.stringify(release(ghMode === "newer" ? "0.4.1" : VERSION)));
  })
  .listen(GH_PORT, "127.0.0.1");
await sleep(800);

// ── fixtures (before watching: "existing" files) ────────────────────────
for (const d of [DESK, DOWN, WORK, ORG]) fs.mkdirSync(d, { recursive: true });
for (const n of ["보고서 A.pdf", "보고서 B.pdf"]) fs.writeFileSync(path.join(DESK, n), "%PDF-1.4 fake");
fs.writeFileSync(path.join(DESK, "메모.xyz"), "규칙 없는 파일");
fs.writeFileSync(path.join(DESK, "회의.docx"), "규칙 없는 파일 2");
fs.writeFileSync(path.join(DOWN, "계약서.pdf"), "%PDF-1.4 fake");
fs.mkdirSync(path.join(ORG, "문서", "2026-01"), { recursive: true });
fs.writeFileSync(path.join(ORG, "문서", "2026-01", "이미 정리됨.pdf"), "%PDF-1.4 fake");
const WORK_FILES = [];
for (let i = 1; i <= 30; i++) {
  const d = String(1 + (i % 20)).padStart(2, "0");
  const n =
    i % 3 === 0
      ? `문자 정책 ${i}.txt`
      : i % 3 === 1
        ? `BADA_과금_202610${d}_로그${i}.log`
        : `SKY_번호전환_202610${d}_자료${i}.txt`;
  fs.writeFileSync(path.join(WORK, n), `업무 자료 ${i}: ${n}`);
  WORK_FILES.push(n);
}
fs.writeFileSync(
  path.join(T, "context.toml"),
  '[[entity]]\nname = "바다정보"\naliases = ["BADA"]\n\n[[entity]]\nname = "하늘통신"\naliases = ["SKY"]\n',
);
const RULES_TEXT = `[[rule]]\nname = "문서"\nmatch = { ext = ["pdf"] }\ndest = "문서/{yyyy}-{mm}"\n\n[[rule]]\nname = "설치파일"\nmatch = { ext = ["exe"] }\ndest = '${ABS}'\n`;

// ── browser ─────────────────────────────────────────────────────────────
const browser = await chromium.launch();
const ctx = await browser.newContext({ viewport: { width: 1280, height: 960 } });
await ctx.addInitScript(() =>
  localStorage.setItem("tidy.settings.sections", JSON.stringify({ watch: true, rules: true, context: true, ai: true, security: false, advanced: true })),
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

// A. settings: folders, collect, rules warning + fix, AI
{
  await tab("설정").click();
  await sleep(800);
  for (const d of [DESK, DOWN, WORK]) {
    process.env.PROMPT_ANSWER = d;
    await main.getByRole("button", { name: "폴더 추가…", exact: true }).click();
    await sleep(500);
  }
  const rows = main.locator(".settings tbody tr:not(.subrow)");
  const down = rows.nth(1); // added in order: 바탕 화면, 다운로드, 업무
  await down.locator("select[aria-label='정리 방식']").selectOption("collect");
  await down.locator("input.organize-root").fill(ORG);
  const d = main.locator("details.advanced");
  if (!(await d.evaluate((e) => e.open))) await d.locator("summary").click();
  await main.locator("textarea.rules").fill(RULES_TEXT);
  await sleep(1500);
  const warn = main.locator("[data-testid=abs-dest-warning]");
  const wtext = await warn.innerText().catch(() => "");
  ok("A", "절대 경로 규칙 + 모으기 폴더 → 규칙 카드 경고", (await warn.count()) === 1 && wtext.includes(ORG) && wtext.includes("설치파일"), wtext.replace(/\n/g, " "));
  const sbw = await main.locator("[data-testid=abs-rules-warning]").innerText().catch(() => "");
  ok("A", "저장바 경고: 규칙 1개가 정리 위치를 무시", sbw.includes("규칙 1개"), sbw);
  await warn.scrollIntoViewIfNeeded();
  await main.evaluate(() => window.scrollBy(0, 0));
  await main.screenshot({ path: `${SHOT}/m11-A-abs-dest.png`, fullPage: false });
  await warn.locator("[data-testid=make-relative]").click();
  await sleep(1000);
  const toml = await main.locator("textarea.rules").inputValue();
  ok("A", "상대 경로로 바꾸기 → dest = \"설치파일\"", /dest = "설치파일"/.test(toml) && (await warn.count()) === 0, toml.split("\n").filter((l) => l.startsWith("dest")).join(" | "));
  ok("A", "업데이트 확인 토글 기본 켜짐", await main.locator("[data-testid=check-updates]").isChecked());

  const ai = main.locator("[data-testid=ai-section]");
  await ai.locator("label.row", { hasText: "AI 사용" }).locator("input[type=checkbox]").check();
  await ai.locator("select").first().selectOption("openai");
  await sleep(300);
  const arows = ai.locator("label.row");
  await arows.filter({ hasText: "모델" }).first().locator("input").fill("fake-slow");
  await arows.filter({ hasText: "주소" }).first().locator("input").fill(`${LLM}/v1`);
  await arows.filter({ hasText: "키 이름" }).first().locator("input").fill("");
  await save();
  const cfg = fs.readFileSync(CFG, "utf8");
  ok("A", "config: 3 watches, collect + organize_root, check_updates", (cfg.match(/\[\[watch\]\]/g) ?? []).length === 3 && /mode = "collect"/.test(cfg) && cfg.includes("check_updates = true"));
  ok("A", "rules.toml 저장: 상대 경로", fs.readFileSync(RULES, "utf8").includes('dest = "설치파일"'));
  const ready = await waitFor(async () => (await bridge("get_status")).aiState === "ready", 20000);
  ok("A", "AI 준비됨 (느린 가짜 서버)", ready, (await bridge("get_status")).aiDetail ?? "");
}

// B. status bar summary + 감시 현황 panel
{
  await tab("홈").click();
  await sleep(1000);
  const sum = await main.locator("[data-testid=watch-summary]").innerText();
  ok("B", "상태바 왼쪽: 감시 중 3곳 · 정리 위치 3곳", sum.includes("감시 중 3곳") && sum.includes("정리 위치 3곳"), sum);
  const title = await main.locator("[data-testid=watch-summary]").getAttribute("title");
  ok("B", "전체 경로는 툴팁", title.includes(DESK) && title.includes(ORG), title.replace(/\n/g, " | "));
  const st = await main.locator("[data-testid=watch-state]").innerText();
  ok("B", "가운데: 새 파일 자동 정리 중", st.includes("새 파일 자동 정리 중"), st);
  const bar = await main.locator(".statusbar").innerText();
  ok("B", "오른쪽: 인덱스 · AI 사용(모델)", /인덱스 [\d,]+/.test(bar) && bar.includes("AI 사용 (fake-slow)"), bar.replace(/\n/g, " | "));
  ok("B", "상태바에 긴 경로 나열 없음", !bar.includes(T), "");
  await main.locator("[data-testid=watch-summary]").click();
  const panel = main.locator("[data-testid=watch-panel]");
  const shown = await waitFor(async () => (await panel.locator("[data-testid=watch-row]").count()) === 3, 5000);
  ok("B", "감시 현황 패널: 폴더 3줄", shown);
  const counted = await waitFor(async () => !(await panel.innerText()).includes("세는 중"), 20000);
  const ptext = await panel.innerText();
  ok("B", "패널: 다운로드 → …\\정리 (모으기)", ptext.includes("→ ") && ptext.includes("정리"), "");
  ok("B", "패널: 상태 정상 · 미정리 수", counted && ptext.includes("정상") && /4개/.test(ptext) && /30개/.test(ptext) && /1개/.test(ptext), ptext.replace(/\n/g, " | ").slice(0, 300));
  await main.screenshot({ path: `${SHOT}/m11-B-status-panel.png` });
}

// C. 정리 현황 card → 지금 정리 → preview → apply → undo
{
  const card = main.locator("[data-testid=organize-card]");
  const total = await card.locator("[data-testid=backlog-total]").innerText().catch(() => "");
  ok("C", "정리 현황: 기존 파일 35개 아직 정리 안 됨", total.includes("35개"), (await card.innerText()).replace(/\n/g, " | "));
  ok("C", "안내: 새 파일은 자동으로 정리", (await card.innerText()).includes("새 파일은 자동으로 정리됩니다"));
  const deskRow = card.locator("[data-testid=backlog-folder]", { hasText: "바탕 화면" });
  ok("C", "폴더별 수: 바탕 화면 4개 (규칙에 맞음 2 · 규칙 없음 2)", (await deskRow.innerText()).includes("4개") && (await deskRow.innerText()).includes("규칙에 맞음 2"));
  await main.screenshot({ path: `${SHOT}/m11-C-home.png` });
  await deskRow.getByRole("button", { name: "지금 정리" }).click();
  const panel = main.locator(".restructure");
  const planned = await waitFor(async () => (await panel.locator(".plan").count()) === 1, 15000);
  ok("C", "지금 정리 → 정리하기 탭 + 규칙 미리보기 실행", planned && (await tab("정리하기").getAttribute("class")).includes("on"));
  ok("C", "선택한 폴더 = 바탕 화면", (await main.locator("[data-testid=report-folder]").inputValue()) === DESK);
  ok("C", "미리보기 위 안내 (적용 전 무이동·되돌리기)", (await panel.locator("[data-testid=no-move-notice]").innerText()).includes("적용을 누르기 전에는 아무것도 옮기지 않습니다"));
  ok("C", "업무 맥락으로 정리 강조 (AI + 사전)", ((await panel.locator("[data-testid=curate-plan]").getAttribute("class")) ?? "").includes("primary"));
  const rows = await panel.locator(".plan table tbody tr").count();
  ok("C", "규칙 미리보기: pdf 2개", rows === 2, `rows=${rows}`);
  ok("C", "미리보기만으로는 무이동", fs.existsSync(path.join(DESK, "보고서 A.pdf")));
  await main.screenshot({ path: `${SHOT}/m11-C-preview.png` });
  dialogs.length = 0;
  await panel.getByRole("button", { name: /선택한 \d+개 적용/ }).click();
  const moved = await waitFor(async () => (await panel.locator(".notice", { hasText: "옮겼습니다" }).count()) === 1, 15000);
  const dest = fs.readdirSync(path.join(DESK, "문서")).flatMap((m) => fs.readdirSync(path.join(DESK, "문서", m)));
  ok("C", "적용 → 바탕 화면/문서/연-월 로 2개", moved && dest.includes("보고서 A.pdf") && dest.includes("보고서 B.pdf"), dest.join(", "));
  ok("C", "적용 확인 문구: 되돌리기 탭", dialogs.some((d) => d.includes("되돌리기 탭")));
  await tab("되돌리기").click();
  await sleep(1000);
  const row = main.locator(".history tbody tr, .grid tbody tr").filter({ hasText: "정리 제안" }).first();
  dialogs.length = 0;
  await row.getByRole("button", { name: "되돌리기", exact: true }).click();
  const back = await waitFor(() => fs.existsSync(path.join(DESK, "보고서 A.pdf")) && fs.existsSync(path.join(DESK, "보고서 B.pdf")), 10000);
  ok("C", "되돌리기 탭 → 원래 자리로", back, dialogs.join(" / "));
}

// D. organizing the collect root asks twice
{
  await tab("정리하기").click();
  await sleep(500);
  process.env.PROMPT_ANSWER = ORG;
  await main.getByRole("button", { name: "다른 폴더…" }).click();
  const warned = await waitFor(async () => (await main.locator("[data-testid=organize-root-warning]").count()) === 1, 5000);
  ok("D", "정리 위치 폴더 선택 → 경고", warned, await main.locator("[data-testid=organize-root-warning]").innerText().catch(() => ""));
  const opts = await main.locator("[data-testid=report-folder] option").allInnerTexts();
  ok("D", "폴더 목록: 감시 폴더 3개 + 고른 다른 폴더", opts.length === 4 && opts.some((o) => o.includes("다른 폴더")), opts.join(" | "));
  dialogs.length = 0;
  await main.locator(".restructure").getByRole("button", { name: "규칙으로 정리 미리보기" }).click();
  await sleep(2000);
  ok("D", "미리보기 전에 확인 두 번", dialogs.length === 2 && dialogs[0].includes("정리 결과가 모이는 곳"), dialogs.map((d) => d.slice(0, 30)).join(" / "));
  await main.screenshot({ path: `${SHOT}/m11-D-organize-root.png` });
}

// E. tabs + 도움말 menu + 정보
{
  const names = await main.locator(".topbar nav button").allInnerTexts();
  ok("E", "탭: 홈 / 확인 대기 / 정리하기 / 되돌리기 / 설정", names.map((n) => n.replace(/\d+$/, "").trim()).join(",") === "홈,확인 대기,정리하기,되돌리기,설정", names.join(","));
  await main.getByRole("button", { name: "도움말 ▾" }).click();
  const items = await main.locator("[data-testid=help-menu] [role=menuitem]").allInnerTexts();
  // (M12) + "고객사 자료는 어디 있나요?" after the manual.
  ok("E", "도움말 메뉴: 설명서·문의·업데이트·정보", ["사용 설명서", "문의·버그 신고", "업데이트 확인", "정보"].every((i) => items.includes(i)), items.join(","));
  await main.getByRole("menuitem", { name: "사용 설명서" }).click();
  await sleep(500);
  let o = await opened();
  ok("E", "사용 설명서 → GitHub docs/manual.md", o.at(-1) === "https://github.com/sjh714252-prog/tidy/blob/main/docs/manual.md", o.at(-1));
  await main.getByRole("button", { name: "도움말 ▾" }).click();
  await main.getByRole("menuitem", { name: "문의·버그 신고" }).click();
  await sleep(800);
  o = await opened();
  const issue = decodeURIComponent(o.at(-1) ?? "");
  ok("E", "문의 → issues/new + 버전·OS·설정 경로", issue.startsWith("https://github.com/sjh714252-prog/tidy/issues/new?title=") && issue.includes(`버전: ${VERSION}`) && issue.includes(CFG), issue.slice(0, 160).replace(/\n/g, " "));
  await main.getByRole("button", { name: "도움말 ▾" }).click();
  await main.getByRole("menuitem", { name: "정보" }).click();
  const modal = main.locator("[data-testid=about-modal]");
  await modal.waitFor();
  await sleep(500);
  ok("E", `정보 창: 버전 = package.json (${VERSION})`, (await modal.locator("[data-testid=about-version]").innerText()) === VERSION);
  ok("E", "정보 창: 빌드 dev · MIT · 설정/DB 경로", (await modal.locator("[data-testid=about-build]").innerText()) === "dev" && (await modal.innerText()).includes("MIT") && (await modal.innerText()).includes(CFG));
  await modal.getByRole("button", { name: "로그 폴더 열기" }).click();
  await sleep(500);
  o = await opened();
  ok("E", "로그 폴더 열기", (o.at(-1) ?? "").endsWith(path.join("tidy", "logs")), o.at(-1));
  await main.screenshot({ path: `${SHOT}/m11-E-about.png` });
  await modal.getByRole("button", { name: "닫기" }).click();
}

// F. update check: newer / latest / error
{
  ghMode = "newer";
  await main.getByRole("button", { name: "도움말 ▾" }).click();
  await main.getByRole("menuitem", { name: "업데이트 확인" }).click();
  const modal = main.locator("[data-testid=about-modal]");
  const res = modal.locator("[data-testid=update-result]");
  await waitFor(async () => (await res.count()) === 1, 15000);
  ok("F", "새 버전 있음 → '새 버전 0.4.1 — 받기'", (await res.innerText()).includes("새 버전 0.4.1"), await res.innerText());
  await res.getByRole("button", { name: "받기" }).click();
  await sleep(500);
  const o = await opened();
  ok("F", "받기 → Windows setup.exe 또는 이 OS 설치 파일", /setup\.exe$|\.AppImage$|\.dmg$/.test(o.at(-1) ?? ""), o.at(-1));
  await main.screenshot({ path: `${SHOT}/m11-F-update.png` });
  await modal.getByRole("button", { name: "닫기" }).click();
  const dot = await waitFor(async () => (await main.locator("[data-testid=update-dot]").count()) === 1, 8000);
  ok("F", "상태바 '업데이트 있음' 점", dot);
  ghMode = "latest";
  await main.locator("[data-testid=update-dot]").click();
  await modal.waitFor();
  await modal.getByRole("button", { name: "업데이트 확인" }).click();
  await waitFor(async () => (await res.innerText().catch(() => "")).includes("최신"), 15000);
  ok("F", "최신이면 '최신 버전입니다'", (await res.innerText()).includes(`최신 버전입니다 (${VERSION})`), await res.innerText());
  ghMode = "error";
  await modal.getByRole("button", { name: "업데이트 확인" }).click();
  await waitFor(async () => (await res.innerText().catch(() => "")).includes("확인하지 못했습니다"), 15000);
  ok("F", "네트워크 오류 → '확인하지 못했습니다' (막지 않음)", (await res.innerText()).includes("확인하지 못했습니다"), (await res.innerText()).slice(0, 80));
  await modal.getByRole("button", { name: "닫기" }).click();
  await sleep(1500);
  ok("F", "최신 확인 뒤 점 사라짐", (await main.locator("[data-testid=update-dot]").count()) === 0);
}

// G. 업무 맥락으로 정리: 30 files with the slow AI
{
  await tab("정리하기").click();
  await sleep(500);
  await main.locator("[data-testid=report-folder]").selectOption(WORK);
  await sleep(800);
  ok("G", "감시 폴더 선택 → 정리 위치 경고 없음", (await main.locator("[data-testid=organize-root-warning]").count()) === 0);
  const panel = main.locator(".restructure");
  dialogs.length = 0;
  const t0 = Date.now();
  await panel.locator("[data-testid=curate-plan]").click();
  const done = await waitFor(async () => (await panel.locator("[data-testid=context-plan]").count()) === 1, 180000, 1000);
  const secs = Math.round((Date.now() - t0) / 1000);
  ok("G", "비용 안내 (파일 30개)", dialogs.some((d) => d.includes("파일 30개")), dialogs[0]?.replace(/\n/g, " ").slice(0, 100));
  const stats = await (await fetch(`${LLM}/stats`)).json();
  ok("G", `느린 AI로 30개 분석 성공 (${secs}초)`, done && secs >= 20, JSON.stringify(stats));
  const rows = await panel.locator("[data-testid=context-row]").count();
  ok("G", "미리보기 30줄", rows === 30, `rows=${rows}`);
  const after = await panel.locator(".diff pre.tree").nth(1).innerText().catch(() => "");
  ok("G", "트리: 바다정보 / 하늘통신 / 기타", after.includes("바다정보") && after.includes("하늘통신") && after.includes("기타"));
  ok("G", "근거가 긴 답도 표시 (300자)", ((await panel.locator("[data-testid=context-row] .reason").first().innerText()).length >= 200));
  ok("G", "분석만으로 무이동", WORK_FILES.every((n) => fs.existsSync(path.join(WORK, n))));
  await main.screenshot({ path: `${SHOT}/m11-G-curate.png` });
}

// H. answer cut off at max_tokens (FAKE max tokens): Tidy splits the batch
//    and retries instead of failing (fix of PR #13, seen end to end)
{
  const OTHER = path.join(T, "다른 업무");
  fs.mkdirSync(OTHER, { recursive: true });
  for (let i = 1; i <= 8; i++) fs.writeFileSync(path.join(OTHER, `BADA_장애_202610${String(10 + i)}_${i}.log`), `장애 로그 ${i}`);
  await fetch(`${LLM}/control`, { method: "POST", body: JSON.stringify({ delayMs: 300, maxTokens: 600 }) });
  const before = await (await fetch(`${LLM}/stats`)).json();
  process.env.PROMPT_ANSWER = OTHER;
  await main.getByRole("button", { name: "다른 폴더…" }).click();
  await sleep(800);
  const panel = main.locator(".restructure");
  await panel.locator("[data-testid=curate-plan]").click();
  const done = await waitFor(async () => (await panel.locator("[data-testid=context-row]").count()) === 8, 90000, 1000);
  const stats = await (await fetch(`${LLM}/stats`)).json();
  ok("H", "답이 max_tokens에서 잘려도 나눠서 다시 물어 8개 성공", done && stats.truncated > before.truncated && stats.chats - before.chats > 1, JSON.stringify({ truncated: stats.truncated - before.truncated, requests: stats.chats - before.chats }));
  await fetch(`${LLM}/control`, { method: "POST", body: JSON.stringify({ delayMs: 1000, maxTokens: 0 }) });
}

const failed = results.filter((r) => !r.pass);
console.log(`\n${results.length - failed.length}/${results.length} passed`);
fs.writeFileSync(path.join(S, "m11-results.json"), JSON.stringify(results, null, 2));
await browser.close();
llm.kill();
gh.close();
process.exit(failed.length ? 1 : 0);
