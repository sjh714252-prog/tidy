//! Self-contained HTML export of a [`Report`] (no scripts, no external
//! resources — safe to open anywhere or mail around).

use std::fmt::Write as _;

use humansize::{format_size, BINARY};

use super::tree::{render_tree, TreeOptions};
use super::Report;

/// Escape text for HTML element content and attribute values.
pub fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&#39;"),
            c => o.push(c),
        }
    }
    o
}

fn size(b: u64) -> String {
    format_size(b, BINARY)
}

fn date(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%Y-%m-%d")
                .to_string()
        })
        .unwrap_or_default()
}

/// Render the report. `tree_depth` limits the folder tree section.
pub fn render(r: &Report, tree_depth: Option<usize>) -> String {
    let s = &r.stats;
    let mut h = String::new();
    let title = format!("Tidy 리포트 — {}", r.root.display());
    let _ = write!(
        h,
        r#"<!doctype html><html lang="ko"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{t}</title><style>
:root{{color-scheme:light dark;font-family:"Segoe UI","Malgun Gothic","Apple SD Gothic Neo",system-ui,sans-serif;font-size:14px}}
body{{margin:24px;max-width:1100px}}h1{{font-size:20px}}h2{{font-size:16px;margin-top:28px}}
.cards{{display:flex;gap:12px;flex-wrap:wrap}}.card{{border:1px solid #8884;border-radius:8px;padding:10px 14px;min-width:120px}}
.card b{{display:block;font-size:20px}}table{{border-collapse:collapse;width:100%}}td,th{{border-bottom:1px solid #8883;padding:4px 8px;text-align:left;vertical-align:top}}
td.n{{text-align:right;white-space:nowrap}}pre{{font-family:Consolas,"D2Coding",monospace;font-size:12px;overflow-x:auto}}.muted{{color:#888}}
</style></head><body><h1>{t}</h1><p class="muted">생성: {gen}</p>
"#,
        t = esc(&title),
        gen = esc(&chrono::DateTime::from_timestamp(r.generated_at, 0)
            .map(|t| t
                .with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string())
            .unwrap_or_default())
    );
    let wasted: u64 = r.duplicates.iter().map(|d| d.wasted).sum();
    let _ = write!(
        h,
        r#"<div class="cards"><div class="card">파일<b>{}</b></div><div class="card">폴더<b>{}</b></div>
<div class="card">용량<b>{}</b></div><div class="card">맨 위 파일<b>{}</b></div>
<div class="card">{}일 넘게 안 쓴 파일<b>{}</b></div><div class="card">중복 낭비<b>{}</b></div></div>"#,
        s.total_files,
        s.total_dirs,
        size(s.total_bytes),
        s.root_level_files,
        r.stale_days,
        s.stale_files,
        size(wasted)
    );

    if let Some(p) = &r.pii {
        let _ = write!(
            h,
            "<p class=pii>🔒 {} — 검색 인덱스 기준</p>",
            esc(&p.label())
        );
    }
    h.push_str("<h2>종류별</h2><table><tr><th>확장자</th><th>파일</th><th>용량</th></tr>");
    for t in &s.by_type {
        let _ = write!(
            h,
            "<tr><td>{}</td><td class=n>{}</td><td class=n>{}</td></tr>",
            esc(&t.ext),
            t.count,
            size(t.bytes)
        );
    }
    h.push_str("</table><h2>큰 폴더</h2><table>");
    for d in &s.largest_dirs {
        let _ = write!(
            h,
            "<tr><td>{}</td><td class=n>{} 파일</td><td class=n>{}</td></tr>",
            esc(&d.path),
            d.files,
            size(d.bytes)
        );
    }
    h.push_str("</table><h2>큰 파일</h2><table>");
    for f in &s.largest_files {
        let _ = write!(
            h,
            "<tr><td>{}</td><td class=n>{}</td></tr>",
            esc(&f.path),
            size(f.bytes)
        );
    }
    h.push_str("</table>");

    let _ = write!(h, "<h2>중복 파일 ({}그룹)</h2>", r.duplicates.len());
    if r.duplicates.is_empty() {
        h.push_str("<p class=muted>없음</p>");
    } else {
        h.push_str("<table><tr><th>파일</th><th>크기</th><th>낭비</th></tr>");
        for g in &r.duplicates {
            let paths: Vec<String> = g.paths.iter().map(|p| esc(p)).collect();
            let _ = write!(
                h,
                "<tr><td>{}</td><td class=n>{}</td><td class=n>{}</td></tr>",
                paths.join("<br>"),
                size(g.size),
                size(g.wasted)
            );
        }
        h.push_str("</table>");
    }

    let _ = write!(h, "<h2>오래 안 쓴 파일 (상위 {})</h2>", s.stale.len());
    if s.stale.is_empty() {
        h.push_str("<p class=muted>없음</p>");
    } else {
        h.push_str("<table><tr><th>파일</th><th>마지막 사용</th><th>크기</th></tr>");
        for f in &s.stale {
            let _ = write!(
                h,
                "<tr><td>{}</td><td class=n>{}</td><td class=n>{}</td></tr>",
                esc(&f.path),
                date(f.last_used),
                size(f.bytes)
            );
        }
        h.push_str("</table>");
    }
    if !s.empty_dirs.is_empty() {
        let _ = write!(h, "<h2>빈 폴더 ({})</h2><ul>", s.empty_dirs.len());
        for d in &s.empty_dirs {
            let _ = write!(h, "<li>{}</li>", esc(d));
        }
        h.push_str("</ul>");
    }
    let tree = render_tree(
        &r.tree,
        &TreeOptions {
            max_depth: tree_depth,
            ..Default::default()
        },
    );
    let _ = write!(h, "<h2>폴더 트리</h2><pre>{}</pre>", esc(&tree));
    if !r.errors.is_empty() {
        let _ = write!(h, "<h2>읽지 못한 항목 ({})</h2><ul>", r.errors.len());
        for e in r.errors.iter().take(50) {
            let _ = write!(h, "<li>{}</li>", esc(e));
        }
        h.push_str("</ul>");
    }
    h.push_str("</body></html>\n");
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::{build, ReportOptions};

    #[test]
    fn html_escapes_names_and_has_sections() {
        let tmp = tempfile::tempdir().unwrap();
        // `<`/`>` cannot appear in Windows file names; `&` and `'` can.
        std::fs::write(tmp.path().join("원본's.txt"), vec![1u8; 2048]).unwrap();
        std::fs::write(tmp.path().join("사본 & 원본.txt"), vec![1u8; 2048]).unwrap();
        let r = build(tmp.path(), &ReportOptions::default(), None).unwrap();
        let h = render(&r, Some(2));
        assert_eq!(
            esc("<script>\"x\"</script>"),
            "&lt;script&gt;&quot;x&quot;&lt;/script&gt;"
        );
        assert!(h.contains("원본&#39;s.txt"));
        assert!(h.contains("사본 &amp; 원본.txt"));
        assert!(h.contains("중복 파일 (1그룹)"));
        assert!(h.contains("폴더 트리"));
    }
}
