// Fixtures for m19.mjs, written before the app starts: an onboarded config
// (바탕 화면 → 정리) and an organize root that grew messy over time.
import fs from "node:fs";
import path from "node:path";

const T = process.env.T;
const DESK = path.join(T, "바탕 화면");
const ORG = path.join(T, "정리");
fs.mkdirSync(DESK, { recursive: true });
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
  `[[rule]]
name = "설치 파일"
match = { ext = ["exe"] }
dest = "설치파일"
`,
);
const put = (rel, text = rel) => {
  const p = path.join(ORG, rel);
  fs.mkdirSync(path.dirname(p), { recursive: true });
  fs.writeFileSync(p, text);
};
// One customer in two spellings; the same issue three times (2·6 days apart).
put("KT스마트메시지/20261014_과금/a1.log");
put("KT스마트메시지/20261014_과금/a2.log");
put("KT스마트메시지/20261020_과금/b.log");
put("KT 스마트메시지/20261016_과금/c.txt");
put("KT 스마트메시지/20261101_장애/d.txt");
// A customer folder parked under 기타, next to a real topic.
put("가람정보/20261020_과금/l.log");
put("기타/가람정보/메모.txt");
put("기타/개발/예제.go");
// (0.16.0) A customer's PDF in the type rules' folder (rebuild → 가람정보/문서),
// and a very long name (the diff panes must stay inside the window).
put("문서/2026/가람정보_견적서.pdf", "pdf");
put(`문서/2026/${"아주 긴 이름의 분기별 정산 보고서 최종본 ".repeat(4).trim()}.pdf`, "pdf");
// Loose at the top; a code project (never touched).
put("설치.exe", "exe");
put("tidy/Cargo.toml", "[package]");
put("tidy/src/main.rs", "fn main() {}");
