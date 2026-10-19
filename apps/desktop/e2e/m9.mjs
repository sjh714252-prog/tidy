// M9 GUI test: personal-information detection + encrypted vault, driven
// through the dev bridge (SPEC §14.2).
//
//   A. settings: watch folder, rule card "🔒 개인정보면 암호화" (TOML gets
//      encrypt = "pii"), 보안 section password (twice) — all saved by the one
//      settings save button; password only in the keychain
//   B. a personal file (15 rows 이름+휴대전화) waits in confirm mode: popup
//      badge "개인정보: 이름+휴대전화 15", "🔒 암호화해서 보관"; a plain
//      file under the same rule moves normally
//   C. approve → <name>.zip (AES, method 99) at the rule's destination,
//      original removed, the decrypted entry equals the original
//   D. search finds it by its text with a 🔒 badge
//   E. undo → original back where it was (same bytes), zip gone, not
//      locked again
//   F. auto + vault mode: a file with a bank account goes straight to the
//      vault folder encrypted, nothing queued
//   G. report: "개인정보 의심 파일 N개(미암호화 M개)"
//   H. the password appears in no config, log or database file
//
// Run: S=/tmp/tidy-e2e bash e2e/run-headless.sh m9
import { chromium } from "playwright";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

const T = process.env.T;
const S = process.env.S;
const DESK = path.join(T, "바탕 화면");
const DOCS = path.join(DESK, "문서");
const VAULT = path.join(T, "정리", "보안");
const CFG = path.join(T, "config.toml");
const RULES = path.join(T, "rules.toml");
const PW = "e2e-보관비밀번호-2026";
const SHOT = path.join(S, "shots");
fs.mkdirSync(SHOT, { recursive: true });

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
const openAdvanced = async (page) => {
  const d = page.locator("details.advanced");
  if (!(await d.evaluate((e) => e.open))) await d.locator("summary").click();
};
/** Compression method of the first local file header (99 = WinZip AES). */
const zipInfo = (p) => {
  const b = fs.readFileSync(p);
  const nameLen = b.readUInt16LE(26);
  return { sig: b.readUInt32LE(0), method: b.readUInt16LE(8), name: b.subarray(30, 30 + nameLen).toString("utf8"), bytes: b };
};

const NAMES = ["홍길동", "김영희", "이민준", "박서연", "최지우", "정하준", "강도윤", "조서윤", "윤지호", "장하은", "임시우", "한예준", "오수아", "서지안", "신유진"];
const CSV = "이름,부서,휴대전화\n" + NAMES.map((n, i) => `${n},영업팀,010-${2000 + i}-${5000 + i}`).join("\n") + "\n";
const PII_FILE = path.join(DESK, "직원 연락망.csv");
const PLAIN_FILE = path.join(DESK, "장보기.txt");
const ACCOUNT_FILE = path.join(DESK, "환불 계좌.txt");

const browser = await chromium.launch();
const ctx = await browser.newContext({ viewport: { width: 1100, height: 1000 } });
const main = await ctx.newPage();
main.on("dialog", (d) => d.accept(process.env.PROMPT_ANSWER ?? ""));
await main.goto("http://localhost:1420/#/main");
await main.waitForSelector(".topbar");
await sleep(1000);
const sec = main.locator("[data-testid=security-section]");

// A. settings: folder, rule with encrypt toggle, password; one save button
{
  await main.getByRole("button", { name: "설정", exact: true }).click();
  await sleep(800);
  process.env.PROMPT_ANSWER = DESK;
  await main.getByRole("button", { name: "폴더 추가…" }).click();
  await sleep(500);
  await openAdvanced(main);
  await main
    .locator("textarea.rules")
    .fill(`# 사용자 주석\n\n[[rule]]\nname = "문서"\nmatch = { ext = ["txt", "csv"] }\ndest = "문서"\n`);
  await sleep(1500);
  const card = main.locator("[data-testid=rule-card]").first();
  await card.locator("[data-testid=encrypt-toggle]").check();
  const synced = await waitFor(async () => (await main.locator("textarea.rules").inputValue()).includes('encrypt = "pii"'), 5000);
  ok("A", "카드 토글 → TOML encrypt = \"pii\"", synced, (await main.locator("textarea.rules").inputValue()).replace(/\n/g, " | "));

  ok("A", "보안 섹션 표시 (비밀번호 없음)", (await sec.locator("[data-testid=vault-key-state]").innerText()).includes("아직 없음"));
  ok("A", "보안 섹션에 따로 저장 버튼 없음 (#8)", (await sec.getByRole("button", { name: /저장/ }).count()) === 0);
  const pw = sec.locator("input[type=password]");
  await pw.nth(0).fill(PW);
  await pw.nth(1).fill(PW + "x");
  await sleep(300);
  const blocked = await main.getByRole("button", { name: "저장", exact: true }).isDisabled();
  ok("A", "비밀번호 불일치 → 저장 불가", blocked, await main.locator(".savebar").innerText());
  await pw.nth(1).fill(PW);
  await sleep(300);
  ok("A", "저장하지 않은 변경 표시", (await main.locator(".savebar").innerText()).includes("저장하지 않은 변경"));
  await main.getByRole("button", { name: "저장", exact: true }).click();
  await sleep(3000);
  const st = await bridge("get_security_settings");
  ok("A", "통합 저장 → 키체인에 비밀번호", st.hasPassword === true, `store=${st.keyStore}`);
  ok("A", "화면: 설정됨 + 저장소 표시", (await sec.locator("[data-testid=vault-key-state]").innerText()).includes("설정됨"));
  const cfg = fs.readFileSync(CFG, "utf8");
  ok("A", "config: [security] 기본값 (rules, confirm)", cfg.includes("[security]") && cfg.includes('approval = "confirm"') && cfg.includes('mode = "rules"'));
  ok("A", "rules.toml: encrypt = \"pii\" + 주석 보존", (() => {
    const r = fs.readFileSync(RULES, "utf8");
    return r.includes('encrypt = "pii"') && r.includes("# 사용자 주석");
  })());
  ok("A", "비밀번호 칸 비워짐", (await pw.nth(0).inputValue()) === "" && (await pw.nth(1).inputValue()) === "");
  const bar = await main.locator(".statusbar").innerText();
  ok("A", "감시 시작", bar.includes("바탕 화면"), bar.replace(/\n/g, " | "));
  await main.screenshot({ path: `${SHOT}/m9-A-settings.png`, fullPage: true });
}

// B. personal file → confirm popup with badge; plain file moves
{
  fs.writeFileSync(PII_FILE, CSV);
  fs.writeFileSync(PLAIN_FILE, "우유, 빵, 계란");
  const moved = await waitFor(() => fs.existsSync(path.join(DOCS, "장보기.txt")), 15000);
  ok("B", "개인정보 없는 파일은 그대로 이동", moved && !fs.existsSync(PLAIN_FILE));
  const queued = await waitFor(async () => (await bridge("list_pending")).some((p) => p.name === "직원 연락망.csv"), 15000);
  ok("B", "개인정보 파일 → 대기열 (승인 전 무이동)", queued && fs.existsSync(PII_FILE) && !fs.existsSync(path.join(DOCS, "직원 연락망.csv.zip")));
  const item = (await bridge("list_pending")).find((p) => p.name === "직원 연락망.csv");
  ok("B", "대기열 항목: encrypt + 건수", item?.encrypt === true && item?.pii === "이름+휴대전화 15", JSON.stringify(item?.pii));

  const c = await ctx.newPage();
  await c.goto("http://localhost:1420/#/confirm");
  await c.waitForSelector(".confirm .c-file");
  const badge = await c.locator("[data-testid=pii-badge]").innerText().catch(() => "");
  ok("B", "팝업 배지: 개인정보: 이름+휴대전화 15", badge === "개인정보: 이름+휴대전화 15", badge);
  ok("B", "팝업: 🔒 암호화해서 보관", await c.locator("[data-testid=encrypt-tag]").isVisible());
  ok("B", "팝업 파일명", (await c.locator(".confirm .c-file").innerText()).includes("직원 연락망.csv"));
  await c.screenshot({ path: `${SHOT}/m9-B-confirm.png` });

  await main.getByRole("button", { name: /^대기열/ }).first().click();
  await sleep(800);
  ok("B", "대기열 화면 배지", (await main.locator(".pending [data-testid=pii-badge]").first().innerText()).includes("이름+휴대전화 15"));

  // C. approve → zip
  await c.getByRole("button", { name: "암호화해서 이동" }).click();
  const zip = path.join(DOCS, "직원 연락망.csv.zip");
  const made = await waitFor(() => fs.existsSync(zip), 15000);
  ok("C", "승인 → 문서/직원 연락망.csv.zip", made);
  ok("C", "원본 제거 (검증 후)", !fs.existsSync(PII_FILE));
  const z = zipInfo(zip);
  ok("C", "zip: AES 암호화 항목 (method 99), 이름 = 원본", z.sig === 0x04034b50 && z.method === 99 && z.name === "직원 연락망.csv", `method=${z.method} name=${z.name}`);
  ok("C", "zip 안에 평문 없음", !z.bytes.includes(Buffer.from("홍길동")) && !z.bytes.includes(Buffer.from("010-2000")));
  const copy = await bridge("vault_copy", { path: zip });
  ok("C", "키체인 비밀번호로 풀면 원본과 같음", fs.readFileSync(copy, "utf8") === CSV, copy);
  ok("C", "임시 사본은 tidy-open 아래", copy.startsWith(path.join(os.tmpdir(), "tidy-open")));
  ok("C", "남은 임시 파일 없음", !fs.readdirSync(DOCS).some((n) => n.includes(".tidy-tmp.")));
  await c.close();

  // D. search with 🔒
  const s = await ctx.newPage();
  await s.goto("http://localhost:1420/#/search");
  await s.locator(".search-input").fill("영업팀");
  const found = await waitFor(async () => (await s.locator(".results li").count()) > 0 && (await s.locator(".results li").first().innerText()).includes(".csv.zip"), 10000);
  ok("D", "검색: 본문 단어로 zip 찾음", found, await s.locator(".results").innerText().catch(() => ""));
  ok("D", "검색: 🔒 배지", await s.locator(".results li [data-testid=lock-badge]").first().isVisible().catch(() => false));
  const note = await s.locator("[data-testid=lock-note]").innerText().catch(() => "");
  ok("D", "미리보기: 암호화 보관 + 개인정보 건수", note.includes("암호화 보관") && note.includes("이름+휴대전화 15"), note);
  await s.screenshot({ path: `${SHOT}/m9-D-search.png` });
  await s.close();
}

// E. undo → original back, not locked again
{
  await main.getByRole("button", { name: "활동", exact: true }).click();
  await main.getByRole("button", { name: "마지막 이동 되돌리기" }).click();
  const back = await waitFor(() => fs.existsSync(PII_FILE), 10000);
  ok("E", "되돌리기 → 원래 위치에 복원", back);
  ok("E", "내용 동일", back && fs.readFileSync(PII_FILE, "utf8") === CSV);
  ok("E", "zip 제거", !fs.existsSync(path.join(DOCS, "직원 연락망.csv.zip")));
  await sleep(5000);
  const pend = await bridge("list_pending");
  ok("E", "되돌린 파일은 다시 잠그지 않음", fs.existsSync(PII_FILE) && !pend.some((p) => p.name === "직원 연락망.csv"), `pending=${pend.length}`);
}

// F. auto + vault folder
{
  await main.getByRole("button", { name: "설정", exact: true }).click();
  await sleep(800);
  await sec.locator("input[name=sec-mode]").nth(1).check();
  await sec.getByPlaceholder("예: D:\\정리\\보안").fill(VAULT);
  await sec.locator("select").selectOption("auto");
  await sleep(300);
  await main.getByRole("button", { name: "저장", exact: true }).click();
  await sleep(3000);
  const cfg = fs.readFileSync(CFG, "utf8");
  ok("F", "config: vault + auto + vault_dir", cfg.includes('mode = "vault"') && cfg.includes('approval = "auto"') && cfg.includes("정리/보안"), cfg.split("[security]")[1]?.split("[security.pii]")[0]?.replace(/\n/g, " | "));
  fs.writeFileSync(ACCOUNT_FILE, "환불 안내\n입금 계좌: 123456-12-345678 (예금주 김영희)\n");
  const zip = path.join(VAULT, "환불 계좌.txt.zip");
  const made = await waitFor(() => fs.existsSync(zip), 15000);
  ok("F", "자동: 보안 폴더에 바로 암호화", made && !fs.existsSync(ACCOUNT_FILE));
  ok("F", "규칙(문서)보다 먼저", !fs.existsSync(path.join(DOCS, "환불 계좌.txt.zip")) && !fs.existsSync(path.join(DOCS, "환불 계좌.txt")));
  const pend = await bridge("list_pending");
  ok("F", "대기열에 안 쌓임", !pend.some((p) => p.name === "환불 계좌.txt"));
  ok("F", "zip 내용", made && fs.readFileSync(await bridge("vault_copy", { path: zip }), "utf8").includes("123456-12-345678"));
  ok("F", "되돌렸던 파일은 그대로", fs.existsSync(PII_FILE));
  const st = await bridge("get_security_settings");
  ok("F", "보안 섹션 집계 (보관 1, 의심 ≥ 2)", st.locked === 1 && st.suspected >= 2, `locked=${st.locked} suspected=${st.suspected} unencrypted=${st.unencrypted}`);
}

// G. report
{
  await main.getByRole("button", { name: "리포트", exact: true }).click();
  await sleep(800);
  await main.locator(".report .toolbar select").selectOption({ index: 0 });
  await main.getByRole("button", { name: "리포트 만들기" }).click();
  const shown = await waitFor(async () => (await main.locator("[data-testid=report-pii]").count()) > 0, 15000);
  const txt = shown ? await main.locator("[data-testid=report-pii]").innerText() : "";
  ok("G", "리포트: 개인정보 의심 파일 N개(미암호화 M개)", /개인정보 의심 파일 \d+개\(미암호화 \d+개\)/.test(txt), txt);
  ok("G", "되돌린 연락망은 미암호화로 셈", /미암호화 [1-9]/.test(txt));
  await main.screenshot({ path: `${SHOT}/m9-G-report.png`, fullPage: true });
}

// H. no password anywhere on disk
{
  const pwBytes = Buffer.from(PW);
  const files = [CFG, RULES, path.join(S, "app-m9.log")];
  const db = (await bridge("get_status")).dbPath;
  for (const f of [db, `${db}-wal`]) if (fs.existsSync(f)) files.push(f);
  const leaks = files.filter((f) => fs.existsSync(f) && fs.readFileSync(f).includes(pwBytes));
  ok("H", "비밀번호가 설정·규칙·로그·DB 어디에도 없음", leaks.length === 0, `checked=${files.length} leaks=${leaks.join(",")}`);
}

await browser.close();
const failed = results.filter((r) => !r.pass);
console.log(`\n${results.length - failed.length}/${results.length} passed`);
process.exit(failed.length ? 1 : 0);
