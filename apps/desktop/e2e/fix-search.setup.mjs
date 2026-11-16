// Fixtures for fix-search.mjs: an onboarded config (바탕 화면 → 정리, collect)
// with one rule (pdf·txt → 문서), no NAS. Files arrive after the app starts.
import fs from "node:fs";
import path from "node:path";

const T = process.env.T;
const DESK = path.join(T, "바탕 화면");
const ORG = path.join(T, "정리");
fs.mkdirSync(DESK, { recursive: true });
fs.mkdirSync(ORG, { recursive: true });
fs.writeFileSync(
  path.join(T, "config.toml"),
  `[general]
onboarded = true
start_on_login = false
check_updates = false

[classify]
rules = ${JSON.stringify(path.join(T, "rules.toml"))}
stabilize_secs = 1

[[watch]]
path = ${JSON.stringify(DESK)}
kind = "local"
recursive = false
index_content = true
mode = "collect"
organize_root = ${JSON.stringify(ORG)}
`,
);
fs.writeFileSync(
  path.join(T, "rules.toml"),
  `[[rule]]
name = "문서"
match = { ext = ["pdf", "txt"] }
dest = "문서"
`,
);
