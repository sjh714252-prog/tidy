// M16 GUI test: organizing on the NAS (SPEC §14.13). A real `tidy serve`
// agent plays the NAS (data root $T/nas), the app connects to it through
// 설정 › NAS and everything is driven through the dev bridge:
//
//   A. 설정 › NAS: make the docker-compose file (share 업무), connect with
//      the password → panel "연결됨"
//   B. pick the NAS folder 업무 in the NAS folder tree, rules in the NAS
//      rules.toml, "NAS에 저장" → the agent watches it
//   C. [지금 정리] moves the file that was already there (on the NAS)
//   D. a new PDF is moved by the NAS on its own; a memo waits in
//      "NAS 확인 대기" (확인 대기 tab) → 이동
//   E. search window lists NAS results after this PC's with the NAS badge
//   F. home NAS card; 되돌리기 › NAS에서 한 일 → undo puts the file back
//
// Run: S=/tmp/tidy-e2e bash e2e/run-headless.sh m16
import { chromium } from "playwright";
import { spawn } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { bridge, gotoSettings, sleep, tab, waitFor } from "./lib.mjs";

const T = process.env.T;
const S = process.env.S;
const SHOT = path.join(S, "shots");
const ROOT = path.join(import.meta.dirname, "..", "..", "..");
const NAS = path.join(T, "nas");
const WORK = path.join(NAS, "업무");
const PORT = 18741;
fs.mkdirSync(SHOT, { recursive: true });
fs.mkdirSync(WORK, { recursive: true });
fs.mkdirSync(path.join(NAS, "사진"), { recursive: true });
fs.writeFileSync(path.join(WORK, "기존 보고서.pdf"), "%PDF-1.4 old report");
fs.writeFileSync(path.join(NAS, "사진", "견적 메모 원본.txt"), "가람정보 견적 합계");

let agent = null;
process.on("exit", () => agent?.kill());
/** Start the "NAS" with the password from the compose file (as the NAS would). */
async function startAgent(token) {
  agent = spawn(path.join(ROOT, "target", "debug", "tidy"), ["serve"], {
    env: {
      ...process.env,
      TIDY_TOKEN: token,
      TIDY_DATA_ROOT: NAS,
      TIDY_LISTEN: `127.0.0.1:${PORT}`,
      TIDY_CONFIG: path.join(T, "nas-config", "config.toml"),
    },
    stdio: ["ignore", fs.openSync(path.join(S, "agent-m16.log"), "w"), fs.openSync(path.join(S, "agent-m16.err"), "w")],
  });
  await waitFor(async () => {
    try {
      return (await fetch(`http://127.0.0.1:${PORT}/api/v1/hello`)).ok;
    } catch {
      return false;
    }
  }, 20000);
}

const results = [];
const ok = (step, what, pass, note = "") => {
  results.push({ step, what, pass, note });
  console.log(`${pass ? "PASS" : "FAIL"} [${step}] ${what} ${note}`);
};

const browser = await chromium.launch();
const ctx = await browser.newContext({ viewport: { width: 1280, height: 900 } });
const main = await ctx.newPage();
main.on("dialog", (d) => void d.accept());
await main.goto("http://localhost:1420/#/main");
await main.waitForSelector(".topbar");
await sleep(1000);

// A. compose file + connect
{
  await gotoSettings(main, "nas");
  const setup = main.locator("[data-testid=nas-setup]");
  await setup.getByLabel("NAS 이름").fill("사무실");
  await setup.getByLabel("공유 폴더").fill("업무, 사진");
  await setup.locator("[data-testid=nas-make-compose]").click();
  await main.waitForSelector("[data-testid=nas-compose]");
  const y = await main.locator("[data-testid=nas-compose]").inputValue();
  ok("A", "설치 파일: 연결 암호·공유 폴더 마운트", /TIDY_TOKEN: "[0-9a-f]{64}"/.test(y) && y.includes('"/volume1/업무:/data/업무"'), "");
  await setup.screenshot({ path: `${SHOT}/m16-A-setup.png` });
  // "Upload to the NAS": the agent runs with the compose file's password;
  // the app already has it in the keychain, so none is typed.
  await startAgent(y.match(/TIDY_TOKEN: "([0-9a-f]{64})"/)[1]);
  await setup.getByLabel("NAS 주소").fill(`127.0.0.1:${PORT}`);
  await setup.locator("[data-testid=nas-connect]").click();
  const connected = await waitFor(async () => (await main.locator("[data-testid=nas-status]").count()) > 0, 10000);
  const st = connected ? await main.locator("[data-testid=nas-status]").innerText() : "";
  ok("A", "연결됨 패널", st.startsWith("연결됨"), st);
  const cfg = fs.readFileSync(path.join(T, "config.toml"), "utf8");
  ok("A", "config.toml [[nas]] (암호는 키체인, 파일에 없음)", cfg.includes("[[nas]]") && cfg.includes("사무실") && !/[0-9a-f]{64}/.test(cfg));
}

// B. watch folder + rules → save on the NAS
{
  const panel = main.locator("[data-testid=nas-panel]");
  await panel.locator("[data-testid=nas-add-watch]").click();
  const picker = panel.locator("[data-testid=nas-picker]");
  await picker.locator(".tree-row .twisty").first().click();
  await sleep(500);
  await picker.locator(".tree-name", { hasText: "업무" }).click();
  await picker.locator("[data-testid=nas-pick-ok]").click();
  // 사진: searched only.
  await panel.locator(".nas-index-list button", { hasText: "+ 폴더 추가" }).click();
  await picker.locator(".tree-row .twisty").first().click();
  await sleep(500);
  await picker.locator(".tree-name", { hasText: "사진" }).click();
  await picker.locator("[data-testid=nas-pick-ok]").click();
  await panel.locator("summary", { hasText: "rules.toml 직접 편집" }).click();
  await panel
    .getByLabel("NAS rules.toml")
    .fill(
      '[[rule]]\nname = "PDF"\nmatch = { ext = ["pdf"] }\ndest = "문서"\n\n[[rule]]\nname = "메모"\nmatch = { ext = ["txt"] }\ndest = "메모"\nconfirm = true\n',
    );
  await sleep(500);
  const cards = await panel.locator("[data-testid=rule-card]").count();
  ok("B", "규칙 카드 2개", cards === 2, `${cards}`);
  await panel.locator("[data-testid=nas-save]").click();
  await waitFor(async () => (await panel.locator("[data-testid=nas-msg]").count()) > 0, 10000);
  const watching = await waitFor(async () => {
    const l = await bridge("nas_list");
    return l[0]?.status?.watching && l[0].status.watches.length === 1;
  }, 15000);
  ok("B", "NAS가 업무 폴더 감시", watching);
  const agentCfg = fs.readFileSync(path.join(T, "nas-config", "config.toml"), "utf8");
  ok("B", "NAS config: 감시 폴더 업무 + 검색만 할 폴더 사진", agentCfg.includes(WORK) && agentCfg.includes(path.join(NAS, "사진")), "");
  await panel.screenshot({ path: `${SHOT}/m16-B-panel.png` });
}

// C. 지금 정리 (existing file, on the NAS)
{
  const panel = main.locator("[data-testid=nas-panel]");
  await panel.locator("[data-testid=nas-organize]").click();
  const moved = await waitFor(() => fs.existsSync(path.join(WORK, "문서", "기존 보고서.pdf")), 10000);
  ok("C", "지금 정리: 기존 보고서.pdf → 업무/문서", moved);
  const msg = await panel.locator("[data-testid=nas-msg]").innerText();
  ok("C", "결과 문구", msg.includes("1개를 정리했습니다"), msg);
}

// D. new files while the NAS watches
{
  fs.writeFileSync(path.join(WORK, "새 견적서.pdf"), "%PDF-1.4 new quote");
  const moved = await waitFor(() => fs.existsSync(path.join(WORK, "문서", "새 견적서.pdf")), 20000);
  ok("D", "새 PDF를 NAS가 직접 정리", moved);
  fs.writeFileSync(path.join(WORK, "회의 메모.txt"), "다음 주 가람정보 견적 확인");
  await waitFor(async () => (await bridge("nas_pending")).items.length > 0, 20000);
  await tab(main, "확인 대기").click();
  await main.waitForSelector("[data-testid=nas-pending]");
  await main.locator("[data-testid=nas-pending] button", { hasText: "새로고침" }).click();
  await main.waitForSelector("[data-testid=nas-pending-row]");
  const row = main.locator("[data-testid=nas-pending-row]").first();
  const text = await row.innerText();
  ok("D", "NAS 확인 대기 행: 이름·NAS·규칙", text.includes("회의 메모.txt") && text.includes("NAS · 사무실") && text.includes("규칙: 메모"), text.replace(/\n/g, " | "));
  await main.locator("[data-testid=nas-pending]").screenshot({ path: `${SHOT}/m16-D-pending.png` });
  await row.locator("[data-testid=nas-pending-move]").click();
  const done = await waitFor(() => fs.existsSync(path.join(WORK, "메모", "회의 메모.txt")), 10000);
  ok("D", "이동 → NAS에서 업무/메모로", done);
  const left = await main.locator("[data-testid=nas-pending-row]").count();
  ok("D", "대기 목록 비움", left === 0, `${left}`);
}

// E. search window: NAS results with the badge
{
  const s = await ctx.newPage();
  await s.goto("http://localhost:1420/#/search");
  await s.waitForSelector(".search-input");
  let rows = [];
  await waitFor(async () => {
    await s.locator(".search-input").fill("");
    await s.locator(".search-input").fill("가람정보");
    await sleep(1200);
    rows = await s.locator(".results li[data-nas]").allInnerTexts();
    return rows.length >= 2;
  }, 30000, 500);
  ok("E", "NAS 결과 2건 이상 (본문 '가람정보')", rows.length >= 2, rows.join(" / ").replace(/\n/g, " "));
  ok("E", "NAS 배지", (await s.locator("[data-testid=nas-badge]").count()) >= 2);
  await s.locator(".results li[data-nas]").first().hover();
  const note = await s.locator("[data-testid=nas-note]").innerText();
  ok("E", "미리보기: NAS에 있는 파일", note.includes("NAS(사무실)"), note);
  const foot = await s.locator(".search-foot").innerText();
  ok("E", "아래줄 NAS 건수", /NAS \d+건/.test(foot), foot);
  await s.screenshot({ path: `${SHOT}/m16-E-search.png` });
  await s.close();
}

// F. home card + undo on the NAS
{
  await tab(main, "홈").click();
  await main.waitForSelector("[data-testid=nas-card]");
  await sleep(500);
  const home = await main.locator("[data-testid=nas-row]").innerText();
  ok("F", "홈 NAS 카드: 오늘 정리 수", /오늘 [3-9]개 정리/.test(home), home.replace(/\n/g, " | "));
  await main.locator("[data-testid=nas-card]").screenshot({ path: `${SHOT}/m16-F-home.png` });
  await tab(main, "되돌리기").click();
  await main.waitForSelector("[data-testid=nas-history]");
  await waitFor(async () => (await main.locator("[data-testid=nas-batch-row]").count()) >= 3, 10000);
  const rows = await main.locator("[data-testid=nas-batch-row]").count();
  ok("F", "NAS에서 한 일: 묶음 3개 이상", rows >= 3, `${rows}`);
  // The organize batch is the oldest → last row.
  const last = main.locator("[data-testid=nas-batch-row]").nth(rows - 1);
  await last.locator("[data-testid=nas-undo]").click();
  const back = await waitFor(() => fs.existsSync(path.join(WORK, "기존 보고서.pdf")), 10000);
  ok("F", "되돌리기 → NAS에서 원래 자리로", back && !fs.existsSync(path.join(WORK, "문서", "기존 보고서.pdf")));
  const msg = await main.locator("[data-testid=nas-history-msg]").innerText();
  ok("F", "결과 문구", msg.includes("NAS에서 1개 되돌림"), msg);
  await main.locator("[data-testid=nas-history]").screenshot({ path: `${SHOT}/m16-F-history.png` });
}

// G. NAS off → "연결 안 됨", the app keeps working
{
  agent.kill();
  await sleep(500);
  await tab(main, "홈").click();
  await main.locator("[data-testid=nas-card] button", { hasText: "새로 고침" }).click();
  const off = await waitFor(async () => (await main.locator("[data-testid=nas-row]").innerText()).includes("연결 안 됨"), 15000);
  ok("G", "NAS 꺼짐 → 연결 안 됨", off);
  const s = await ctx.newPage();
  await s.goto("http://localhost:1420/#/search");
  await s.waitForSelector(".search-input");
  await s.locator(".search-input").fill("보고서");
  await sleep(2500);
  const foot = await s.locator(".search-foot").innerText();
  ok("G", "검색은 그대로, NAS 응답 없음 표시", foot.includes("NAS 응답 없음"), foot);
}

await browser.close();
const failed = results.filter((r) => !r.pass);
console.log(`\n${results.length - failed.length}/${results.length} passed`);
fs.writeFileSync(path.join(S, "m16-results.json"), JSON.stringify(results, null, 2));
process.exit(failed.length ? 1 : 0);
