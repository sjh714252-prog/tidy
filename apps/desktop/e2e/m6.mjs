// M6 GUI test (PR #3): "네트워크(NAS)" watch folder (simulated with a local
// dir): settings sub-row, polling-only detection, copy-verify move with
// progress, metadata-only index, offline → status bar warning → reconnect.
import { chromium } from "playwright";
import fs from "node:fs";
import path from "node:path";

const T = process.env.T;
const NAS = path.join(T, "nas");
// Destination on ANOTHER filesystem (tmpfs) so rename fails with EXDEV and
// the copy-verify path (with progress) is exercised, as with a real NAS.
const DEST = process.env.XDEV_DEST ?? "/dev/shm/tidy-e2e";
fs.rmSync(DEST, { recursive: true, force: true });
fs.mkdirSync(DEST, { recursive: true });
const LOCAL = path.join(DEST, "로컬 문서");
const SHOT = path.join(process.env.S, "shots");
fs.mkdirSync(SHOT, { recursive: true });
fs.mkdirSync(NAS, { recursive: true });
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
const events = async (since) => (await (await fetch(`http://127.0.0.1:1421/events?since=${since}`)).json());
const waitFor = async (fn, ms, step = 1000) => {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    if (await fn()) return true;
    await sleep(step);
  }
  return false;
};

const browser = await chromium.launch();
const ctx = await browser.newContext({ viewport: { width: 1000, height: 820 } });
// M10: settings sections are foldable; these older tests expect everything open.
await ctx.addInitScript(() =>
  localStorage.setItem("tidy.settings.sections", JSON.stringify({ watch: true, rules: true, context: true, ai: true, security: true, advanced: true })),
);
const main = await ctx.newPage();
main.on("dialog", (d) => d.accept(process.env.PROMPT_ANSWER ?? ""));
await main.goto("http://localhost:1420/#/main");
await main.waitForSelector(".topbar");
await sleep(800);
const evStart = (await events(1e9)).next;

// A. settings: add folder, switch kind to 네트워크(NAS) → sub-row, poll 5 s, polling only
{
  await main.getByRole("button", { name: "설정", exact: true }).click();
  await sleep(500);
  process.env.PROMPT_ANSWER = NAS;
  await main.getByRole("button", { name: "폴더 추가…" }).click();
  await sleep(500);
  const row = main.locator(".settings tbody tr").first();
  await row.locator("select").first().selectOption("network"); // kind (2nd select = M8 정리 방식)
  await sleep(300);
  const sub = main.locator(".settings tbody tr.subrow:not(.exclude-row)");
  ok("A", "네트워크 선택 → 폴링/시간대 하위 행 표시", (await sub.count()) === 1);
  const idx = await row.locator("input[type=checkbox]").nth(1).isChecked();
  ok("A", "NAS는 본문 인덱싱 기본 꺼짐 (메타만)", idx === false);
  await sub.locator("input[type=number]").fill("5");
  await sub.locator("label", { hasText: "주기 확인만 사용" }).locator("input").check();
  await openAdvanced(main);
  const ta = main.locator("textarea.rules");
  await ta.fill(
    `[[rule]]\nname = "NAS 문서"\nmatch = { ext = ["pdf"] }\ndest = '${LOCAL}'\n\n` + (await ta.inputValue()),
  );
  await main.getByRole("button", { name: "저장", exact: true }).click();
  await sleep(3000);
  const bar = await main.locator(".statusbar").innerText();
  ok("A", "저장 → 감시 중: nas", bar.includes("nas"), bar.replace(/\n/g, " | "));
  const cfg = fs.readFileSync(path.join(T, "config.toml"), "utf8");
  ok("A", "config.toml에 network/poll 설정", /network/.test(cfg) && /poll_interval_s *= *5/.test(cfg) && /poll_only *= *true/.test(cfg), cfg.split("\n").filter((l) => /kind|poll/.test(l)).join(" | "));
  await main.screenshot({ path: `${SHOT}/m6-A-settings.png`, fullPage: true });
}

// B. polling picks up a new pdf; copy-verify move with progress for a big file
{
  await main.locator(".topbar nav").getByRole("button", { name: "홈" }).click();
  fs.writeFileSync(path.join(NAS, "nas-small.pdf"), "small");
  const movedSmall = await waitFor(() => fs.existsSync(path.join(LOCAL, "nas-small.pdf")), 20000);
  ok("B", "폴링으로 새 파일 감지 → 규칙대로 이동 (≤20 s)", movedSmall);
  // The activity event reaches the page shortly after the file lands on disk.
  await waitFor(async () => /nas-small\.pdf/.test(await main.locator(".activity").innerText()), 5000, 300);
  const feed = await main.locator(".activity").innerText();
  ok("B", "활동 탭에 이동 기록", /nas-small\.pdf/.test(feed) && /이동/.test(feed));

  // 1 GB so the copy outlasts the 250 ms first-report delay (tmpfs is fast)
  const bigPath = path.join(NAS, "nas-big.pdf");
  const fd = fs.openSync(bigPath, "w");
  const chunk = Buffer.alloc(8 * 1024 * 1024, 7);
  for (let i = 0; i < 128; i++) fs.writeSync(fd, chunk);
  fs.closeSync(fd);
  const movedBig = await waitFor(() => fs.existsSync(path.join(LOCAL, "nas-big.pdf")) && !fs.existsSync(bigPath), 60000);
  ok("B", "큰 파일 복사-검증 이동 완료, 원본 제거", movedBig);
  const ev = (await events(evStart)).events;
  const prog = ev.filter((e) => e.name === "tidy://progress").map((e) => e.payload);
  const phases = [...new Set(prog.map((p) => p.phase))];
  ok("B", "진행률 이벤트(copy/verify/done) 발생", prog.length > 0 && phases.includes("done"), `n=${prog.length} phases=${phases.join(",")}`);
  const size = fs.statSync(path.join(LOCAL, "nas-big.pdf")).size;
  ok("B", "복사본 크기 일치 (1 GB)", size === 128 * 8 * 1024 * 1024, `${size}`);
  const log = await bridge("list_batches", { limit: 10 });
  ok("B", "move_log에 기록 (undo 가능)", log.some((b) => /nas-big|nas-small/.test(b.example ?? "")), JSON.stringify(log[0] ?? {}).slice(0, 120));
}

// C. metadata-only index: name search hits, no body text
{
  await sleep(3000);
  const s = await ctx.newPage();
  await s.goto("http://localhost:1420/#/search");
  await s.locator(".search-input").fill("nas-small");
  await sleep(1500);
  const names = await s.locator(".results li").allInnerTexts();
  const prev = await s.locator(".preview").innerText().catch(() => "");
  ok("C", "이름으로 검색됨", names.some((n) => n.includes("nas-small")), names.join(" | ").slice(0, 100));
  ok("C", "본문은 인덱싱 안 됨(메타만) → 본문 일치 없음/본문 없음", /본문/.test(prev), prev.replace(/\n/g, " | ").slice(0, 120));
  await s.close();
}

// D. offline → status bar warning; back online → warning gone, polling resumes
{
  fs.renameSync(NAS, NAS + ".offline");
  const off = await waitFor(async () => (await bridge("get_status")).offline.length > 0, 30000);
  ok("D", "폴더 사라짐 → status.offline (≤30 s)", off, JSON.stringify((await bridge("get_status")).offline));
  await sleep(1500);
  const bar = await main.locator(".statusbar").innerText();
  ok("D", "상태바 '연결 끊김(일시정지)'", bar.includes("연결 끊김"), bar.replace(/\n/g, " | "));
  const feed = await main.locator(".activity").innerText();
  ok("D", "활동 탭에 끊김 알림", /(끊김|오프라인|unreachable|연결)/.test(feed));
  await main.screenshot({ path: `${SHOT}/m6-D-offline.png` });

  fs.renameSync(NAS + ".offline", NAS);
  const on = await waitFor(async () => (await bridge("get_status")).offline.length === 0, 90000, 2000);
  ok("D", "폴더 복귀 → 재연결 (≤90 s, SPEC 60 s 재확인)", on);
  fs.writeFileSync(path.join(NAS, "after-reconnect.pdf"), "back");
  const moved = await waitFor(() => fs.existsSync(path.join(LOCAL, "after-reconnect.pdf")), 30000);
  ok("D", "재연결 후 새 파일 다시 처리", moved);
  const bar2 = await main.locator(".statusbar").innerText();
  ok("D", "상태바 경고 사라짐", !bar2.includes("연결 끊김"), bar2.replace(/\n/g, " | "));
}

// E. undo of a copy-verify move restores the file to the NAS folder
{
  await main.locator(".topbar nav").getByRole("button", { name: "되돌리기" }).click();
  await sleep(1200);
  const row = main.locator(".history tbody tr", { hasText: "after-reconnect" }).first();
  await row.getByRole("button", { name: "되돌리기", exact: true }).click();
  await sleep(3000);
  ok("E", "NAS로 되돌리기", fs.existsSync(path.join(NAS, "after-reconnect.pdf")) && !fs.existsSync(path.join(LOCAL, "after-reconnect.pdf")));
}

fs.writeFileSync(path.join(process.env.S, "m6-results.json"), JSON.stringify(results, null, 2));
await browser.close();
const fails = results.filter((r) => !r.pass).length;
console.log(`\n${results.length - fails}/${results.length} passed`);
