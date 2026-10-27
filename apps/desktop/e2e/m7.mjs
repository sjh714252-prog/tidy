// M7 GUI test: semantic search + AI health + autostart, through the dev
// bridge with a fake OpenAI-compatible server (e2e/fake-embed.mjs; no real
// API is ever called).
//
//   A. watch folder + start-at-login on → OS login item written (Linux:
//      ~/.config/autostart/Tidy.desktop with --autostart)
//   B. AI settings: embedding model (OpenAI-compatible, fake) → files are
//      embedded in the background, status bar "의미검색 켜짐"
//   C. search window: natural-language question finds a file whose words
//      don't match (의미 badge, similarity, "단어+의미 (질문형)"); keyword
//      query keeps the word match first; filters apply to meaning hits
//   D. a new file dropped into the watch folder is embedded and found by meaning
//   E. outage: embedding + chat model down → search falls back to words only
//      with a note, status bar "AI 사용 불가" / "의미검색 일시 중단" after 3
//      failures; back up → both "ready" again and files that skipped the
//      AI are classified (pending)
//   F. start-at-login off → login item removed
//
// Run: S=/tmp/tidy-e2e bash e2e/run-headless.sh m7   (sets TIDY_DEV_AUTOSTART=1)
import { chromium } from "playwright";
import { spawn } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { addWatchFolder, gotoSettings, openToml, saveSettings, statusText, tab } from "./lib.mjs";

const T = process.env.T;
const DESK = path.join(T, "바탕 화면");
const AI_DEST = path.join(T, "AI 추천");
const PORT = 18081;
const FAKE = `http://127.0.0.1:${PORT}`;
const AUTOSTART = path.join(os.homedir(), ".config", "autostart", "Tidy.desktop");
const SHOT = path.join(process.env.S, "shots");
fs.mkdirSync(SHOT, { recursive: true });
fs.rmSync(AUTOSTART, { force: true });

const results = [];
const ok = (step, what, pass, note = "") => {
  results.push({ step, what, pass, note });
  console.log(`${pass ? "PASS" : "FAIL"} [${step}] ${what} ${note}`);
};
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const bridge = async (cmd, args = {}) =>
  (await (await fetch(`http://127.0.0.1:1421/invoke/${cmd}`, { method: "POST", body: JSON.stringify(args) })).json()).value;
const waitFor = async (fn, ms, step = 1000) => {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    if (await fn()) return true;
    await sleep(step);
  }
  return false;
};
const control = (fail) => fetch(`${FAKE}/control`, { method: "POST", body: JSON.stringify({ fail }) });
const stats = async () => (await fetch(`${FAKE}/stats`)).json();

// Fake OpenAI-compatible server (embeddings + chat); a leftover one from an
// aborted run is told to shut up first.
await fetch(`${FAKE}/control`, { method: "POST", body: '{"fail":false}' }).catch(() => {});
const fake = spawn(process.execPath, [path.join(import.meta.dirname, "fake-embed.mjs"), String(PORT)], {
  env: { ...process.env, FAKE_DEST: AI_DEST },
  stdio: "inherit",
});
process.on("exit", () => fake.kill());
await sleep(800);

// Files that are already on the desktop (indexed when the folder is added).
fs.writeFileSync(path.join(DESK, "KT_고지서_2026-09.txt"), "9월 인터넷 이용 요금 33,000원 납부 안내");
fs.writeFileSync(path.join(DESK, "오사카_항공권.txt"), "인천 → 간사이 왕복 e-ticket, 2026-11-02 출발");
fs.writeFileSync(path.join(DESK, "회의록.txt"), "분기 실적 보고와 다음 분기 목표 정리");
fs.writeFileSync(path.join(DESK, "부동산_서류.md"), "임대차 약정 조건과 보증금 반환 일정");

const browser = await chromium.launch();
const ctx = await browser.newContext({ viewport: { width: 1000, height: 900 } });
const main = await ctx.newPage();
main.on("dialog", (d) => d.accept(process.env.PROMPT_ANSWER ?? ""));
await main.goto("http://localhost:1420/#/main");
await main.waitForSelector(".topbar");
await sleep(1000);

// A. watch folder (no rules: nothing moves) + start at login
{
  await gotoSettings(main, "watch");
  await addWatchFolder(main, DESK);
  const ta = await openToml(main);
  await ta.fill("# 규칙 없음 — 이 테스트는 파일을 옮기지 않는다\n");
  await gotoSettings(main, "advanced");
  const auto = main.getByRole("checkbox", { name: /로그인 시 자동 시작/ });
  ok("A", "자동 시작 체크박스 (기본 켜짐, 설정 → 고급)", await auto.isChecked());
  await saveSettings(main, 3000);
  const bar = await statusText(main);
  const title = (await main.locator("[data-testid=watch-summary]").getAttribute("title")) ?? "";
  ok("A", "감시 설정 저장 (감시 1곳)", bar.includes("감시 1곳") && title.includes("바탕 화면"), bar.replace(/\n/g, " | "));
  const desktop = fs.existsSync(AUTOSTART) ? fs.readFileSync(AUTOSTART, "utf8") : "";
  ok("A", "OS 시작 항목 등록 (~/.config/autostart/Tidy.desktop, --autostart)", /--autostart/.test(desktop), desktop.split("\n").find((l) => l.startsWith("Exec")) ?? "(없음)");
  await gotoSettings(main, "advanced");
  const st = await main.locator("[data-testid=autostart-state]").innerText();
  ok("A", "설정 화면: OS 시작 항목에 등록됨", st.includes("등록됨"), st);
  const cfg = fs.readFileSync(path.join(T, "config.toml"), "utf8");
  ok("A", "config.toml start_on_login = true", /start_on_login *= *true/.test(cfg));
}

// B. AI settings: embedding model only (no classify model yet)
{
  await gotoSettings(main, "ai");
  const ai = main.locator("[data-testid=ai-section]");
  await ai.locator("label.row", { hasText: "AI 사용" }).locator("input[type=checkbox]").check();
  const embedRow = ai.locator("label.row").filter({ has: main.locator("option[value=ollama]") }).filter({ hasNotText: "Anthropic" });
  await embedRow.locator("select").selectOption("openai");
  await sleep(300);
  const rows = ai.locator("label.row");
  const modelRows = rows.filter({ hasText: "모델" });
  await modelRows.last().locator("input").fill("fake-embed");
  await rows.filter({ hasText: "주소" }).last().locator("input").fill(`${FAKE}/v1`);
  await rows.filter({ hasText: "키 이름" }).last().locator("input").fill("");
  ok("B", "키 저장 위치 표시", (await ai.innerText()).includes("키 저장 위치"), (await ai.locator("p", { hasText: "키 저장 위치" }).innerText()).trim());
  await saveSettings(main, 2000);
  const err = await ai.locator(".error").count();
  ok("B", "임베딩 모델 저장 (오류 없음)", err === 0, err ? await ai.locator(".error").innerText() : "");
  const cfg = fs.readFileSync(path.join(T, "config.toml"), "utf8");
  ok("B", "config.toml에 embed = openai / fake-embed", /embed *= *\{[^}]*fake-embed/.test(cfg), cfg.split("\n").find((l) => l.startsWith("embed")) ?? "");
  ok("B", "config.toml에 키 문자열 없음", !/sk-/.test(cfg));
  const embedded = await waitFor(async () => {
    const s = await bridge("semantic_status");
    return s && s.state === "ready" && s.files >= 4 && s.embedded === s.files;
  }, 40000);
  const s = await bridge("semantic_status");
  ok("B", "백그라운드 임베딩 완료", embedded, JSON.stringify(s));
  await tab(main, "홈").click();
  await sleep(800);
  const bar = await main.locator("[data-testid=statusbar]").innerText();
  ok("B", "상태바: 의미검색 켜짐", bar.includes("의미검색 켜짐"), bar.replace(/\n/g, " | "));
  await main.getByRole("button", { name: "전체 보기" }).click();
  await sleep(300);
  const feed = await main.locator("[data-testid=activity-feed]").innerText();
  ok("B", "활동: 의미검색 임베딩 기록", feed.includes("의미검색 임베딩"));
  await main.screenshot({ path: `${SHOT}/m7-B-main.png`, fullPage: true });
}

const search = await ctx.newPage();
await search.goto("http://localhost:1420/#/search");
await search.waitForSelector(".search-input");
await sleep(800);
const query = async (q, wait = 2500) => {
  await search.locator(".search-input").fill(q);
  await sleep(wait);
  const items = await search.locator(".results li").allInnerTexts();
  const foot = await search.locator(".search-foot").innerText();
  return { items, foot };
};

// C. natural-language search
{
  const ph = await search.locator(".search-input").getAttribute("placeholder");
  ok("C", "검색창 안내: 질문 가능", ph.includes("질문"), ph);
  const r1 = await query("작년 통신 요금 고지서 어디 있지?");
  ok("C", "질문형 → 단어 불일치 파일을 의미로 찾음 (1위 KT_고지서)", r1.items[0]?.includes("KT_고지서"), JSON.stringify(r1.items.slice(0, 3)));
  ok("C", "의미 배지 + 유사도", /의미 \d+%/.test(r1.items[0] ?? ""), r1.items[0]);
  ok("C", "하단: 단어+의미 (질문형)", r1.foot.includes("단어+의미") && r1.foot.includes("질문형"), r1.foot.replace(/\n/g, " | "));
  const preview = await search.locator(".preview").innerText();
  ok("C", "미리보기: 유사도 + 의미 설명", preview.includes("유사도") && preview.includes("뜻이"), preview.split("\n").filter((l) => /유사도|뜻이/.test(l)).join(" | "));
  await search.screenshot({ path: `${SHOT}/m7-C-question.png` });
  const r2 = await query("여행 관련 서류");
  ok("C", "여행 → 오사카_항공권 (동의어)", r2.items[0]?.includes("오사카_항공권"), JSON.stringify(r2.items.slice(0, 3)));
  const r3 = await query("회의록");
  ok("C", "키워드 질의: 단어 일치가 1위 (배지 없음)", r3.items[0]?.includes("회의록") && !r3.items[0].includes("의미"), JSON.stringify(r3.items.slice(0, 3)));
  const r4 = await query("계약 관련 문서 ext:md");
  ok("C", "필터(ext:md)는 의미 결과에도 적용", r4.items.length === 1 && r4.items[0].includes("부동산_서류.md"), JSON.stringify(r4.items));
  await search.screenshot({ path: `${SHOT}/m7-C-filter.png` });
}

// D. new file → indexed by the watcher → embedded in the background → found
{
  fs.writeFileSync(path.join(DESK, "제주_숙소_예약.txt"), "11월 제주 2박 펜션 예약 확인서");
  const done = await waitFor(async () => {
    const s = await bridge("semantic_status");
    return s && s.files >= 5 && s.embedded === s.files;
  }, 60000, 2000);
  ok("D", "새 파일 자동 임베딩 (≤60 s)", done, JSON.stringify(await bridge("semantic_status")));
  const r = await query("여행 계획");
  ok("D", "여행 계획 → 제주 숙소 + 오사카 항공권", r.items.slice(0, 2).some((i) => i.includes("제주_숙소")) && r.items.slice(0, 2).some((i) => i.includes("오사카")), JSON.stringify(r.items.slice(0, 3)));
}

// E. outage and recovery (CLAUDE.md rule 4)
{
  // Classify model on the same fake server (no key).
  await gotoSettings(main, "ai");
  const ai = main.locator("[data-testid=ai-section]");
  await ai.locator("select").first().selectOption("openai");
  await sleep(300);
  const rows = ai.locator("label.row");
  await rows.filter({ hasText: "모델" }).first().locator("input").fill("fake-chat");
  await rows.filter({ hasText: "주소" }).first().locator("input").fill(`${FAKE}/v1`);
  await rows.filter({ hasText: "키 이름" }).first().locator("input").fill("");
  await saveSettings(main, 3000);
  await tab(main, "홈").click();
  const ready = await waitFor(async () => (await bridge("get_status")).aiState === "ready", 15000);
  ok("E", "AI 분류 모델 준비됨 (상태바 AI 사용)", ready, (await bridge("get_status")).aiDetail ?? "");

  await control(true);
  const t0 = Date.now();
  for (const n of ["장애1.txt", "장애2.txt", "장애3.txt"]) {
    fs.writeFileSync(path.join(DESK, n), `장애 중에 들어온 파일 ${n}`);
    await sleep(3500);
  }
  const down = await waitFor(async () => (await bridge("get_status")).aiState === "unavailable", 20000);
  const st = await bridge("get_status");
  ok("E", "AI 연속 3회 실패 → aiState unavailable", down, st.aiDetail ?? "");
  const bar = await main.locator("[data-testid=statusbar]").innerText();
  ok("E", "상태바: AI 사용 불가", bar.includes("AI 사용 불가"), bar.replace(/\n/g, " | "));

  const r1 = await query("숙소 예약 확인서 어디 있지", 4000);
  ok("E", "임베딩 실패 → 단어 검색으로 폴백 + 안내", r1.foot.includes("단어 검색만"), r1.foot.replace(/\n/g, " | "));
  const r2 = await query("분기 실적", 4000);
  ok("E", "폴백 중에도 단어 검색 결과는 그대로", r2.items[0]?.includes("회의록"), JSON.stringify(r2.items.slice(0, 2)));
  await query("보증금 반환 일정 언제였지", 4000);
  const semDown = await waitFor(async () => (await bridge("get_status")).semanticState === "unavailable", 15000);
  ok("E", "의미검색도 연속 실패 → unavailable", semDown, (await bridge("get_status")).semanticDetail ?? "");
  const bar2 = await main.locator("[data-testid=statusbar]").innerText();
  ok("E", "상태바: 의미검색 일시 중단", bar2.includes("의미검색 일시 중단"), bar2.replace(/\n/g, " | "));
  await main.screenshot({ path: `${SHOT}/m7-E-down.png`, fullPage: true });
  const pendingBefore = (await bridge("list_pending")).length;

  await control(false);
  // Probes start 60 s after the AI went unavailable.
  const back = await waitFor(async () => (await bridge("get_status")).aiState === "ready", 120000, 3000);
  ok("E", "복구 → aiState ready (재시도 프로브)", back, `${Math.round((Date.now() - t0) / 1000)} s after outage start`);
  const requeued = await waitFor(async () => (await bridge("list_pending")).filter((p) => p.source === "ai").length >= 3, 30000);
  const pend = await bridge("list_pending");
  ok("E", "장애 중 건너뛴 파일 3개를 복구 후 AI가 다시 분류 (대기열)", requeued && pendingBefore === 0, `pending=${pend.map((p) => p.name).join(",")}`);
  const semBack = await waitFor(async () => {
    await query(`보증금 ${Date.now() % 1000}`, 2500);
    return (await bridge("get_status")).semanticState === "ready";
  }, 90000, 5000);
  ok("E", "복구 → 의미검색 ready", semBack);
  const r3 = await query("통신 요금 고지서 찾아줘");
  ok("E", "복구 후 의미검색 다시 동작", r3.foot.includes("단어+의미") && r3.items[0]?.includes("KT_고지서"), JSON.stringify(r3.items.slice(0, 2)));
  const bar3 = await main.locator("[data-testid=statusbar]").innerText();
  ok("E", "상태바: AI 사용 + 의미검색 켜짐", /AI 사용\s*·\s*fake-chat/.test(bar3) && bar3.includes("의미검색 켜짐"), bar3.replace(/\n/g, " | "));
  await main.screenshot({ path: `${SHOT}/m7-E-back.png`, fullPage: true });
  const s = await stats();
  ok("E", "가짜 서버 호출 확인 (실제 API 호출 없음)", s.calls > 0 && s.chats > 0, JSON.stringify(s));
}

// F. start at login off → login item removed
{
  await gotoSettings(main, "advanced");
  await main.getByRole("checkbox", { name: /로그인 시 자동 시작/ }).uncheck();
  await saveSettings(main, 2500);
  ok("F", "자동 시작 끔 → 시작 항목 삭제", !fs.existsSync(AUTOSTART));
  const cfg = fs.readFileSync(path.join(T, "config.toml"), "utf8");
  ok("F", "config.toml start_on_login = false", /start_on_login *= *false/.test(cfg));
}

fs.writeFileSync(path.join(process.env.S, "m7-results.json"), JSON.stringify(results, null, 2));
await browser.close();
fake.kill();
const fails = results.filter((r) => !r.pass).length;
console.log(`\n${results.length - fails}/${results.length} passed`);
process.exit(fails ? 1 : 0);
