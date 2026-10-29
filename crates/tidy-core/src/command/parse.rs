//! Offline command parser (SPEC §14.14): Korean (and a little English)
//! date words, kind words, "X 폴더로" destinations, "X에서" sources and
//! sizes. Whatever is left (minus verbs, filler words and particles) are
//! the keywords. Always available — the AI parser falls back to this.

use std::sync::LazyLock;

use chrono::{Datelike, Duration, NaiveDate};
use regex::Regex;

use super::{CommandAction, CommandError, Intent};

/// Parse `text` relative to `today`.
pub fn parse_rules(text: &str, today: NaiveDate) -> Result<Intent, CommandError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(CommandError::Empty);
    }
    if DELETE.is_match(text) {
        return Err(CommandError::Unsupported(
            "지우기·휴지통 명령은 받지 않습니다 — 옮기기와 찾기만 할 수 있습니다".into(),
        ));
    }
    if RENAME.is_match(text) {
        return Err(CommandError::Unsupported(
            "이름 바꾸기는 아직 할 수 없습니다 — 옮기기와 찾기만 할 수 있습니다".into(),
        ));
    }
    let mut it = Intent::default();
    let mut rest = format!(" {text} ");

    // Destination first: it swallows "X 폴더로" before the source / kind
    // passes can misread X.
    for re in DEST.iter() {
        if let Some(c) = re.captures(&rest) {
            let m = c.get(0).expect("whole match");
            let name = clean_dest(c.get(1).map_or("", |g| g.as_str()));
            if !name.is_empty() {
                it.destination = Some(name);
                rest.replace_range(m.range(), " ");
                break;
            }
        }
    }
    if let Some(c) = SOURCE.captures(&rest) {
        let m = c.get(0).expect("whole match");
        let name = clean_dest(&c[1]);
        if !name.is_empty() && !is_stop(&name) {
            it.source = Some(name);
            rest.replace_range(m.range(), " ");
        }
    }
    rest = parse_sizes(&rest, &mut it);
    rest = parse_dates(&rest, today, &mut it);

    let mut generic_doc = false;
    for raw in rest.split_whitespace() {
        // A leading dot is an extension (".log"), so only trailing dots go.
        let word = raw
            .trim_matches(|c: char| ",!?;:()[]{}\"'“”‘’「」".contains(c))
            .trim_end_matches('.');
        if word.is_empty() {
            continue;
        }
        if let Some(ext) = word.strip_prefix('.') {
            let ext = ext.to_lowercase();
            if !ext.is_empty() && ext.chars().all(|c| c.is_ascii_alphanumeric()) {
                push_unique(&mut it.exts, &ext);
                continue;
            }
        }
        let stem = strip_particles(word);
        let lower = stem.to_lowercase();
        if MOVE_VERB.is_match(&lower) {
            it.action = CommandAction::Move;
            continue;
        }
        if FIND_VERB.is_match(&lower) {
            continue;
        }
        if lower == "문서" || lower == "문서파일" {
            generic_doc = true;
            continue;
        }
        if let Some(exts) = kind_exts(&lower) {
            for e in exts {
                push_unique(&mut it.exts, e);
            }
            continue;
        }
        if is_stop(&lower) {
            continue;
        }
        push_unique(&mut it.keywords, &stem);
    }
    if generic_doc && it.exts.is_empty() {
        for e in crate::search::type_exts("문서").unwrap_or(&[]) {
            push_unique(&mut it.exts, e);
        }
    }
    if it.destination.is_some() {
        it.action = CommandAction::Move;
    } else if it.action == CommandAction::Move {
        // "옮겨줘" without a place: keep it a move so the caller asks for one.
    } else {
        it.action = CommandAction::Find;
    }
    Ok(it)
}

fn push_unique(v: &mut Vec<String>, s: &str) {
    if !v.iter().any(|x| x.eq_ignore_ascii_case(s)) {
        v.push(s.to_string());
    }
}

static DELETE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(삭제|지워|지우|지울|휴지통|버려|\bdelete\b|\bremove\b|\btrash\b)").unwrap()
});
static RENAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(이름\s*(을|를)?\s*(바꿔|바꾸|변경)|\brename\b)").unwrap());
static MOVE_VERB: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(옮기|옮겨|옮길|이동|보내|보낼|넣어|넣을|넣기|모아|모을|모으|정리해|정리하|move)")
        .unwrap()
});
static FIND_VERB: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(찾아|찾기|찾을|찾는|검색|보여|알려|목록|리스트|find|show|list|search)").unwrap()
});

/// Destination patterns, most specific first. Group 1 = the place.
static DEST: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    // "…로" / "…으로" alone mean "to"; "…에" only with a putting verb
    // ("폴더에 있는" is a source). The verb word is swallowed too.
    let verb = r"(?:넣|옮|모아|모으|보관|이동|보내)\S*";
    let to = format!(r"(?:(?:으로|로|에다가|에다|안으로)(?:\s+{verb})?|(?:안에|에)\s*{verb})");
    [
        // "프로젝트" 폴더로 / '프로젝트'로
        format!(r#"["'“‘「]([^"'”’」]+)["'”’」]\s*(?:폴더)?\s*{to}"#),
        // D:\정리\프로젝트로, ~/work/a로
        format!(r"\s((?:[A-Za-z]:[\\/]|~[\\/]|/)\S*?)\s*{to}(?:\s|$)"),
        // 프로젝트 폴더로
        format!(r"\s(\S+?)\s*폴더\s*{to}"),
        // 프로젝트로 옮겨 / 프로젝트에 넣어
        format!(r"\s(\S+?)(?:으로|로)\s+{verb}"),
        format!(r"\s(\S+?)에\s+{verb}"),
        r"(?i)\s(?:to|into)\s+(\S+)".to_string(),
    ]
    .iter()
    .map(|p| Regex::new(p).unwrap())
    .collect()
});

static SOURCE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\s(\S+?)\s*(?:폴더)?\s*(?:에서|에 있는|에있는|안에 있는)\s").unwrap()
});

fn clean_dest(s: &str) -> String {
    s.trim()
        .trim_matches(|c: char| "\"'“”‘’「」".contains(c))
        .trim()
        .to_string()
}

// ── sizes ───────────────────────────────────────────────────────────────

static SIZE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(\d+(?:\.\d+)?)\s*(kb|mb|gb|k|m|g|킬로|메가|기가)\s*(?:바이트)?\s*(이상|넘는|넘은|넘게|초과|보다\s*큰|큰|이하|미만|보다\s*작은|작은|안\s*되는|까지)?",
    )
    .unwrap()
});

fn parse_sizes(text: &str, it: &mut Intent) -> String {
    let mut cut = Vec::new();
    for c in SIZE.captures_iter(text) {
        let m = c.get(0).expect("whole");
        // "10 kakao" is not 10 KB: the unit must end the word.
        let unit_end = c.get(2).expect("unit").end();
        if text[unit_end..]
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_alphabetic())
        {
            continue;
        }
        let n: f64 = c[1].parse().unwrap_or(0.0);
        let mult: f64 = match c[2].to_lowercase().as_str() {
            "kb" | "k" | "킬로" => 1024.0,
            "mb" | "m" | "메가" => 1024.0 * 1024.0,
            _ => 1024.0 * 1024.0 * 1024.0,
        };
        let bytes = (n * mult) as u64;
        let op = c.get(3).map_or("", |g| g.as_str());
        if ["이하", "미만", "작은", "안", "까지"]
            .iter()
            .any(|w| op.contains(w))
        {
            it.max_size = Some(bytes);
        } else {
            it.min_size = Some(bytes);
        }
        cut.push(m.range());
    }
    let mut out = text.to_string();
    for r in cut.into_iter().rev() {
        out.replace_range(r, " ");
    }
    out
}

// ── dates ───────────────────────────────────────────────────────────────

/// What follows a date phrase: "부터/이후" (from), "까지" (until, inclusive),
/// "이전/전에" (before).
#[derive(Clone, Copy, PartialEq)]
enum Bound {
    Range,
    From,
    Until,
    Before,
}

static BOUND: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?:(부터|이후|since|after)|(까지|until)|(이전|전에|before))").unwrap()
});

type DateFn = fn(&regex::Captures, NaiveDate) -> Option<(NaiveDate, NaiveDate)>;

static DATES: LazyLock<Vec<(Regex, DateFn)>> = LazyLock::new(|| {
    let v: Vec<(&str, DateFn)> = vec![
        (r"최근\s*(\d+)\s*(일|주|개월|달)", |c, t| {
            let n: i64 = c[1].parse().ok()?;
            let start = match &c[2] {
                "일" => t - Duration::days(n - 1),
                "주" => t - Duration::days(n * 7 - 1),
                _ => add_months(t, -(n as i32)),
            };
            Some((start, t + Duration::days(1)))
        }),
        (r"(\d+)\s*(일|주|개월|달)\s*전", |c, t| {
            let n: i64 = c[1].parse().ok()?;
            match &c[2] {
                "일" => {
                    let d = t - Duration::days(n);
                    Some((d, d + Duration::days(1)))
                }
                "주" => Some(week_of(t - Duration::days(n * 7))),
                _ => Some(month_of(add_months(t, -(n as i32)))),
            }
        }),
        (
            r"(\d{4})\s*년\s*(\d{1,2})\s*월\s*(\d{1,2})\s*일",
            |c, _| day(c[1].parse().ok()?, c[2].parse().ok()?, c[3].parse().ok()?),
        ),
        (r"(\d{4})\s*년\s*(\d{1,2})\s*월", |c, _| {
            Some(month_of(NaiveDate::from_ymd_opt(
                c[1].parse().ok()?,
                c[2].parse().ok()?,
                1,
            )?))
        }),
        (r"(\d{4})\s*년(?:도)?", |c, _| year_of(c[1].parse().ok()?)),
        (r"(\d{4})[-./](\d{1,2})[-./](\d{1,2})", |c, _| {
            day(c[1].parse().ok()?, c[2].parse().ok()?, c[3].parse().ok()?)
        }),
        (r"(\d{4})[-./](\d{1,2})", |c, _| {
            Some(month_of(NaiveDate::from_ymd_opt(
                c[1].parse().ok()?,
                c[2].parse().ok()?,
                1,
            )?))
        }),
        (r"(\d{1,2})\s*월\s*(\d{1,2})\s*일", |c, t| {
            let (m, d): (u32, u32) = (c[1].parse().ok()?, c[2].parse().ok()?);
            let mut date = NaiveDate::from_ymd_opt(t.year(), m, d)?;
            if date > t {
                date = NaiveDate::from_ymd_opt(t.year() - 1, m, d)?;
            }
            Some((date, date + Duration::days(1)))
        }),
        (r"(\d{1,2})\s*월", |c, t| {
            let m: u32 = c[1].parse().ok()?;
            let mut first = NaiveDate::from_ymd_opt(t.year(), m, 1)?;
            if first > t {
                first = NaiveDate::from_ymd_opt(t.year() - 1, m, 1)?;
            }
            Some(month_of(first))
        }),
        (r"(?i)오늘|today", |_, t| Some((t, t + Duration::days(1)))),
        (r"(?i)어제|yesterday", |_, t| {
            Some((t - Duration::days(1), t))
        }),
        (r"그저께|그제", |_, t| {
            Some((t - Duration::days(2), t - Duration::days(1)))
        }),
        (r"(?i)이번\s*주|금주|this\s+week", |_, t| {
            Some(week_of(t))
        }),
        (r"(?i)지난\s*주|저번\s*주|last\s+week", |_, t| {
            Some(week_of(t - Duration::days(7)))
        }),
        (r"(?i)이번\s*달|이달|금월|this\s+month", |_, t| {
            Some(month_of(t))
        }),
        (
            r"(?i)지난\s*달|저번\s*달|지난\s*월|last\s+month",
            |_, t| Some(month_of(add_months(t, -1))),
        ),
        (r"재작년", |_, t| year_of(t.year() - 2)),
        (r"(?i)올해|금년|this\s+year", |_, t| year_of(t.year())),
        (r"(?i)작년|지난\s*해|전년|last\s+year", |_, t| {
            year_of(t.year() - 1)
        }),
    ];
    v.into_iter()
        .map(|(p, f)| (Regex::new(p).unwrap(), f))
        .collect()
});

fn parse_dates(text: &str, today: NaiveDate, it: &mut Intent) -> String {
    let mut out = text.to_string();
    for (re, f) in DATES.iter() {
        while let Some(c) = re.captures(&out) {
            let m = c.get(0).expect("whole");
            let range = f(&c, today);
            let mut end = m.end();
            let mut bound = Bound::Range;
            if let Some(b) = BOUND.captures(&out[end..]) {
                bound = if b.get(1).is_some() {
                    Bound::From
                } else if b.get(2).is_some() {
                    Bound::Until
                } else {
                    Bound::Before
                };
                end += b.get(0).expect("whole").end();
            }
            if let Some((start, stop)) = range {
                match bound {
                    Bound::Range => {
                        it.after = Some(start);
                        it.before = Some(stop);
                    }
                    Bound::From => it.after = Some(start),
                    Bound::Until => it.before = Some(stop),
                    Bound::Before => it.before = Some(start),
                }
            }
            out.replace_range(m.start()..end, " ");
        }
    }
    out
}

fn day(y: i32, m: u32, d: u32) -> Option<(NaiveDate, NaiveDate)> {
    let date = NaiveDate::from_ymd_opt(y, m, d)?;
    Some((date, date + Duration::days(1)))
}

fn year_of(y: i32) -> Option<(NaiveDate, NaiveDate)> {
    Some((
        NaiveDate::from_ymd_opt(y, 1, 1)?,
        NaiveDate::from_ymd_opt(y + 1, 1, 1)?,
    ))
}

fn month_of(d: NaiveDate) -> (NaiveDate, NaiveDate) {
    let first = d.with_day(1).expect("day 1 exists");
    (first, add_months(first, 1))
}

/// Monday-based week containing `d`.
fn week_of(d: NaiveDate) -> (NaiveDate, NaiveDate) {
    let start = d - Duration::days(i64::from(d.weekday().num_days_from_monday()));
    (start, start + Duration::days(7))
}

fn add_months(d: NaiveDate, n: i32) -> NaiveDate {
    let total = d.year() * 12 + d.month0() as i32 + n;
    let (y, m) = (total.div_euclid(12), total.rem_euclid(12) as u32 + 1);
    let mut day = d.day();
    loop {
        if let Some(x) = NaiveDate::from_ymd_opt(y, m, day) {
            return x;
        }
        day -= 1;
    }
}

// ── words ───────────────────────────────────────────────────────────────

/// Extensions for a kind word ("엑셀", "pdf", "사진"…). `None` = not a kind.
pub(crate) fn kind_exts(w: &str) -> Option<&'static [&'static str]> {
    let w = w.trim_end_matches("파일").trim_end_matches("들");
    Some(match w {
        "pdf" | "피디에프" => &["pdf"],
        "엑셀" | "excel" | "xlsx" | "xls" | "스프레드시트" => {
            &["xlsx", "xls", "xlsm", "csv"]
        }
        "csv" => &["csv"],
        "워드" | "word" | "docx" | "doc" => &["docx", "doc"],
        "한글" | "한컴" | "hwp" | "hwpx" | "아래한글" => &["hwp", "hwpx"],
        "ppt" | "pptx" | "피피티" | "파워포인트" | "발표자료" | "프레젠테이션" | "슬라이드" => {
            &["pptx", "ppt"]
        }
        "사진" | "이미지" | "그림" | "스크린샷" | "캡처" | "캡쳐" | "image" | "images"
        | "photo" | "photos" | "jpg" | "jpeg" | "png" => crate::search::type_exts("image")?,
        "동영상" | "영상" | "비디오" | "video" | "videos" | "mp4" => {
            crate::search::type_exts("video")?
        }
        "음악" | "음원" | "오디오" | "노래" | "audio" | "mp3" => {
            crate::search::type_exts("audio")?
        }
        "압축" | "zip" | "압축본" => crate::search::type_exts("archive")?,
        "텍스트" | "txt" | "메모" => &["txt", "md"],
        "코드" | "소스" | "소스코드" => crate::search::type_exts("code")?,
        "documents" | "docs" => crate::search::type_exts("doc")?,
        _ => return None,
    })
}

const STOP: &[&str] = &[
    "관련",
    "관련된",
    "관한",
    "대한",
    "전부",
    "모두",
    "다",
    "모든",
    "전체",
    "파일",
    "파일들",
    "것",
    "거",
    "것들",
    "들",
    "받은",
    "다운받은",
    "다운로드한",
    "내려받은",
    "만든",
    "작성한",
    "저장한",
    "수정한",
    "생긴",
    "들어온",
    "있는",
    "좀",
    "줘",
    "주세요",
    "줄래",
    "해줘",
    "해",
    "해주세요",
    "그리고",
    "및",
    "중",
    "중에",
    "안",
    "속",
    "에서",
    "폴더",
    "자료",
    "the",
    "all",
    "every",
    "file",
    "files",
    "my",
    "please",
    "and",
    "of",
    "from",
    "in",
    "with",
    "any",
    "some",
];

fn is_stop(w: &str) -> bool {
    let w = w.to_lowercase();
    STOP.contains(&w.as_str())
}

/// Particles and plural markers, longest first.
const PARTICLES: &[&str] = &[
    "에서부터",
    "들에서",
    "으로는",
    "에서는",
    "들을",
    "들이",
    "들은",
    "들의",
    "들도",
    "에서",
    "으로",
    "에게",
    "까지",
    "부터",
    "이랑",
    "하고",
    "처럼",
    "이나",
    "을",
    "를",
    "이",
    "가",
    "은",
    "는",
    "의",
    "와",
    "과",
    "도",
    "만",
    "로",
    "에",
    "들",
    "랑",
    "나",
];

/// "견적서를" → "견적서", "보고서들의" → "보고서". Keeps at least two
/// characters (a 1-char stem only via prefix search would match too much).
pub(crate) fn strip_particles(w: &str) -> String {
    let mut s = w.to_string();
    for _ in 0..2 {
        let mut changed = false;
        for p in PARTICLES {
            if let Some(stem) = s.strip_suffix(p) {
                let is_hangul_end = stem.chars().last().is_some_and(is_hangul_or_alnum);
                if stem.chars().count() >= 2 && is_hangul_end {
                    s = stem.to_string();
                    changed = true;
                    break;
                }
            }
        }
        if !changed {
            break;
        }
    }
    s
}

fn is_hangul_or_alnum(c: char) -> bool {
    c.is_alphanumeric()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, dd: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, dd).unwrap()
    }

    fn today() -> NaiveDate {
        d(2026, 10, 29) // Thursday
    }

    #[test]
    fn spec_example() {
        let it = parse_rules("지난달 받은 KT 관련 엑셀 전부 프로젝트 폴더로", today()).unwrap();
        assert_eq!(it.action, CommandAction::Move);
        assert_eq!(it.after, Some(d(2026, 9, 1)));
        assert_eq!(it.before, Some(d(2026, 10, 1)));
        assert_eq!(it.keywords, vec!["KT"]);
        assert!(it.exts.contains(&"xlsx".to_string()));
        assert_eq!(it.destination.as_deref(), Some("프로젝트"));
        assert_eq!(it.source, None);
    }

    #[test]
    fn move_with_particles_and_source() {
        let it = parse_rules("바탕화면에 있는 견적서 PDF를 고객사로 옮겨줘", today()).unwrap();
        assert_eq!(it.action, CommandAction::Move);
        assert_eq!(it.source.as_deref(), Some("바탕화면"));
        assert_eq!(it.destination.as_deref(), Some("고객사"));
        assert_eq!(it.keywords, vec!["견적서"]);
        assert_eq!(it.exts, vec!["pdf"]);
    }

    #[test]
    fn find_only() {
        let it = parse_rules("작년 계약서 찾아줘", today()).unwrap();
        assert_eq!(it.action, CommandAction::Find);
        assert_eq!(it.after, Some(d(2025, 1, 1)));
        assert_eq!(it.before, Some(d(2026, 1, 1)));
        assert_eq!(it.keywords, vec!["계약서"]);
        assert_eq!(it.destination, None);
    }

    #[test]
    fn quoted_and_absolute_destinations() {
        let it = parse_rules("사진을 \"여행 2026\" 폴더로 옮겨", today()).unwrap();
        assert_eq!(it.destination.as_deref(), Some("여행 2026"));
        assert!(it.exts.contains(&"jpg".to_string()));
        let it = parse_rules(r"회의록 D:\정리\회의로 옮겨줘", today()).unwrap();
        assert_eq!(it.destination.as_deref(), Some(r"D:\정리\회의"));
        assert_eq!(it.keywords, vec!["회의록"]);
        let it = parse_rules("보고서를 아카이브에 넣어줘", today()).unwrap();
        assert_eq!(it.destination.as_deref(), Some("아카이브"));
        assert_eq!(it.keywords, vec!["보고서"]);
    }

    #[test]
    fn date_words() {
        let r = |s: &str| {
            let it = parse_rules(s, today()).unwrap();
            (it.after, it.before)
        };
        assert_eq!(
            r("오늘 받은 파일"),
            (Some(d(2026, 10, 29)), Some(d(2026, 10, 30)))
        );
        assert_eq!(
            r("어제 사진"),
            (Some(d(2026, 10, 28)), Some(d(2026, 10, 29)))
        );
        assert_eq!(
            r("이번 주 문서"),
            (Some(d(2026, 10, 26)), Some(d(2026, 11, 2)))
        );
        assert_eq!(
            r("지난주 문서"),
            (Some(d(2026, 10, 19)), Some(d(2026, 10, 26)))
        );
        assert_eq!(
            r("최근 7일 pdf"),
            (Some(d(2026, 10, 23)), Some(d(2026, 10, 30)))
        );
        assert_eq!(
            r("3일 전 pdf"),
            (Some(d(2026, 10, 26)), Some(d(2026, 10, 27)))
        );
        assert_eq!(
            r("2024년 3월 견적"),
            (Some(d(2024, 3, 1)), Some(d(2024, 4, 1)))
        );
        assert_eq!(r("2024년 견적"), (Some(d(2024, 1, 1)), Some(d(2025, 1, 1))));
        assert_eq!(r("3월 견적"), (Some(d(2026, 3, 1)), Some(d(2026, 4, 1))));
        // A month still ahead this year means last year's.
        assert_eq!(r("12월 견적"), (Some(d(2025, 12, 1)), Some(d(2026, 1, 1))));
        assert_eq!(
            r("2026-09-05 견적"),
            (Some(d(2026, 9, 5)), Some(d(2026, 9, 6)))
        );
        assert_eq!(
            r("3월부터 5월까지 견적"),
            (Some(d(2026, 3, 1)), Some(d(2026, 6, 1)))
        );
        assert_eq!(r("2025년 이전 견적"), (None, Some(d(2025, 1, 1))));
        assert_eq!(r("올해 견적"), (Some(d(2026, 1, 1)), Some(d(2027, 1, 1))));
        let it = parse_rules("12월 31일 견적", d(2026, 3, 1)).unwrap();
        assert_eq!(it.after, Some(d(2025, 12, 31)));
        // Month-end arithmetic does not panic.
        let it = parse_rules("지난달 견적", d(2026, 3, 31)).unwrap();
        assert_eq!(it.after, Some(d(2026, 2, 1)));
        assert_eq!(it.before, Some(d(2026, 3, 1)));
    }

    #[test]
    fn sizes_and_exts() {
        let it = parse_rules("100MB 넘는 영상 찾아줘", today()).unwrap();
        assert_eq!(it.min_size, Some(100 * 1024 * 1024));
        assert!(it.exts.contains(&"mp4".to_string()));
        let it = parse_rules(".log 파일 1mb 이하", today()).unwrap();
        assert_eq!(it.max_size, Some(1024 * 1024));
        assert_eq!(it.exts, vec!["log"]);
        assert!(it.keywords.is_empty());
        let it = parse_rules("한글 문서 찾아", today()).unwrap();
        assert_eq!(it.exts, vec!["hwp", "hwpx"]);
        let it = parse_rules("문서 찾아", today()).unwrap();
        assert!(it.exts.contains(&"docx".to_string()));
    }

    #[test]
    fn refuses_delete_and_rename() {
        assert!(matches!(
            parse_rules("지난달 사진 다 지워줘", today()),
            Err(CommandError::Unsupported(_))
        ));
        assert!(matches!(
            parse_rules("휴지통으로 보내", today()),
            Err(CommandError::Unsupported(_))
        ));
        assert!(matches!(
            parse_rules("파일 이름을 바꿔줘", today()),
            Err(CommandError::Unsupported(_))
        ));
        assert!(matches!(
            parse_rules("  ", today()),
            Err(CommandError::Empty)
        ));
    }

    #[test]
    fn particles() {
        assert_eq!(strip_particles("견적서를"), "견적서");
        assert_eq!(strip_particles("보고서들의"), "보고서");
        assert_eq!(strip_particles("회의"), "회의");
        assert_eq!(strip_particles("KT"), "KT");
    }
}
