// M10 GUI test: work-context curation + tidied settings (SPEC §14.3, §14.4),
// driven through the dev bridge with a fake OpenAI-compatible server
// (e2e/fake-embed.mjs: chat answers fixed per file name; no real API).
//
//   A. settings: ①② open, ③–⑥ folded by default; ⓘ help shows a line +
//      example; folding is remembered across reloads
//   B. AI 연결 (fake chat) + watch folder, one save button
//   C. ③ 업무 맥락: "바탕화면에서 뽑기" → candidates → pick → save →
//      context.toml has 바다정보 [BADA] / 하늘통신 [SKY]
//   D. 리포트 → "업무 맥락으로 정리": cost notice first, then the preview
//      tree 고객사/날짜_이슈/종류 (dates within 14 days merged), 기타/정책/…,
//      a low-confidence file stays as 미분류; AI judgement + reason per row
//   E. change one item's customer (OceanInfo → 바다정보): re-laid out,
//      dictionary suggestion "별칭" → 사전에 반영 → context.toml
//   F. apply (confirm) → files where the preview said; undo → all back
//   G. watching: curate on → a new file for a known customer/issue goes
//      straight into the existing folder; a new issue asks in the popup
//   H. AI off → "AI 필요" badge, buttons disabled, link opens ④ AI 연결
//
// Run: S=/tmp/tidy-e2e bash e2e/run-headless.sh m10
import { chromium } from "playwright";
import { spawn } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

const T = process.env.T;
const S = process.env.S;
const DESK = path.join(T, "바탕 화면");
const CFG = path.join(T, "config.toml");
const CTX = path.join(T, "context.toml");
const SHOT = path.join(S, "shots");
fs.mkdirSync(SHOT, { recursive: true });
const PORT = 18093;
const FAKE = `http://127.0.0.1:${PORT}`;

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

const fake = spawn(process.execPath, [path.join(import.meta.dirname, "fake-embed.mjs"), String(PORT)], {
  stdio: ["ignore", "inherit", "inherit"],
});
process.on("exit", () => fake.kill());
await sleep(800);

// Fixtures exist before the folder is watched (existing files are not moved
// by the watcher; the report's plan handles them).
const FILES = {
  "BADA_과금_20261014_로그.log": "과금 배치 로그 2026-10-14 ERROR 단가 불일치",
  "바다정보_과금이슈_20261020_안내문.txt": "바다정보 고객 안내: 10월 과금 정정 안내",
  "바다정보_장애_20261002_캡처.png": "PNG-not-really",
  "SKY_번호전환_20261005_대상.csv": "번호,전환일\n01012345678,2026-10-05\n",
  "하늘통신_번호전환_20261008_회의.txt": "하늘통신 번호전환 회의록: 일정 합의",
  "하늘통신_장애_20260915_로그.log": "장애 로그 2026-09-15 timeout",
  "OceanInfo_과금_20261016_정산.csv": "정산,금액\n10월,1200000\n",
  "문자 발송 정책.txt": "문자 발송 정책 — 야간 발송 제한",
  "개인정보 처리 방침.txt": "개인정보 처리 방침 개정",
  "요금 정책 개정안.txt": "요금 정책 개정안 초안",
  "애매한 메모.txt": "누구 건인지 모를 메모",
};
for (const [n, body] of Object.entries(FILES)) fs.writeFileSync(path.join(DESK, n), body);
const at = (...p) => path.join(DESK, ...p);

const browser = await chromium.launch();
const ctx = await browser.newContext({ viewport: { width: 1280, height: 1000 } });
const main = await ctx.newPage();
const dialogs = [];
main.on("dialog", (d) => {
  dialogs.push(d.message());
  void d.accept(process.env.PROMPT_ANSWER ?? "");
});
await main.goto("http://localhost:1420/#/main");
await main.waitForSelector(".topbar");
await sleep(1000);
const section = (id) => main.locator(`section.fold[data-section=${id}]`);
const openSection = async (id) => {
  const h = section(id).locator(".fold-head");
  if ((await h.getAttribute("aria-expanded")) !== "true") await h.click();
  await sleep(300);
};
const save = async () => {
  await main.getByRole("button", { name: "저장", exact: true }).click();
  await sleep(2500);
};

// A. six foldable sections, ⓘ help, remembered state
{
  await main.getByRole("button", { name: "설정", exact: true }).click();
  await sleep(800);
  const ids = ["watch", "rules", "context", "ai", "security", "advanced"];
  const state = [];
  for (const id of ids) state.push(await section(id).locator(".fold-head").getAttribute("aria-expanded"));
  ok("A", "설정 6개 섹션, ①② 펼침 · 나머지 접힘", state.join(",") === "true,true,false,false,false,false", state.join(","));
  const titles = await main.locator("section.fold .fold-head").allInnerTexts();
  ok("A", "섹션 제목 (한국어)", titles.length === 6 && titles[2].includes("업무 맥락") && titles[3].includes("AI 연결"), titles.join(" | "));
  ok("A", "저장 버튼은 하나 (#8)", (await main.getByRole("button", { name: "저장", exact: true }).count()) === 1);
  await section("advanced").locator(".fold-head").click();
  await sleep(300);
  ok("A", "⑥ 고급 펼치기 → 내용 표시", await section("advanced").locator("details.advanced").isVisible());
  await main.reload();
  await main.waitForSelector(".topbar");
  await main.getByRole("button", { name: "설정", exact: true }).click();
  await sleep(800);
  ok("A", "접힘/펼침 상태 기억 (새로고침 후 ⑥ 펼침)", (await section("advanced").locator(".fold-head").getAttribute("aria-expanded")) === "true");
  await section("advanced").locator(".fold-head").click();
  await sleep(300);
  ok("A", "다시 접기", (await section("advanced").locator("details.advanced").count()) === 0);
  const help = section("watch").locator("[data-help=organizeMode] .help-i");
  await help.click();
  await sleep(200);
  const pop = section("watch").locator("[data-help=organizeMode] .help-pop");
  const line = await pop.innerText().catch(() => "");
  await pop.locator("summary").click().catch(() => {});
  await sleep(200);
  const ex = await pop.locator(".help-ex").innerText().catch(() => "");
  ok("A", "ⓘ 도움말: 한 줄 + 예시", line.includes("이 폴더 안에서 정리") && ex.includes("바탕 화면"), `${line.slice(0, 40)} / ${ex.slice(0, 40)}`);
  await main.screenshot({ path: `${SHOT}/m10-A-settings-sections.png` });
  await help.click();
}

// B. AI connection (fake chat) + watch folder, one save
{
  process.env.PROMPT_ANSWER = DESK;
  await main.getByRole("button", { name: "폴더 추가…" }).click();
  await sleep(500);
  await openSection("ai");
  const ai = main.locator("[data-testid=ai-section]");
  await ai.locator("label.row", { hasText: "AI 사용" }).locator("input[type=checkbox]").check();
  await ai.locator("select").first().selectOption("openai");
  await sleep(300);
  const rows = ai.locator("label.row");
  await rows.filter({ hasText: "모델" }).first().locator("input").fill("fake-chat");
  await rows.filter({ hasText: "주소" }).first().locator("input").fill(`${FAKE}/v1`);
  await rows.filter({ hasText: "키 이름" }).first().locator("input").fill("");
  ok("B", "④ AI 연결 섹션에 도움말 (확신도·본문 보내기)", (await ai.locator("[data-help=confidence]").count()) === 1 && (await ai.locator("[data-help=sendContent]").count()) === 1);
  await save();
  const ready = await waitFor(async () => (await bridge("get_status")).aiState === "ready", 20000);
  ok("B", "AI 준비됨 (가짜 OpenAI 호환 서버)", ready, (await bridge("get_status")).aiDetail ?? "");
  const bar = await main.locator(".statusbar").innerText();
  ok("B", "감시 시작", bar.includes("바탕 화면"), bar.replace(/\n/g, " | "));
  ok("B", "감시 시작해도 기존 파일은 그대로", Object.keys(FILES).every((n) => fs.existsSync(at(n))));
}

// C. dictionary from the desktop: candidates → pick → save
{
  await openSection("context");
  const cs = main.locator("[data-testid=context-section]");
  ok("C", "③ 업무 맥락 활성 (AI 켜짐)", await cs.locator("[data-testid=extract-context]").isEnabled());
  await cs.locator("[data-testid=extract-context]").click();
  const shown = await waitFor(async () => (await cs.locator("[data-testid=candidate-entity]").count()) >= 2, 20000);
  const cands = await cs.locator("[data-testid=candidate-entity]").allInnerTexts().catch(() => []);
  ok("C", "바탕화면에서 뽑기 → 고객사 후보 2개", shown, cands.join(" | ").replace(/\n/g, " "));
  ok("C", "뽑기만으로는 사전에 저장 안 됨", !fs.existsSync(CTX) || !fs.readFileSync(CTX, "utf8").includes("바다정보"));
  await cs.screenshot({ path: `${SHOT}/m10-C-candidates.png` });
  await cs.getByRole("button", { name: "고른 것 사전에 넣기" }).click();
  await sleep(300);
  ok("C", "사전 표에 2개", (await cs.locator("[data-testid=dict-entity]").count()) === 2);
  ok("C", "저장하지 않은 변경 표시", (await main.locator(".savebar").innerText()).includes("저장하지 않은 변경"));
  await save();
  const t = fs.existsSync(CTX) ? fs.readFileSync(CTX, "utf8") : "";
  ok("C", "통합 저장 → context.toml: 바다정보[BADA], 하늘통신[SKY]", t.includes("바다정보") && t.includes("BADA") && t.includes("하늘통신") && t.includes("SKY"), t.split("\n").filter((l) => /name|aliases/.test(l)).join(" ; "));
}

// D. 리포트 → 업무 맥락으로 정리: cost notice, then the preview
let planId = null;
{
  await main.getByRole("button", { name: "리포트" }).click();
  await sleep(800);
  const panel = main.locator(".restructure");
  const btn = panel.locator("[data-testid=curate-plan]");
  ok("D", "업무 맥락으로 정리 버튼 활성", await btn.isEnabled());
  dialogs.length = 0;
  await btn.click();
  const shown = await waitFor(async () => (await panel.locator("[data-testid=context-plan]").count()) === 1, 30000);
  ok("D", "분석 전 비용 안내 (파일 수·토큰)", dialogs.length > 0 && /파일 11개/.test(dialogs[0]) && /토큰/.test(dialogs[0]), dialogs[0]?.replace(/\n/g, " ") ?? "(없음)");
  ok("D", "미리보기 표 표시", shown);
  ok("D", "분석만으로 무이동", Object.keys(FILES).every((n) => fs.existsSync(at(n))));
  const after = await panel.locator(".diff pre.tree").nth(1).innerText().catch(() => "");
  ok("D", "적용 후 트리: 바다정보/20261014_과금이슈/로그", /바다정보[\s\S]*20261014_과금이슈[\s\S]*로그/.test(after));
  ok("D", "14일 안 날짜 병합: 20261020 안내문도 20261014_과금이슈로", !after.includes("20261020"));
  ok("D", "하늘통신/20261005_번호전환 (회의록 병합) + 20260915_장애", after.includes("하늘통신") && after.includes("20261005_번호전환") && after.includes("회의록") && after.includes("20260915_장애"));
  ok("D", "고객사 없는 정책 문서 → 기타/정책/…", /기타[\s\S]*정책/.test(after));
  const row = (name) => panel.locator(`[data-testid=context-row][data-name="${name}"]`);
  const vague = await row("애매한 메모.txt").innerText();
  ok("D", "낮은 확신 → 미분류 · 그대로 둠 · 체크 안 됨", vague.includes("미분류") && vague.includes("그대로 둠") && !(await row("애매한 메모.txt").locator("input[type=checkbox]").isChecked()));
  const bada = await row("BADA_과금_20261014_로그.log").innerText();
  ok("D", "AI 판단·근거 표시 (확신 95% + 이유)", bada.includes("확신 95%") && bada.includes("파일 이름"), bada.replace(/\n/g, " ").slice(0, 120));
  ok("D", "별칭 BADA → 사전의 바다정보", (await row("BADA_과금_20261014_로그.log").locator("[data-testid=ctx-entity]").inputValue()) === "바다정보");
  const plans = await bridge("list_plans").catch(() => []);
  planId = plans.find((p) => p.source === "context")?.id ?? null;
  await main.screenshot({ path: `${SHOT}/m10-D-context-preview.png`, fullPage: true });
}

// E. change one item's customer → re-laid out + dictionary suggestion
{
  const panel = main.locator(".restructure");
  const r = panel.locator('[data-testid=context-row][data-name="OceanInfo_과금_20261016_정산.csv"]');
  const before = await r.locator("td.path").nth(1).getAttribute("title");
  await r.locator("[data-testid=ctx-entity]").selectOption("바다정보");
  const moved = await waitFor(async () => ((await r.locator("td.path").nth(1).getAttribute("title")) ?? "").includes("바다정보"), 10000);
  const to = await r.locator("td.path").nth(1).getAttribute("title");
  ok("E", "고객사 변경 → 옮길 곳 다시 계산 (바다정보/20261014_과금이슈/데이터)", moved && to.includes(path.join("바다정보", "20261014_과금이슈", "데이터")), `${before} → ${to}`);
  ok("E", "고친 항목 표시", (await r.innerText()).includes("직접 고침"));
  const sug = panel.locator("[data-testid=context-suggestions]");
  const sugText = await sug.innerText().catch(() => "");
  ok("E", "사전 학습 제안: OceanInfo를 바다정보 별칭으로", sugText.includes("OceanInfo") && sugText.includes("바다정보"), sugText.replace(/\n/g, " ").slice(0, 120));
  await sug.getByRole("button", { name: "사전에 반영" }).click();
  await sleep(1000);
  const t = fs.readFileSync(CTX, "utf8");
  ok("E", "사전에 반영 → context.toml 별칭 OceanInfo", t.includes("OceanInfo"));
  const after = await panel.locator(".diff pre.tree").nth(1).innerText();
  ok("E", "트리에 OceanInfo 폴더 없음", !after.includes("OceanInfo"));
}

// F. apply → files moved where the preview said; undo → all back
const DEST = {
  "BADA_과금_20261014_로그.log": ["바다정보", "20261014_과금이슈", "로그"],
  "바다정보_과금이슈_20261020_안내문.txt": ["바다정보", "20261014_과금이슈", "안내문서"],
  "OceanInfo_과금_20261016_정산.csv": ["바다정보", "20261014_과금이슈", "데이터"],
  "바다정보_장애_20261002_캡처.png": ["바다정보", "20261002_장애", "캡처"],
  "SKY_번호전환_20261005_대상.csv": ["하늘통신", "20261005_번호전환", "데이터"],
  "하늘통신_번호전환_20261008_회의.txt": ["하늘통신", "20261005_번호전환", "회의록"],
  "하늘통신_장애_20260915_로그.log": ["하늘통신", "20260915_장애", "로그"],
  "문자 발송 정책.txt": ["기타", "정책", "문자 발송 정책"],
};
{
  const panel = main.locator(".restructure");
  dialogs.length = 0;
  await panel.getByRole("button", { name: /선택한 \d+개 적용/ }).click();
  const done = await waitFor(async () => (await panel.locator(".notice", { hasText: "옮겼습니다" }).count()) === 1, 20000);
  ok("F", "적용 전 확인 대화상자", dialogs.some((d) => d.includes("되돌릴 수 있습니다")));
  const wrong = Object.entries(DEST).filter(([n, p]) => !fs.existsSync(at(...p, n)));
  ok("F", "파일 위치 = 미리보기 (고객사/날짜_이슈/종류, 기타/정책)", done && wrong.length === 0, wrong.map(([n]) => n).join(", "));
  ok("F", "미분류 파일은 그대로", fs.existsSync(at("애매한 메모.txt")));
  const batches = await bridge("list_batches", { limit: 5 });
  ok("F", "이동 기록: 한 배치 '업무 맥락'", batches[0]?.label?.includes("업무 맥락") ?? false, batches[0]?.label ?? "");
  await main.screenshot({ path: `${SHOT}/m10-F-applied.png` });
  dialogs.length = 0;
  await panel.getByRole("button", { name: "이 재구성 되돌리기" }).click();
  await waitFor(() => dialogs.some((d) => /되돌림/.test(d)), 15000);
  const back = Object.keys(FILES).filter((n) => !fs.existsSync(at(n)));
  ok("F", "되돌리기 → 모두 원래 자리", back.length === 0, back.join(", "));
  ok("F", "빈 폴더는 지우지 않고 남김 (삭제 금지)", fs.existsSync(at("바다정보", "20261014_과금이슈", "로그")));
}

// G. watching: known folder → auto; new issue → popup
{
  await sleep(3000); // the restored files must not be re-sorted
  ok("G", "되돌린 파일은 다시 옮기지 않음", Object.keys(FILES).every((n) => fs.existsSync(at(n))));
  await main.getByRole("button", { name: "설정", exact: true }).click();
  await sleep(800);
  await openSection("context");
  await main.locator("[data-testid=curate-enabled]").check();
  await save();
  ok("G", "config: [curate] enabled = true", /\[curate\][\s\S]*enabled = true/.test(fs.readFileSync(CFG, "utf8")));
  await sleep(1500);
  fs.writeFileSync(at("BADA_과금_20261018_추가.log"), "과금 재처리 로그 2026-10-18");
  const auto = await waitFor(() => fs.existsSync(at("바다정보", "20261014_과금이슈", "로그", "BADA_과금_20261018_추가.log")), 20000);
  ok("G", "새 파일(기존 고객사·이슈 폴더, 확신 95%) → 자동 배치", auto && !fs.existsSync(at("BADA_과금_20261018_추가.log")));
  fs.writeFileSync(at("바다정보_대체발송_20261021_문의.txt"), "대체발송이 두 번 나갔다는 문의");
  const queued = await waitFor(async () => (await bridge("list_pending")).some((p) => p.name === "바다정보_대체발송_20261021_문의.txt"), 20000);
  ok("G", "새 이슈 → 대기열 (무이동)", queued && fs.existsSync(at("바다정보_대체발송_20261021_문의.txt")));
  const c = await ctx.newPage();
  await c.setViewportSize({ width: 480, height: 340 }); // about the real popup
  await c.goto("http://localhost:1420/#/confirm");
  await c.waitForSelector(".confirm .c-file");
  await sleep(500);
  const q = await c.locator("[data-testid=confirm-question]").innerText();
  ok("G", "확인 팝업: '새 이슈 … 폴더를 만들까요?'", q.includes("새 이슈") && q.includes("대체발송") && q.endsWith("까요?"), q);
  const dest = await c.locator(".c-dest").innerText();
  ok("G", "제안 위치: 바다정보/20261021_대체발송 중복", dest.includes("20261021_대체발송 중복"), dest.split("\n")[1] ?? "");
  await c.screenshot({ path: `${SHOT}/m10-G-confirm.png` });
  await c.getByRole("button", { name: "이동", exact: true }).click();
  const okMove = await waitFor(() => fs.existsSync(at("바다정보", "20261021_대체발송 중복", "안내문서", "바다정보_대체발송_20261021_문의.txt")), 10000);
  ok("G", "승인 → 새 이슈 폴더로 이동", okMove);
  await c.close();
}

// H. AI off → "AI 필요" everywhere, link to ④
{
  await main.getByRole("button", { name: "설정", exact: true }).click();
  await sleep(800);
  await openSection("ai");
  const ai = main.locator("[data-testid=ai-section]");
  await ai.locator("label.row", { hasText: "AI 사용" }).locator("input[type=checkbox]").uncheck();
  await save();
  const off = await waitFor(async () => !["ready", "starting"].includes((await bridge("get_status")).aiState), 15000);
  ok("H", "AI 끔 → aiState off", off, (await bridge("get_status")).aiState);
  await sleep(500);
  const cs = main.locator("[data-testid=context-section]");
  await openSection("context");
  const extractOff = await cs.locator("[data-testid=extract-context]").isDisabled();
  const badges = await section("context").locator("[data-testid=ai-needed]").count();
  ok("H", "③ 업무 맥락 비활성 + 'AI 필요' 배지", extractOff && badges >= 1, `extractDisabled=${extractOff} badges=${badges}`);
  ok("H", "규칙 'AI로 규칙 만들기' 비활성", await main.getByRole("button", { name: "AI로 규칙 만들기" }).isDisabled());
  await main.getByRole("button", { name: "리포트" }).click();
  await sleep(800);
  const panel = main.locator(".restructure");
  if (await panel.getByRole("button", { name: "닫기" }).count()) await panel.getByRole("button", { name: "닫기" }).click();
  await panel.locator("h3").scrollIntoViewIfNeeded();
  ok("H", "리포트: 업무 맥락으로 정리 비활성", await panel.locator("[data-testid=curate-plan]").isDisabled());
  const badge = panel.locator("[data-testid=ai-needed]");
  ok("H", "리포트: 'AI 필요' + 'AI 연결에서 켜기'", (await badge.count()) === 1 && (await badge.innerText()).includes("AI 연결에서 켜기"));
  await main.screenshot({ path: `${SHOT}/m10-H-ai-needed.png` });
  await badge.getByRole("button", { name: "AI 연결에서 켜기" }).click();
  await sleep(800);
  ok("H", "링크 → 설정 ④ AI 연결 펼쳐짐", (await section("ai").locator(".fold-head").getAttribute("aria-expanded")) === "true" && (await main.locator("[data-testid=ai-section]").isVisible()));
}

const failed = results.filter((r) => !r.pass);
console.log(`\n${results.length - failed.length}/${results.length} passed`);
fs.writeFileSync(path.join(S, "m10-results.json"), JSON.stringify(results, null, 2));
await browser.close();
fake.kill();
process.exit(failed.length ? 1 : 0);
