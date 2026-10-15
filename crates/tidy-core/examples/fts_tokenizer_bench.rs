//! Korean FTS5 tokenizer comparison (SPEC §13). Output is recorded in
//! `docs/fts-tokenizer.md`.
//!
//! ```text
//! cargo run --release -p tidy-core --example fts_tokenizer_bench
//! ```
//! Builds three indexes over the same Korean sample documents —
//! `unicode61` (exact token), `unicode61` with prefix queries (`term*`),
//! and `trigram` — and prints which documents each query finds, then
//! index size and query latency on a synthetic 20k-document corpus.

use std::time::Instant;

use rusqlite::Connection;

struct Doc {
    name: &'static str,
    body: &'static str,
}

const DOCS: &[Doc] = &[
    Doc {
        name: "KT과금내역서_2026-09.xlsx",
        body: "9월 인터넷 회선 과금 33,000원 IPTV 12,000원",
    },
    Doc {
        name: "회의록_20261015.docx",
        body: "10월 정기 회의록. 참석자 홍길동 김영희. 과금내역서는 매월 5일까지 제출한다.",
    },
    Doc {
        name: "분기 보고서 초안.pdf",
        body: "3분기 매출이 12% 성장했다. 보고서를 임원진에게 공유함.",
    },
    Doc {
        name: "사업제안서(최종).docx",
        body: "KT 과금 시스템 개선 제안. 담당 홍길동 (기술팀)",
    },
    Doc {
        name: "Screenshot 2026-10-01.png",
        body: "",
    },
    Doc {
        name: "설치파일목록.txt",
        body: "setup.exe, installer.msi 다운로드 폴더에 보관",
    },
    Doc {
        name: "주차장안내문.html",
        body: "10월 20일부터 지하 2층 주차장을 폐쇄합니다. 문의: 총무팀",
    },
    Doc {
        name: "영수증 모음.zip",
        body: "",
    },
];

/// (query, what a user means by it)
const QUERIES: &[(&str, &str)] = &[
    ("과금", "2글자, 복합어 '과금내역서' 안에 있음"),
    ("과금내역", "복합어 일부"),
    ("내역서", "복합어 뒷부분"),
    ("보고서", "조사 붙음 '보고서를'"),
    ("회의록", "밑줄로 이어진 파일명"),
    ("KT", "2글자 영문, 복합어 앞 'KT과금내역서'"),
    ("제안서", "괄호 붙은 파일명 '사업제안서(최종)'"),
    ("홍길동", "띄어쓰기 없는 이름"),
    ("주차장", "'주차장안내문', '주차장을'"),
    ("screenshot", "영문 대소문자"),
    ("2026", "숫자"),
    ("exe", "확장자 조각 'setup.exe'"),
];

fn make(conn: &Connection, table: &str, tokenize: &str) {
    conn.execute_batch(&format!(
        "CREATE VIRTUAL TABLE {table} USING fts5(name, body, tokenize='{tokenize}');"
    ))
    .unwrap();
    for (i, d) in DOCS.iter().enumerate() {
        conn.execute(
            &format!("INSERT INTO {table}(rowid, name, body) VALUES (?1, ?2, ?3)"),
            rusqlite::params![i as i64 + 1, d.name, d.body],
        )
        .unwrap();
    }
}

fn query(conn: &Connection, table: &str, m: &str) -> Result<Vec<i64>, rusqlite::Error> {
    let mut stmt = conn.prepare(&format!(
        "SELECT rowid FROM {table} WHERE {table} MATCH ?1 ORDER BY rowid"
    ))?;
    let rows = stmt.query_map([m], |r| r.get(0))?;
    rows.collect()
}

fn fmt(ids: &Result<Vec<i64>, rusqlite::Error>) -> String {
    match ids {
        Ok(v) if v.is_empty() => "-".into(),
        Ok(v) => v
            .iter()
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join(","),
        Err(_) => "ERR".into(),
    }
}

fn quality() {
    let conn = Connection::open_in_memory().unwrap();
    make(&conn, "uni", "unicode61 remove_diacritics 2");
    make(&conn, "tri", "trigram");

    println!("## 샘플 문서\n");
    for (i, d) in DOCS.iter().enumerate() {
        println!(
            "{}. `{}` — {}",
            i + 1,
            d.name,
            if d.body.is_empty() {
                "(본문 없음)"
            } else {
                d.body
            }
        );
    }
    println!("\n## 질의별 매치 (문서 번호)\n");
    println!("| 질의 | 의도 | unicode61 | unicode61 + prefix(`*`) | trigram |");
    println!("|---|---|---|---|---|");
    for (q, why) in QUERIES {
        let quoted = format!("\"{q}\"");
        let uni = query(&conn, "uni", &quoted);
        let uni_pre = query(&conn, "uni", &format!("{quoted}*"));
        let tri = query(&conn, "tri", &quoted);
        println!(
            "| `{q}` | {why} | {} | {} | {} |",
            fmt(&uni),
            fmt(&uni_pre),
            fmt(&tri)
        );
    }
}

fn synth_body(i: usize) -> String {
    const WORDS: &[&str] = &[
        "과금",
        "내역서",
        "회의록",
        "보고서를",
        "제출한다",
        "홍길동",
        "김영희",
        "분기",
        "매출",
        "성장했다",
        "인터넷",
        "회선",
        "주차장",
        "폐쇄합니다",
        "총무팀",
        "다운로드",
        "설치파일",
        "KT",
        "IPTV",
        "2026년",
        "10월",
        "예산",
        "기술팀",
        "제안서",
        "개선",
        "시스템",
    ];
    let mut s = String::new();
    let mut x = i as u64 * 2654435761;
    for _ in 0..300 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        s.push_str(WORDS[(x % WORDS.len() as u64) as usize]);
        s.push(' ');
    }
    s
}

fn perf() {
    let dir = tempfile_dir();
    println!("\n## 성능 (합성 20,000문서, 문서당 ≈300단어 한국어)\n");
    println!("| 토크나이저 | DB 크기 | 삽입 시간 | `\"과금내역\"` | `\"보고서\"*` / `\"보고서\"` | `\"KT\"` |");
    println!("|---|---|---|---|---|---|");
    for (label, tokenize) in [
        ("unicode61", "unicode61 remove_diacritics 2"),
        ("trigram", "trigram"),
    ] {
        let path = dir.join(format!("{label}.db"));
        let _ = std::fs::remove_file(&path);
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&format!(
            "CREATE VIRTUAL TABLE t USING fts5(name, body, tokenize='{tokenize}');"
        ))
        .unwrap();
        let t0 = Instant::now();
        conn.execute_batch("BEGIN").unwrap();
        {
            let mut stmt = conn
                .prepare("INSERT INTO t(name, body) VALUES (?1, ?2)")
                .unwrap();
            for i in 0..20_000 {
                stmt.execute(rusqlite::params![format!("문서{i}.txt"), synth_body(i)])
                    .unwrap();
            }
        }
        conn.execute_batch("COMMIT").unwrap();
        let insert = t0.elapsed();
        let size = std::fs::metadata(&path).unwrap().len();
        let time = |m: &str| {
            let t = Instant::now();
            let n = query(&conn, "t", m).map(|v| v.len()).unwrap_or(0);
            format!("{} hits / {:.1} ms", n, t.elapsed().as_secs_f64() * 1e3)
        };
        let q2 = if label == "unicode61" {
            time("\"보고서\"*")
        } else {
            time("\"보고서\"")
        };
        println!(
            "| {label} | {:.1} MB | {:.2} s | {} | {} | {} |",
            size as f64 / 1e6,
            insert.as_secs_f64(),
            time("\"과금내역\""),
            q2,
            time("\"KT\"")
        );
        let _ = std::fs::remove_file(&path);
    }
}

fn tempfile_dir() -> std::path::PathBuf {
    let d = std::env::temp_dir().join("tidy-fts-bench");
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn main() {
    println!("# FTS5 토크나이저 비교 (생성: `cargo run --release -p tidy-core --example fts_tokenizer_bench`)\n");
    quality();
    perf();
}
