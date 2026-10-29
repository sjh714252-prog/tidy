// M16 GUI test: pictures are found and sorted by what they show (SPEC
// §14.13), driven through the dev bridge, with the fake LLM
// (e2e/fake-llm.mjs, OpenAI-compatible) as the vision model.
//
//   A. AI = fake server; 설정 › AI "사진 내용 인식" card: off by default,
//      switch on → config.toml [vision] enabled = true, the model and the
//      "보냅니다" privacy line are shown
//   B. two pictures already on the desktop → watch folder added →
//      background descriptions ("설명 붙임 2장", pictures sent as images)
//   C. search window: "화이트보드" (only in the description) finds the
//      meeting picture, with the "사진 설명" badge
//   D. a new camera picture (IMG_…) dropped while watching → described
//      before the AI picks its folder (the classify prompt has
//      "[사진 설명] 카페에서 받은 종이 영수증"), later found by "영수증"
//      without sending it again
//   E. model + daily limit saved → config; switch off → enabled = false
//
// Run: S=/tmp/tidy-e2e bash e2e/run-headless.sh m16
import { chromium } from "playwright";
import { spawn } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import zlib from "node:zlib";
import { addWatchFolder, bridge, gotoSettings, saveSettings, sleep, waitFor } from "./lib.mjs";

const T = process.env.T;
const S = process.env.S;
const CFG = path.join(T, "config.toml");
const DESK = path.join(T, "바탕 화면");
const SHOT = path.join(S, "shots");
fs.mkdirSync(SHOT, { recursive: true });
const LLM_PORT = 18095;
const LLM = `http://127.0.0.1:${LLM_PORT}`;
const stats = async () => (await fetch(`${LLM}/stats`)).json();

const results = [];
const ok = (step, what, pass, note = "") => {
  results.push({ step, what, pass, note });
  console.log(`${pass ? "PASS" : "FAIL"} [${step}] ${what} ${note}`);
};
const searchNames = async (q) =>
  (await bridge("search", { query: q, limit: 20, customer: null })).map((h) => path.basename(h.path));

/** A noisy RGB PNG (~100 KB: a picture, not an icon); `seed` changes it. */
function png(file, seed, w = 200, h = 160) {
  const rows = Buffer.alloc((w * 3 + 1) * h);
  let x = seed * 2654435761;
  for (let i = 0; i < rows.length; i++) {
    x = (x * 1103515245 + 12345) >>> 0;
    rows[i] = i % (w * 3 + 1) === 0 ? 0 : x >>> 24;
  }
  const chunk = (type, data) => {
    const len = Buffer.alloc(4);
    len.writeUInt32BE(data.length);
    const td = Buffer.concat([Buffer.from(type), data]);
    const crc = Buffer.alloc(4);
    crc.writeUInt32BE(zlib.crc32(td));
    return Buffer.concat([len, td, crc]);
  };
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(w, 0);
  ihdr.writeUInt32BE(h, 4);
  ihdr.set([8, 2, 0, 0, 0], 8);
  fs.writeFileSync(
    file,
    Buffer.concat([
      Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
      chunk("IHDR", ihdr),
      chunk("IDAT", zlib.deflateSync(rows)),
      chunk("IEND", Buffer.alloc(0)),
    ]),
  );
}

// ── fake LLM (fast) ─────────────────────────────────────────────────────
const llm = spawn(process.execPath, [path.join(import.meta.dirname, "fake-llm.mjs"), String(LLM_PORT)], {
  stdio: ["ignore", "inherit", "inherit"],
  env: { ...process.env, FAKE_DELAY_MS: "50" },
});
process.on("exit", () => llm.kill());
await sleep(800);

png(path.join(DESK, "회의 사진.png"), 1);
png(path.join(DESK, "책상.png"), 2);

const browser = await chromium.launch();
const ctx = await browser.newContext({ viewport: { width: 1280, height: 960 } });
const main = await ctx.newPage();
main.on("dialog", (d) => {
  if (d.type() === "prompt") void d.accept(process.env.PROMPT_ANSWER ?? "");
  else void d.accept();
});
await main.goto("http://localhost:1420/#/main");
await main.waitForSelector(".topbar");
await sleep(1000);

// A. AI + the card
{
  await gotoSettings(main, "ai");
  const ai = main.locator("[data-testid=ai-section]");
  await ai.locator("label.row", { hasText: "AI 사용" }).locator("input[type=checkbox]").check();
  await ai.locator("select").first().selectOption("openai");
  await sleep(300);
  const arows = ai.locator("label.row");
  await arows.filter({ hasText: "모델" }).first().locator("input").fill("fake-slow");
  await arows.filter({ hasText: "주소" }).first().locator("input").fill(`${LLM}/v1`);
  await arows.filter({ hasText: "키 이름" }).first().locator("input").fill("");
  await saveSettings(main, 3000);

  const card = main.locator("[data-testid=vision-card]");
  await card.waitFor();
  const sw = card.locator("[data-testid=vision-enabled]");
  ok("A", "사진 내용 인식: 기본 꺼짐", !(await sw.isChecked()) && !(await bridge("vision_status")).enabled);
  await sw.check();
  await waitFor(async () => (await bridge("vision_status")).enabled, 5000);
  await sleep(800);
  const cfg = fs.readFileSync(CFG, "utf8");
  ok("A", "켜기 → config.toml [vision] enabled = true", /\[vision\][\s\S]*enabled = true/.test(cfg));
  const text = await card.innerText();
  ok(
    "A",
    "카드: 모델·보냄 안내·오늘 한도",
    text.includes("openai · fake-slow") && text.includes("AI 회사(openai)로 보냅니다") && text.includes("/200장"),
    text.replace(/\n/g, " | "),
  );
}

// B. watch folder → descriptions in the background
{
  await gotoSettings(main, "watch");
  await addWatchFolder(main, DESK);
  await saveSettings(main, 3500);
  // The watch loop (and with it the AI) starts with the first watch folder.
  const ready = await waitFor(async () => (await bridge("get_status")).aiState === "ready", 20000);
  ok("B", "AI 준비됨 (가짜 서버)", ready, (await bridge("get_status")).aiDetail ?? "");
  const done = await waitFor(async () => (await bridge("vision_status")).ok >= 2, 60000, 1500);
  const st = await bridge("vision_status");
  const s = await stats();
  ok("B", "백그라운드 설명: 설명 붙임 2장", done, JSON.stringify(st));
  ok("B", "사진으로 보냄 (이미지 2장, image/png)", s.images >= 2 && s.lastImageType.startsWith("image/"), `${s.images} ${s.lastImageType}`);
  await gotoSettings(main, "ai");
  const card = main.locator("[data-testid=vision-card]");
  await sleep(5500); // card refreshes every 5 s
  const text = await card.innerText();
  ok("B", "카드: 설명 붙임 2장", text.includes("설명 붙임 2장"), text.replace(/\n/g, " | "));
  await card.screenshot({ path: `${SHOT}/m16-B-card.png` });
}

// C. search window
{
  const s = await ctx.newPage();
  await s.goto("http://localhost:1420/#/search");
  await s.waitForSelector(".search-input");
  await s.locator(".search-input").fill("화이트보드");
  await sleep(1500);
  const rows = await s.locator(".results li").allInnerTexts();
  ok("C", "'화이트보드'(사진 설명에만 있음) → 회의 사진.png", rows.some((r) => r.includes("회의 사진.png")), rows.join(" / ").replace(/\n/g, " "));
  ok("C", "'사진 설명' 표시", (await s.locator("[data-testid=vision-badge]").count()) >= 1);
  await s.screenshot({ path: `${SHOT}/m16-C-search.png` });
  await s.close();
}

// D. a new camera picture while watching
{
  const before = (await stats()).images;
  png(path.join(DESK, "IMG_2041.png"), 3);
  const asked = await waitFor(
    async () => (await stats()).classifyUsers.some((u) => u.includes("IMG_2041.png")),
    40000,
    1000,
  );
  const users = (await stats()).classifyUsers.filter((u) => u.includes("IMG_2041.png"));
  ok(
    "D",
    "새 사진: 분류 요청에 '[사진 설명] 카페에서 받은 종이 영수증'",
    asked && users.some((u) => u.includes("[사진 설명] 카페에서 받은 종이 영수증")),
    (users[0] ?? "").split("\n").slice(0, 12).join(" | "),
  );
  const found = await waitFor(async () => (await searchNames("영수증")).includes("IMG_2041.png"), 40000, 1000);
  ok("D", "'영수증'으로 찾힘", found, (await searchNames("영수증")).join(", "));
  const after = (await stats()).images;
  ok("D", "분류 때 한 번만 보냄 (인덱싱은 캐시)", after - before === 1, `${before} → ${after}`);
}

// E. model / limit / off
{
  await gotoSettings(main, "ai");
  const card = main.locator("[data-testid=vision-card]");
  await card.locator("[data-testid=vision-model]").fill("fake-vision");
  await card.locator("[data-testid=vision-limit]").fill("50");
  await card.locator("[data-testid=vision-save]").click();
  await sleep(1500);
  let cfg = fs.readFileSync(CFG, "utf8");
  ok("E", "모델·하루 최대 저장", /model = "fake-vision"/.test(cfg) && /daily_limit = 50/.test(cfg), cfg.split("[vision]")[1]?.slice(0, 120));
  const st = await bridge("vision_status");
  ok("E", "상태: 사진용 모델 반영", st.model === "openai · fake-vision" && st.dailyLimit === 50, JSON.stringify(st.model));
  await card.locator("[data-testid=vision-enabled]").uncheck();
  await sleep(1500);
  cfg = fs.readFileSync(CFG, "utf8");
  ok("E", "끄기 → enabled = false", /\[vision\][\s\S]*enabled = false/.test(cfg) && !(await bridge("vision_status")).enabled);
}

const failed = results.filter((r) => !r.pass);
console.log(`\n${results.length - failed.length}/${results.length} passed`);
fs.writeFileSync(path.join(S, "m16-results.json"), JSON.stringify(results, null, 2));
await browser.close();
process.exit(failed.length ? 1 : 0);
