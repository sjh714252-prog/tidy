// Fixtures for fix-relocate.mjs, written before the app starts: an
// onboarded config (바탕 화면 → 정리) whose rules.toml still has the first
// example rules' absolute destinations, and files they already put there.
import fs from "node:fs";
import os from "node:os";
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
  `# 처음 버전 예시 규칙
[[rule]]
name = "문서"
match = { ext = ["pdf", "docx"] }
dest = "~/Documents/Tidy/문서/{yyyy}-{mm}"

[[rule]]
name = "압축 파일"
match = { ext = ["zip"] }
dest = "~/Downloads/압축 파일"

[[rule]]
name = "로그"
match = { ext = ["log"] }
dest = "로그"
`,
);
const HOME = os.homedir();
const TIDY = path.join(HOME, "Documents", "Tidy");
fs.rmSync(TIDY, { recursive: true, force: true });
fs.mkdirSync(path.join(TIDY, "문서", "2026-10"), { recursive: true });
fs.writeFileSync(path.join(TIDY, "문서", "2026-10", "견적서.pdf"), "예전 규칙이 옮긴 문서");
fs.writeFileSync(path.join(TIDY, "문서", "2026-10", "회의록.docx"), "예전 규칙이 옮긴 문서 2");
// Not Tidy's (no move_log) and not in Tidy's own folder: left alone.
const ZIPS = path.join(HOME, "Downloads", "압축 파일");
fs.mkdirSync(ZIPS, { recursive: true });
fs.writeFileSync(path.join(ZIPS, "내가 둔 파일.zip"), "user");
