// Fixtures for fix-sense.mjs: 바탕 화면 → 정리 (collect), customer folders
// already there, one rule (zip → 압축파일). Files arrive after the start.
import fs from "node:fs";
import path from "node:path";

const T = process.env.T;
const DESK = path.join(T, "바탕 화면");
const ORG = path.join(T, "정리");
for (const d of [DESK, path.join(ORG, "가람정보", "20261014_과금"), path.join(ORG, "현대홈쇼핑", "20261020_장애"), path.join(ORG, "신세계", "20261001_전환")])
  fs.mkdirSync(d, { recursive: true });
fs.writeFileSync(
  path.join(T, "config.toml"),
  `[general]
onboarded = true
start_on_login = false
check_updates = false

[classify]
rules = ${JSON.stringify(path.join(T, "rules.toml"))}
stabilize_secs = 1

[curate]
context = ${JSON.stringify(path.join(T, "context.toml"))}

[[watch]]
path = ${JSON.stringify(DESK)}
kind = "local"
recursive = false
index_content = true
mode = "collect"
organize_root = ${JSON.stringify(ORG)}
`,
);
fs.writeFileSync(path.join(T, "rules.toml"), `[[rule]]\nname = "압축 파일"\nmatch = { ext = ["zip"] }\ndest = "압축파일"\n`);
