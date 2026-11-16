// 0.18.0 (SPEC §14.24): files placed by what their names say, and the
// "어느 고객사 로그인가요?" question in 확인 대기 / the popup.
// Fixtures: fix-sense.setup.mjs.
//
// Run: S=/tmp/tidy-e2e bash e2e/run-headless.sh fix-sense
import { chromium } from "playwright";
import fs from "node:fs";
import path from "node:path";
import { sleep, tab as tabOf, waitFor } from "./lib.mjs";

const T = process.env.T;
const S = process.env.S;
const DESK = path.join(T, "바탕 화면");
const ORG = path.join(T, "정리");
const SHOT = path.join(S, "shots");
fs.mkdirSync(SHOT, { recursive: true });

const results = [];
const ok = (step, what, pass, note = "") => {
  results.push({ step, what, pass, note });
  console.log(`${pass ? "PASS" : "FAIL"} [${step}] ${what} ${note}`);
};
const put = (n) => fs.writeFileSync(path.join(DESK, n), "PK");
const find = (dir, name) => {
  for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, e.name);
    if (e.isDirectory()) {
      const r = find(p, name);
      if (r) return r;
    } else if (e.name === name) return p;
  }
  return null;
};
const rel = (p) => (p ? path.relative(ORG, p) : "(없음)");

const browser = await chromium.launch();
const ctx = await browser.newContext({ viewport: { width: 1280, height: 860 } });
const main = await ctx.newPage();
main.on("dialog", (d) => void d.accept());
await main.goto("http://localhost:1420/#/main");
await main.waitForSelector(".topbar");
await sleep(1500);
const tab = (name) => tabOf(main, name);

// A. names say where: customer day bundles, a manual, a runtime
{
  for (const n of ["현대홈쇼핑_13일.zip", "현대홈쇼핑_CAMP_13일.zip", "KTRCS_MANUAL_v2.0.zip", "jre-8u121-linux-x64.zip", "사진 모음.zip"]) put(n);
  await waitFor(() => !fs.existsSync(path.join(DESK, "jre-8u121-linux-x64.zip")) && !fs.existsSync(path.join(DESK, "현대홈쇼핑_CAMP_13일.zip")), 30000);
  await sleep(1500);
  const a = find(ORG, "현대홈쇼핑_13일.zip");
  const b = find(ORG, "현대홈쇼핑_CAMP_13일.zip");
  ok("A", "고객사 로그 묶음 → 현대홈쇼핑/로그/…-13 로그 (둘이 같은 폴더)", !!a && path.dirname(a) === path.dirname(b ?? "") && /^현대홈쇼핑[\\/]로그[\\/]\d{4}-\d{2}-13 로그$/.test(rel(path.dirname(a))), rel(a));
  ok("A", "매뉴얼 → 서비스/RCS/매뉴얼", fs.existsSync(path.join(ORG, "서비스", "RCS", "매뉴얼", "KTRCS_MANUAL_v2.0.zip")), rel(find(ORG, "KTRCS_MANUAL_v2.0.zip")));
  ok("A", "JRE → 설치파일/Java/Linux", fs.existsSync(path.join(ORG, "설치파일", "Java", "Linux", "jre-8u121-linux-x64.zip")), rel(find(ORG, "jre-8u121-linux-x64.zip")));
  ok("A", "그 밖의 zip은 규칙대로 압축파일", fs.existsSync(path.join(ORG, "압축파일", "사진 모음.zip")));
}

// B. a log nobody's name is on → 확인 대기 asks which customer
const LOG = "m2x_rcs_2026-06-28_1.log.zip";
{
  put(LOG);
  await tab("확인 대기").click();
  const ask = main.locator("[data-testid=customer-ask]");
  await waitFor(async () => (await ask.count()) >= 1, 20000, 700);
  ok("B", "고객사 모르는 로그: 그대로 있고 '어느 고객사 로그인가요?'", fs.existsSync(path.join(DESK, LOG)) && (await ask.count()) === 1 && (await ask.innerText()).includes("어느 고객사 로그인가요?"));
  const names = await ask.locator("[data-testid=customer-choice]").allInnerTexts();
  ok("B", "기존 고객사 폴더 목록", ["가람정보", "신세계", "현대홈쇼핑"].every((n) => names.some((t) => t.startsWith(n))), names.join(","));
  await ask.locator("[data-testid=customer-search]").fill("가람");
  await sleep(300);
  const filtered = await ask.locator("[data-testid=customer-choice]").allInnerTexts();
  ok("B", "검색 '가람' → 가람정보만", filtered.length === 1 && filtered[0].startsWith("가람정보"), filtered.join(","));
  ok("B", "다음부터 묻지 않기(m2x_rcs) 체크", (await ask.innerText()).includes("m2x_rcs"));
  await main.screenshot({ path: `${SHOT}/fix-sense-B-ask.png` });
  await ask.locator("[data-testid=customer-choice]").first().click();
  const to = path.join(ORG, "가람정보", "로그", "2026-06-28 로그", LOG);
  ok("B", "고르면 → 가람정보/로그/2026-06-28 로그", await waitFor(() => fs.existsSync(to), 10000), rel(find(ORG, LOG)));
  const dict = fs.existsSync(path.join(T, "context.toml")) ? fs.readFileSync(path.join(T, "context.toml"), "utf8") : "";
  ok("B", "context.toml에 가람정보 별칭 m2x_rcs", dict.includes("가람정보") && dict.includes("m2x_rcs"), dict.replace(/\n/g, " | ").slice(0, 200));
}

// C. the same family later: no question, straight to that customer
{
  put("m2x_rcs_2026-07-01_2.log.zip");
  const to = path.join(ORG, "가람정보", "로그", "2026-07-01 로그", "m2x_rcs_2026-07-01_2.log.zip");
  ok("C", "같은 이름의 다음 로그는 묻지 않고 가람정보로", await waitFor(() => fs.existsSync(to), 20000), rel(find(ORG, "m2x_rcs_2026-07-01_2.log.zip")));
}

// D. popup: new name, and 그대로 두기
{
  put("abc_gw_2026-07-02.log.zip");
  put("zz_relay_2026-07-03.log.zip");
  const pop = await ctx.newPage();
  pop.on("dialog", (d) => void d.accept());
  await pop.goto("http://localhost:1420/#/confirm");
  await waitFor(async () => (await pop.locator("[data-testid=customer-ask]").count()) === 1, 20000, 700);
  ok("D", "팝업 질문: 어느 고객사 로그인가요?", (await pop.locator("[data-testid=confirm-question]").innerText()).includes("어느 고객사"));
  await pop.screenshot({ path: `${SHOT}/fix-sense-D-popup.png` });
  const first = await pop.locator("[data-testid=pending-name]").innerText();
  await pop.locator("[data-testid=customer-search]").fill("롯데홈쇼핑");
  await sleep(300);
  await pop.locator("[data-testid=customer-new]").click();
  const to = path.join(ORG, "롯데홈쇼핑", "로그");
  ok("D", "새 이름 '롯데홈쇼핑' → 새 고객사 폴더의 로그", await waitFor(() => fs.existsSync(to) && fs.readdirSync(to).length === 1, 10000), first);
  await waitFor(async () => (await pop.locator("[data-testid=pending-name]").count()) === 1 && (await pop.locator("[data-testid=pending-name]").innerText()) !== first, 10000);
  const second = await pop.locator("[data-testid=pending-name]").innerText();
  await pop.locator("[data-testid=customer-skip]").click();
  await sleep(1500);
  ok("D", "그대로 두기 → 바탕 화면에 그대로", fs.existsSync(path.join(DESK, second)), second);
  await pop.close();
}

// E. 960×640: the question card fits
{
  put("qq_sms_2026-07-04.log.zip");
  await main.setViewportSize({ width: 960, height: 640 });
  await tab("확인 대기").click();
  await waitFor(async () => (await main.locator("[data-testid=customer-ask]").count()) >= 1, 20000, 700);
  const noH = await main.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1);
  ok("E", "960×640: 가로 스크롤 없음", noH);
  await main.screenshot({ path: `${SHOT}/fix-sense-E-narrow.png` });
}

await browser.close();
const failed = results.filter((r) => !r.pass);
console.log(`\n${results.length - failed.length}/${results.length} passed`);
process.exit(failed.length ? 1 : 0);
