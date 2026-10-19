// M8 GUI test: first-run wizard → visual rule editor → organize mode per
// folder, driven through the dev bridge (SPEC §14.1).
//
//   A. first run: wizard, status bar "마법사를 마치면…", nothing written yet
//   B. wizard 1–3: two folders (desktop in_place, downloads collect into
//      another folder), starter rules with "지금 N개 해당", AI skipped
//   C. before "감시 시작": a new file is NOT moved, no config/rules written
//   D. finish: config (onboarded, mode, organize_root) + only the ticked
//      rules; new files move — in_place inside the folder, collect under
//      organize_root; files that were there before stay
//   E. "기존 파일 미리보기" → report tab rules plan (nothing applied)
//   F. rule cards: add card, extension chip, folder tree pick + "새 폴더",
//      reorder (drag + ↑), TOML synced both ways (header comments kept),
//      live match count, invalid TOML → cards read-only, save → new rule moves
//      (the card's "개인정보면 암호화" option is M9's, see e2e/m9.mjs)
//   G. "마법사 다시 실행" reopens the wizard with the current folders/rules
//
// Run: S=/tmp/tidy-e2e bash e2e/run-headless.sh m8
import { chromium } from "playwright";
import fs from "node:fs";
import path from "node:path";

const T = process.env.T;
const DESK = path.join(T, "바탕 화면");
const DOWN = path.join(T, "다운로드");
const ORG = path.join(T, "정리 보관함");
const CFG = path.join(T, "config.toml");
const RULES = path.join(T, "rules.toml");
const SHOT = path.join(process.env.S, "shots");
fs.mkdirSync(SHOT, { recursive: true });
fs.mkdirSync(DOWN, { recursive: true });
const ym = (() => {
  const d = new Date();
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}`;
})();

const results = [];
const ok = (step, what, pass, note = "") => {
  results.push({ step, what, pass, note });
  console.log(`${pass ? "PASS" : "FAIL"} [${step}] ${what} ${note}`);
};
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const bridge = async (cmd, args = {}) =>
  (await (await fetch(`http://127.0.0.1:1421/invoke/${cmd}`, { method: "POST", body: JSON.stringify(args) })).json()).value;
const waitFor = async (fn, ms, step = 500) => {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    if (await fn()) return true;
    await sleep(step);
  }
  return false;
};
const tab = (page, name) => page.getByRole("button", { name, exact: true }).click();

// Files that are already there before setup.
fs.writeFileSync(path.join(DESK, "보고서 A.pdf"), "old report");
fs.writeFileSync(path.join(DESK, "스크린샷 2026-10-01.png"), "png");
fs.writeFileSync(path.join(DOWN, "옛 압축.zip"), "zip");

const browser = await chromium.launch();
const ctx = await browser.newContext({ viewport: { width: 1100, height: 900 } });
const answers = [];
const onDialog = (d) => d.accept(answers.shift() ?? "");
const main = await ctx.newPage();
main.on("dialog", onDialog);
await main.goto("http://localhost:1420/#/main");
await main.waitForSelector(".topbar");
const wiz = await ctx.newPage();
wiz.on("dialog", onDialog);
await wiz.goto("http://localhost:1420/#/wizard");
await wiz.waitForSelector("[data-testid=wizard]");
await sleep(1000);

// A. first run
{
  const st = await bridge("get_status");
  ok("A", "첫 실행: onboarded = false", st.onboarded === false);
  const bar = await main.locator(".statusbar").innerText();
  ok("A", "상태바: 마법사를 마치면 감시 시작", bar.includes("마법사를 마치면"), bar.replace(/\n/g, " | "));
  ok("A", "설정·규칙 파일 아직 없음", !fs.existsSync(CFG) && !fs.existsSync(RULES));
  ok("A", "마법사 1단계 표시", await wiz.locator("[data-testid=step-folders]").isVisible());
  await wiz.screenshot({ path: `${SHOT}/m8-A-wizard.png` });
}

// B. wizard steps 1–3
{
  // Uncheck the suggested folders (the container's ~/Desktop etc.).
  const rows = wiz.locator("[data-testid=wizard-folder]");
  for (let i = 0; i < (await rows.count()); i++) {
    const cb = rows.nth(i).locator("input[type=checkbox]");
    if (await cb.isChecked()) await cb.uncheck();
  }
  answers.push(DESK);
  await wiz.getByRole("button", { name: "다른 폴더…" }).click();
  await sleep(500);
  answers.push(DOWN);
  await wiz.getByRole("button", { name: "다른 폴더…" }).click();
  await sleep(500);
  const desk = wiz.locator("[data-testid=wizard-folder]", { hasText: DESK });
  const down = wiz.locator("[data-testid=wizard-folder]", { hasText: DOWN });
  ok("B", "다른 폴더… 로 두 폴더 추가", (await desk.count()) === 1 && (await down.count()) === 1);
  await down.locator("select").selectOption("collect");
  const next = wiz.getByRole("button", { name: "다음" });
  ok("B", "모으기인데 기준 폴더 없으면 다음 불가", await next.isDisabled());
  await down.locator("input.organize-root").fill(ORG);
  await sleep(300);
  await next.click();
  await sleep(1500);
  ok("B", "2단계: 시작 규칙", await wiz.locator("[data-testid=step-rules]").isVisible());
  const starter = (name) =>
    wiz.locator("[data-testid=starter]").filter({ has: wiz.locator("b", { hasText: new RegExp(`^${name}$`) }) });
  const docCount = await starter("문서").locator("[data-testid=starter-count]").innerText();
  const zipCount = await starter("압축 파일").locator("[data-testid=starter-count]").innerText();
  const shotCount = await starter("스크린샷").locator("[data-testid=starter-count]").innerText();
  ok("B", "문서: 지금 1개 해당 (보고서 A.pdf)", docCount.includes("지금 1개") && docCount.includes("보고서 A.pdf"), docCount);
  ok("B", "압축: 지금 1개 해당", zipCount.includes("지금 1개"), zipCount);
  ok("B", "스크린샷: 지금 1개 해당", shotCount.includes("지금 1개"), shotCount);
  // Ticks: documents (default on), archives (on), screenshots off, installers off.
  await starter("압축 파일").locator("input[type=checkbox]").check();
  await starter("스크린샷").locator("input[type=checkbox]").uncheck();
  await starter("설치파일").locator("input[type=checkbox]").uncheck();
  await wiz.screenshot({ path: `${SHOT}/m8-B-starters.png` });
  await next.click();
  await sleep(800);
  ok("B", "3단계: AI (선택)", await wiz.locator("[data-testid=step-ai]").isVisible());
  await wiz.getByRole("button", { name: "건너뛰기" }).click();
  await sleep(500);
  const sum = await wiz.locator("[data-testid=step-summary]").innerText();
  ok(
    "B",
    "요약: 폴더별 목적지",
    sum.includes(`${DESK}/문서/{yyyy}-{mm}`) && sum.includes(`${ORG}/압축 파일`) && !sum.includes("스크린샷"),
    sum.replace(/\n/g, " | "),
  );
  await wiz.screenshot({ path: `${SHOT}/m8-B-summary.png` });
}

// C. nothing moves before "감시 시작"
{
  fs.writeFileSync(path.join(DESK, "마법사 중 문서.pdf"), "new during wizard");
  await sleep(6000);
  ok("C", "마법사 끝나기 전: 새 파일 그대로", fs.existsSync(path.join(DESK, "마법사 중 문서.pdf")));
  ok("C", "마법사 끝나기 전: 문서 폴더 없음", !fs.existsSync(path.join(DESK, "문서")));
  ok("C", "마법사 끝나기 전: config/rules 미작성", !fs.existsSync(CFG) && !fs.existsSync(RULES));
}

// D. finish → watching; in_place and collect moves
{
  await wiz.getByRole("button", { name: "감시 시작" }).click();
  const done = await waitFor(() => wiz.locator("[data-testid=step-done]").isVisible(), 15000);
  const doneText = done ? await wiz.locator("[data-testid=step-done]").innerText() : "";
  ok("D", "감시 시작 → 완료 화면 (기존 파일 수/해당 수)", done && /\d+개 중 \d+개/.test(doneText), doneText.replace(/\n/g, " | "));
  const cfg = fs.readFileSync(CFG, "utf8");
  ok("D", "config: onboarded = true", /onboarded *= *true/.test(cfg));
  ok("D", "config: collect + organize_root", /mode *= *"collect"/.test(cfg) && cfg.includes("정리 보관함"), cfg.split("\n").filter((l) => /mode|organize/.test(l)).join(" | "));
  const rules = fs.readFileSync(RULES, "utf8");
  ok(
    "D",
    "rules.toml: 체크한 규칙만 (문서·압축), 상대 경로",
    rules.includes('name = "문서"') && rules.includes('name = "압축 파일"') && !rules.includes("스크린샷") && !rules.includes("설치파일") && rules.includes('dest = "문서/{yyyy}-{mm}"'),
    rules.split("\n").filter((l) => l.startsWith("name") || l.startsWith("dest")).join(" | "),
  );
  await waitFor(async () => (await bridge("get_status")).watching.length === 2, 15000);
  const st = await bridge("get_status");
  ok("D", "감시 중: 두 폴더", st.onboarded && st.watching.length === 2, st.watching.join(" · "));
  await sleep(1500);
  fs.writeFileSync(path.join(DESK, "분기 보고서.pdf"), "pdf");
  fs.writeFileSync(path.join(DOWN, "사진 모음.zip"), "zip");
  const inPlace = path.join(DESK, "문서", ym, "분기 보고서.pdf");
  const collected = path.join(ORG, "압축 파일", "사진 모음.zip");
  ok("D", "in_place: 바탕 화면/문서/연-월 로 이동", await waitFor(() => fs.existsSync(inPlace), 20000), inPlace);
  ok("D", "collect: 정리 보관함/압축 파일 로 이동", await waitFor(() => fs.existsSync(collected), 20000), collected);
  ok(
    "D",
    "기존 파일은 그대로 (보고서 A.pdf, 마법사 중 문서.pdf, 옛 압축.zip)",
    fs.existsSync(path.join(DESK, "보고서 A.pdf")) && fs.existsSync(path.join(DESK, "마법사 중 문서.pdf")) && fs.existsSync(path.join(DOWN, "옛 압축.zip")),
  );
  const bar = await main.locator(".statusbar").innerText();
  ok("D", "상태바: 감시 중", bar.includes("감시 중"), bar.replace(/\n/g, " | "));
}

// E. "기존 파일 미리보기" → report tab rules plan, nothing applied
{
  const btn = wiz.getByRole("button", { name: "기존 파일 미리보기" });
  ok("E", "미리보기 제안 버튼", (await btn.count()) === 1);
  await btn.click();
  const shown = await waitFor(async () => (await main.locator("body").innerText()).includes("보고서 A.pdf"), 15000);
  const txt = await main.locator(".content").innerText();
  ok("E", "리포트 탭 규칙 계획에 기존 파일", shown && txt.includes("마법사 중 문서.pdf"), txt.split("\n").slice(0, 6).join(" | "));
  ok("E", "미리보기만 (이동 없음)", fs.existsSync(path.join(DESK, "보고서 A.pdf")));
  await main.screenshot({ path: `${SHOT}/m8-E-preview.png`, fullPage: true });
}

// F. visual rule editor
{
  fs.writeFileSync(path.join(DESK, "로고.psd"), "psd"); // no rule yet → stays
  await sleep(11000); // past the editor's 10 s listing cache
  await tab(main, "설정");
  await sleep(1500);
  const cards = main.locator("[data-testid=rule-card]");
  ok("F", "규칙 카드 2개 (문서, 압축)", (await cards.count()) === 2, `${await cards.count()}`);
  const names = async () => main.locator("[data-testid=rule-card] input.rule-name").evaluateAll((els) => els.map((e) => e.value));
  await main.getByRole("button", { name: "+ 규칙 추가" }).click();
  await sleep(500);
  const card = cards.nth(2);
  await card.locator("input.rule-name").fill("디자인");
  const extIn = card.locator("input[placeholder='확장자 + Enter']");
  await extIn.fill("PSD");
  await extIn.press("Enter");
  await sleep(300);
  ok("F", "확장자 칩 추가 (소문자)", (await card.locator(".chip").innerText()).includes(".psd"));
  // Folder tree: watch folder → 문서 (made by the in-place move) → new folder.
  await card.getByRole("button", { name: "폴더 선택…" }).click();
  await sleep(1000);
  const tree = card.locator(".folder-tree");
  const watchRow = tree.locator(".tree-row", { hasText: "감시: 바탕 화면" }).first();
  await watchRow.locator("button.twisty").click();
  await sleep(1000);
  const docRow = tree.locator(`.tree-row[data-path="${path.join(DESK, "문서")}"]`);
  ok("F", "폴더 트리: 펼치면 하위 폴더(문서) 표시", (await docRow.count()) === 1);
  await docRow.locator("button.tree-name").click();
  await sleep(300);
  ok("F", "트리에서 고르면 상대 경로 dest", (await card.locator("input.dest").inputValue()) === "문서");
  // "새 폴더" under the selected node (문서).
  answers.push("디자인");
  await tree.getByRole("button", { name: "새 폴더" }).click();
  await sleep(1500);
  const madeDir = path.join(DESK, "문서", "디자인");
  const destNow = await card.locator("input.dest").inputValue();
  ok("F", "새 폴더 → mover로 생성 + dest 반영", fs.existsSync(madeDir) && destNow === "문서/디자인", `dest=${destNow}`);
  await card.getByRole("button", { name: "폴더 선택…" }).click(); // close tree
  await sleep(1500);
  const count = await card.locator("[data-testid=match-count]").innerText();
  ok("F", "실시간 매치 수: 지금 1개 · 예: 로고.psd", count.includes("지금 1개") && count.includes("로고.psd"), count);
  // Reorder by dragging the handle (all cards on screen: tall viewport),
  // then with the ↑/↓ buttons.
  await main.setViewportSize({ width: 1100, height: 2400 });
  // The settings page is long (M9 security section): start with the first
  // card at the top so both ends of the drag are on screen.
  await cards.nth(0).evaluate((e) => e.scrollIntoView({ block: "start" }));
  await sleep(300);
  await card.locator(".handle").dragTo(cards.nth(0).locator("header"));
  await sleep(1200);
  let order = await names();
  ok("F", "드래그로 순서 변경 → 디자인이 맨 위", order[0] === "디자인", order.join(","));
  await cards.nth(0).getByRole("button", { name: "↓" }).click();
  await sleep(600);
  await cards.nth(2).getByRole("button", { name: "↑" }).click();
  await sleep(1200);
  order = await names();
  ok("F", "↓/↑ 버튼으로 순서 변경", order.join(",") === "문서,압축 파일,디자인", order.join(","));
  await cards.nth(2).getByRole("button", { name: "↑" }).click();
  await sleep(1200);
  order = await names();
  ok("F", "↑ 버튼 → 문서, 디자인, 압축", order[1] === "디자인", order.join(","));
  await main.setViewportSize({ width: 1100, height: 900 });
  // TOML synced (advanced, collapsed by default).
  const adv = main.locator("details.advanced");
  ok("F", "고급(TOML)은 접힌 상태", !(await adv.evaluate((d) => d.open)));
  await adv.locator("summary").click();
  await sleep(300);
  const ta = main.locator("textarea.rules");
  let toml = await ta.inputValue();
  const iDoc = toml.indexOf('name = "문서"');
  const iDes = toml.indexOf('name = "디자인"');
  const iZip = toml.indexOf('name = "압축 파일"');
  ok("F", "카드 → TOML: 순서·내용 동기화", iDoc >= 0 && iDoc < iDes && iDes < iZip && toml.includes('dest = "문서/디자인"') && toml.includes('ext = ["psd"]'), `${iDoc}<${iDes}<${iZip}`);
  ok("F", "TOML 머리 주석 보존", toml.startsWith("# Tidy 규칙"));
  // TOML → cards
  await ta.fill(toml.replace('name = "디자인"', 'name = "디자인 파일"').replace("# Tidy 규칙", "# 내 메모\n# Tidy 규칙"));
  await sleep(1200);
  order = await names();
  ok("F", "TOML → 카드: 이름 변경 반영", order[1] === "디자인 파일", order.join(","));
  // Invalid TOML → read-only cards
  toml = await ta.inputValue();
  await ta.fill(toml.replace('dest = "문서/디자인"', 'dest = "문서/{year}"'));
  await sleep(1200);
  const err = await main.locator(".rules-section .error").innerText().catch(() => "");
  const disabled = await main.locator("[data-testid=rule-card] input.rule-name").first().isDisabled();
  ok("F", "TOML 오류 → 카드 읽기 전용 + 오류 표시", disabled && err.includes("year"), err);
  ok("F", "오류 중 저장 불가", await main.getByRole("button", { name: "저장", exact: true }).isDisabled());
  await ta.fill(toml);
  await sleep(1200);
  ok("F", "고치면 카드 다시 편집 가능", !(await main.locator("[data-testid=rule-card] input.rule-name").first().isDisabled()));
  // M8 left a disabled placeholder here; M9 made it the real option (e2e/m9.mjs).
  ok("F", "카드 옵션: 개인정보면 암호화 (M9, 꺼진 채 시작)", !(await main.locator("[data-testid=encrypt-toggle]").first().isChecked()));
  await main.screenshot({ path: `${SHOT}/m8-F-editor.png`, fullPage: true });
  await main.getByRole("button", { name: "저장", exact: true }).click();
  await sleep(3000);
  const saved = fs.readFileSync(RULES, "utf8");
  ok("F", "저장: rules.toml에 사용자 주석·새 규칙", saved.startsWith("# 내 메모") && saved.includes('name = "디자인 파일"'));
  ok("F", "저장 전후 기존 psd는 그대로", fs.existsSync(path.join(DESK, "로고.psd")));
  await sleep(1500);
  fs.writeFileSync(path.join(DESK, "아이콘.psd"), "psd2");
  ok("F", "새 규칙으로 이동 (바탕 화면/문서/디자인)", await waitFor(() => fs.existsSync(path.join(madeDir, "아이콘.psd")), 20000));
}

// G. rerun wizard
{
  await main.getByRole("button", { name: "마법사 다시 실행" }).click();
  await sleep(2000);
  const txt = await wiz.locator("[data-testid=wizard]").innerText();
  const deskRow = wiz.locator("[data-testid=wizard-folder]", { hasText: DESK });
  ok("G", "마법사 다시 실행: 현재 폴더가 체크된 채로", (await deskRow.locator("input[type=checkbox]").isChecked()) && txt.includes("1. 감시 폴더"));
  await wiz.getByRole("button", { name: "다음" }).click();
  await sleep(1000);
  const note = await wiz.locator("[data-testid=step-rules]").innerText();
  ok("G", "기존 규칙은 그대로 둔다는 안내", note.includes("이미 있는 규칙 3개"), note.split("\n")[1] ?? "");
  await wiz.getByRole("button", { name: "나중에" }).click();
  ok("G", "나중에: 아무것도 안 바뀜", fs.readFileSync(RULES, "utf8").includes('name = "디자인 파일"'));
}

fs.writeFileSync(path.join(process.env.S, "m8-results.json"), JSON.stringify(results, null, 2));
await browser.close();
const fails = results.filter((r) => !r.pass).length;
console.log(`\n${results.length - fails}/${results.length} passed`);
process.exit(fails ? 1 : 0);
