// Shared helpers for the headless GUI tests (M13 screens, SPEC §14.8 7).
export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

export const bridge = async (cmd, args = {}) => {
  const r = await (await fetch(`http://127.0.0.1:1421/invoke/${cmd}`, { method: "POST", body: JSON.stringify(args) })).json();
  if (r.error) throw new Error(`${cmd}: ${r.error}`);
  return r.value;
};

export const waitFor = async (fn, ms, step = 500) => {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    if (await fn()) return true;
    await sleep(step);
  }
  return false;
};

/** Top bar tab: 홈 / 확인 대기 / 정리하기 / 되돌리기 / 설정. */
export const tab = (page, name) => page.locator(".topbar nav").getByRole("button", { name, exact: name !== "확인 대기" });

/** Settings page by key: watch · rules · context · ai · security · advanced. */
export const gotoSettings = async (page, key = "watch") => {
  if (!(await page.locator(".settings-nav").count())) {
    await tab(page, "설정").click();
    await page.waitForSelector(".settings-nav");
  }
  await page.locator(`[data-testid=settings-nav-${key}]`).click();
  await page.waitForSelector(`[data-testid=settings-${key}]`);
  await sleep(300);
};

/** "+ 폴더 추가" (the dev bridge asks for the path with a prompt: PROMPT_ANSWER). */
export const addWatchFolder = async (page, dir) => {
  process.env.PROMPT_ANSWER = dir;
  await page.getByRole("button", { name: "+ 폴더 추가", exact: true }).click();
  await sleep(500);
};

export const watchCard = (page, i) => page.locator("[data-testid=watch-card]").nth(i);

/** Open watch folder card `i` for editing (accordion: one at a time). */
export const openWatchEdit = async (page, i) => {
  const c = watchCard(page, i);
  if ((await c.getAttribute("data-open")) !== "true") await c.getByRole("button", { name: "편집", exact: true }).click();
  await sleep(200);
  return watchCard(page, i);
};

export const ruleCard = (page, i) => page.locator("[data-testid=rule-card]").nth(i);

/** Open rule card `i` (accordion). */
export const openRule = async (page, i) => {
  const c = ruleCard(page, i);
  if ((await c.getAttribute("data-open")) !== "true") await c.getByRole("button", { name: "펼치기", exact: true }).click();
  await sleep(200);
  return ruleCard(page, i);
};

/** 설정 → 규칙 → "고급: rules.toml 직접 편집" opened; returns the textarea. */
export const openToml = async (page) => {
  await gotoSettings(page, "rules");
  const d = page.locator("details.toml-edit");
  if (!(await d.evaluate((e) => e.open))) await d.locator("summary").click();
  return page.locator("textarea.rules");
};

export const saveSettings = async (page, ms = 2500) => {
  await page.locator("[data-testid=savebar]").getByRole("button", { name: "저장", exact: true }).click();
  await sleep(ms);
};

/** Status bar text (on any tab but 설정, which has none). */
export const statusText = async (page) => {
  if (await page.locator(".settings-nav").count()) await tab(page, "홈").click();
  await sleep(300);
  return page.locator("[data-testid=statusbar]").innerText();
};

/** Click item `label` of the "⋯" menu inside `scope` (button aria-label `menu`). */
export const moreItem = async (scope, label, menu = "다른 선택") => {
  await scope.getByRole("button", { name: menu, exact: true }).first().click();
  await sleep(150);
  const page = typeof scope.page === "function" ? scope.page() : scope;
  await page.getByRole("menuitem", { name: label, exact: true }).click();
};
