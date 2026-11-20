//! (0.20.0, SPEC §14.30) 압축 파일 대상군 — archives whose names (or the
//! folder inside them) share a subject belong together: `현대홈쇼핑_13일.zip`,
//! `현대홈쇼핑_CAMP_13일.zip` and `홈쇼핑 13일 처리건.zip` are one company's
//! material; the three `특수한 유형의 부가통신사업자…` zips are one topic.
//! Offline, no AI. Used by "정리 위치 새로 짜기" for archives that no known
//! customer claims (they used to stay together in a flat `압축 파일/`), and
//! while watching to send a new archive to the group folder made before.
//!
//! - A group whose subject looks like a company (`…홈쇼핑`, `…시스템`,
//!   `…텔레콤`, `주식회사…`) becomes a customer: `{업체}/압축파일/` (and
//!   log bundles go to `{업체}/로그/{날짜} 로그`, §14.24).
//! - Any other subject is a topic: `기타/{주제}/` — only with at least
//!   `min_folder_items` archives (no folder of one or two files, §14.19).
//! - One archive whose first word is a company (`제니엘시스템.zip`) gets
//!   its company folder too (a customer folder is never "too small").

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::existing::{norm, sub_dirs};
use super::sense::{is_date_word, is_version, split_exts, words, ARCHIVES};

/// A customer's folder for archives (the kind name of `curate::kinds`).
pub const ARCHIVE_KIND: &str = "압축파일";

/// Words that say nothing about whose / which work a file is (normalized).
const STOP: &[&str] = &[
    "관련자료",
    "검색결과",
    "복사본",
    "스크린샷",
    "새폴더",
    "고객사",
    "처리건",
    "리스트",
    "보고서",
    "요청서",
    "견적서",
    "계약서",
    "회의록",
    "테스트",
    "프로젝트",
    "결과물",
    "관리용",
    "데이터",
    "자료",
    "관련",
    "목록",
    "전체",
    "고객",
    "최종",
    "요청",
    "결과",
    "파일",
    "샘플",
    "변경",
    "처리",
    "정리",
    "백업",
    "사본",
    "신청",
    "서류",
    "내부",
    "관리",
    "수정",
    "참고",
    "캡처",
    "캡쳐",
    "문서",
    "업무",
    "작업",
    "대상",
    "발송",
    "정상",
    "일부",
    "압축",
    "backup",
    "copy",
    "final",
    "data",
    "list",
    "files",
    "file",
    "sample",
    "test",
    "temp",
    "tmp",
    "export",
    "report",
    "tools",
    "tool",
    "download",
    "logs",
    "log",
    "new",
    "old",
];

/// File-type words left in a name (`리스트.xlsx.zip`).
const TYPE_WORDS: &[&str] = &[
    "xlsx", "xls", "csv", "pdf", "docx", "doc", "hwp", "hwpx", "pptx", "ppt", "txt", "zip", "json",
    "xml", "png", "jpg", "jpeg",
];

/// Endings of company names (normalized).
const COMPANY_ENDS: &[&str] = &[
    "홈쇼핑",
    "시스템",
    "시스템즈",
    "텔레콤",
    "정보통신",
    "네트웍스",
    "네트워크스",
    "테크",
    "테크놀로지",
    "솔루션",
    "솔루션즈",
    "소프트",
    "소프트웨어",
    "전자",
    "은행",
    "카드",
    "증권",
    "보험",
    "생명",
    "화재",
    "캐피탈",
    "건설",
    "제약",
    "커머스",
    "엔터테인먼트",
    "랩스",
    "그룹",
    "코리아",
    "corp",
    "inc",
    "ltd",
];

/// Whole names of well-known companies (normalized).
const COMPANY_NAMES: &[&str] = &[
    "케이티",
    "에스케이",
    "엘지",
    "삼성",
    "현대",
    "롯데",
    "신세계",
    "한화",
    "카카오",
    "네이버",
    "쿠팡",
    "농협",
    "신한",
];

fn hangul(c: char) -> bool {
    ('\u{AC00}'..='\u{D7A3}').contains(&c)
}

/// Enough left after the stop words to name a subject: three Hangul
/// letters, four Latin letters, or two of each.
pub fn meaningful(key: &str) -> bool {
    let mut r = key.to_string();
    for w in STOP {
        r = r.replace(w, " ");
    }
    // The longest run between stop words must carry the meaning.
    r.split(' ').any(|part| {
        let h = part.chars().filter(|c| hangul(*c)).count();
        let l = part.chars().filter(char::is_ascii_alphabetic).count();
        h >= 3 || l >= 4 || (h >= 2 && l >= 2)
    })
}

/// Whether `label` reads as a company's name.
pub fn company_like(label: &str) -> bool {
    let k = norm(label);
    let n = k.chars().count();
    k.starts_with("주식회사")
        || COMPANY_NAMES.contains(&k.as_str())
        || COMPANY_ENDS
            .iter()
            .any(|e| k.ends_with(e) && n > e.chars().count())
}

/// Whether `path` is an archive (`.zip`, `.7z`, …).
pub fn is_archive(path: &Path) -> bool {
    ARCHIVES.contains(&crate::paths::ext_lower(path).as_str())
}

/// The words of a file name that say what it is about: no extensions,
/// dates, numbers, versions or file-type words (`홈쇼핑 13일 처리건.zip`
/// → `["홈쇼핑", "처리건"]`).
pub fn subject_words(name: &str) -> Vec<String> {
    let (stem, _) = split_exts(name);
    words(&stem)
        .into_iter()
        .flat_map(|w| {
            // `케이티_리스트.xlsx` keeps `.xlsx` in the stem.
            w.split('.').map(str::to_string).collect::<Vec<_>>()
        })
        .filter(|w| {
            let digits = w.chars().filter(char::is_ascii_digit).count();
            let n = w.chars().count();
            !w.is_empty()
                && digits * 2 < n
                && !is_date_word(w)
                && !is_version(w)
                && !TYPE_WORDS.contains(&w.to_lowercase().as_str())
        })
        .collect()
}

/// The folder every entry of a zip sits in (`현대홈쇼핑_정산/…`), when there
/// is exactly one and it names something.
pub fn inner_subject(entries: &[String]) -> Option<String> {
    let mut top: Option<&str> = None;
    let mut any_file = false;
    for e in entries {
        let e = e.trim_start_matches(['/', '\\']);
        if e.starts_with("__MACOSX") {
            continue;
        }
        let (first, rest) = e.split_once(['/', '\\'])?;
        if !rest.is_empty() && !rest.ends_with('/') {
            any_file = true;
        }
        match top {
            Some(t) if t != first => return None,
            _ => top = Some(first),
        }
    }
    let t = top.filter(|_| any_file)?;
    let ws = subject_words(&format!("{t}.zip"));
    let s = ws.join(" ");
    meaningful(&norm(&s)).then_some(s)
}

/// The company a lone archive's first word names (`제니엘시스템.zip`).
pub fn company_in(name: &str) -> Option<String> {
    let first = subject_words(name).into_iter().next()?;
    (company_like(&first) && meaningful(&norm(&first))).then(|| super::clean(&first))
}

/// One archive to group: its name and (optional) the folder inside it.
#[derive(Debug, Clone)]
pub struct Item {
    pub name: String,
    pub inner: Option<String>,
}

impl Item {
    /// (normalized key, original words joined by spaces).
    fn keys(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let s = subject_words(&self.name).join(" ");
        if !s.is_empty() {
            out.push((norm(&s), s));
        }
        if let Some(i) = &self.inner {
            if norm(i) != out.first().map(|k| k.0.clone()).unwrap_or_default() {
                out.push((norm(i), i.clone()));
            }
        }
        out
    }
}

/// A group of archives about one subject.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    /// Folder name (the spelling most members use).
    pub label: String,
    /// Indexes into the items given to [`groups`].
    pub members: Vec<usize>,
    /// The subject reads as a company (→ a customer folder).
    pub company: bool,
}

/// Longest common substring of two strings (by characters).
fn lcs(a: &str, b: &str) -> String {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev = vec![0usize; b.len() + 1];
    let mut cur = vec![0usize; b.len() + 1];
    let (mut best, mut end) = (0usize, 0usize);
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            cur[j] = if a[i - 1] == b[j - 1] {
                prev[j - 1] + 1
            } else {
                0
            };
            if cur[j] > best {
                best = cur[j];
                end = i;
            }
        }
        std::mem::swap(&mut prev, &mut cur);
        cur.iter_mut().for_each(|x| *x = 0);
    }
    a[end - best..end].iter().collect()
}

/// The slice of `orig` whose letters are `key` (`"특수한 유형의 부가통신사업자 관련"`,
/// `"특수한유형의부가통신사업자"` → `"특수한 유형의 부가통신사업자"`).
fn spelling(orig: &str, key: &str) -> Option<String> {
    let chars: Vec<(usize, char)> = orig.char_indices().collect();
    let want: Vec<char> = key.chars().collect();
    for s in 0..chars.len() {
        if !chars[s].1.is_alphanumeric() {
            continue;
        }
        let mut got = 0usize;
        for (e, (_, c)) in chars.iter().enumerate().skip(s) {
            if !c.is_alphanumeric() {
                continue;
            }
            let mut ok = true;
            for l in c.to_lowercase() {
                if want.get(got) == Some(&l) {
                    got += 1;
                } else {
                    ok = false;
                    break;
                }
            }
            if !ok {
                break;
            }
            if got == want.len() {
                let end = chars.get(e + 1).map_or(orig.len(), |(i, _)| *i);
                return Some(orig[chars[s].0..end].trim().to_string());
            }
        }
    }
    None
}

/// Most archives looked at in one grouping (the pairs grow fast).
const MAX_ITEMS: usize = 5_000;
/// A trigram shared by more names than this is too common to pair on.
const MAX_BUCKET: usize = 300;

/// Archives that belong together by name. `min_topic`: a group that is not
/// a company needs this many members (company groups: two).
pub fn groups(items: &[Item], min_topic: usize) -> Vec<Group> {
    let n = items.len().min(MAX_ITEMS);
    let keys: Vec<Vec<(String, String)>> = items[..n].iter().map(Item::keys).collect();
    // Candidate pairs: names sharing a three-letter run.
    let mut buckets: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, ks) in keys.iter().enumerate() {
        let mut seen: Vec<String> = Vec::new();
        for (k, _) in ks {
            let c: Vec<char> = k.chars().collect();
            for w in c.windows(3) {
                let t: String = w.iter().collect();
                if !seen.contains(&t) {
                    seen.push(t.clone());
                    buckets.entry(t).or_default().push(i);
                }
            }
        }
    }
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(p: &mut [usize], x: usize) -> usize {
        let mut r = x;
        while p[r] != r {
            r = p[r];
        }
        let mut y = x;
        while p[y] != r {
            let nx = p[y];
            p[y] = r;
            y = nx;
        }
        r
    }
    let mut tried: std::collections::HashSet<(usize, usize)> = Default::default();
    let mut edges: Vec<(usize, usize, String)> = Vec::new();
    let mut bucket_list: Vec<&Vec<usize>> = buckets.values().collect();
    bucket_list.sort();
    for b in bucket_list {
        if b.len() < 2 || b.len() > MAX_BUCKET {
            continue;
        }
        for x in 0..b.len() {
            for y in x + 1..b.len() {
                let (i, j) = (b[x], b[y]);
                if !tried.insert((i, j)) {
                    continue;
                }
                let mut best = String::new();
                for (ki, _) in &keys[i] {
                    for (kj, _) in &keys[j] {
                        let s = lcs(ki, kj);
                        if s.chars().count() > best.chars().count() {
                            best = s;
                        }
                    }
                }
                if meaningful(&best) {
                    let (ri, rj) = (find(&mut parent, i), find(&mut parent, j));
                    if ri != rj {
                        parent[ri] = rj;
                    }
                    edges.push((i, j, best));
                }
            }
        }
    }
    let mut comps: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..n {
        let r = find(&mut parent, i);
        comps.entry(r).or_default().push(i);
    }
    let mut out: Vec<Group> = Vec::new();
    let mut comp_list: Vec<Vec<usize>> = comps.into_values().filter(|c| c.len() >= 2).collect();
    comp_list.sort();
    for comp in comp_list {
        let has = |i: usize, s: &str| keys[i].iter().any(|(k, _)| k.contains(s));
        // The subject: the shared run that covers most of the group
        // (members × length), longer on a tie.
        let mut cands: Vec<String> = edges
            .iter()
            .filter(|(i, _, _)| comp.contains(i))
            .map(|(_, _, s)| s.clone())
            .collect();
        cands.sort();
        cands.dedup();
        let Some(label) = cands
            .into_iter()
            .map(|s| {
                let cnt = comp.iter().filter(|&&i| has(i, &s)).count();
                (cnt * s.chars().count(), s.chars().count(), s)
            })
            .filter(|(score, len, _)| *score >= 2 * *len)
            .max_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(b.2.cmp(&a.2)))
            .map(|(_, _, s)| s)
        else {
            continue;
        };
        // Members: those that show the subject, or share a meaningful part
        // of it (`홈쇼핑 처리건` in the `현대홈쇼핑` group).
        let members: Vec<usize> = comp
            .iter()
            .copied()
            .filter(|&i| has(i, &label) || keys[i].iter().any(|(k, _)| meaningful(&lcs(k, &label))))
            .collect();
        if members.len() < 2 {
            continue;
        }
        // The spelling most members use.
        let mut spells: Vec<(usize, String)> = Vec::new();
        for &i in &members {
            for (_, orig) in &keys[i] {
                if let Some(s) = spelling(orig, &label) {
                    match spells.iter_mut().find(|(_, x)| *x == s) {
                        Some(e) => e.0 += 1,
                        None => spells.push((1, s)),
                    }
                    break;
                }
            }
        }
        spells.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        let label = super::clean(&spells.first().map(|s| s.1.clone()).unwrap_or(label));
        if label.is_empty() {
            continue;
        }
        let company = company_like(&label);
        if !company && members.len() < min_topic.max(2) {
            continue;
        }
        out.push(Group {
            label,
            members,
            company,
        });
    }
    out
}

/// (While watching, and in "새로 짜기") The topic folder already made under
/// `{anchor}/{other}` whose name the archive's name shows (four letters or
/// more, longest wins): `기타/특수한유형의부가통신사업자` for
/// `특수한유형의부가통신사업자_2차.zip`.
pub fn topic_dir(anchor: &Path, other: &str, name: &str) -> Option<PathBuf> {
    let key = norm(&subject_words(name).join(" "));
    if key.is_empty() {
        return None;
    }
    let base = anchor.join(other);
    sub_dirs(&base)
        .into_iter()
        .filter(|d| {
            let k = norm(d);
            k.chars().count() >= 4 && meaningful(&k) && key.contains(&k)
        })
        .max_by_key(|d| norm(d).chars().count())
        .map(|d| base.join(d))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(n: &str) -> Item {
        Item {
            name: n.into(),
            inner: None,
        }
    }

    #[test]
    fn subject_words_drop_dates_numbers_types() {
        assert_eq!(subject_words("현대홈쇼핑_13일.zip"), vec!["현대홈쇼핑"]);
        assert_eq!(
            subject_words("홈쇼핑 13일 처리건.zip"),
            vec!["홈쇼핑", "처리건"]
        );
        assert_eq!(
            subject_words("케이티_정상고객사_리스트.xlsx.zip"),
            vec!["케이티", "정상고객사", "리스트"]
        );
        assert_eq!(
            subject_words("특수한유형의부가통신사업자 수수료(22~24년)_241221(1차).xlsx.zip"),
            vec!["특수한유형의부가통신사업자", "수수료"]
        );
    }

    #[test]
    fn groups_the_screenshot() {
        let names = [
            "예외 신청 필요 서류.zip",
            "오라클_Concepts.zip",
            "음성 샘플.zip",
            "이모티콘_전체.zip",
            "전설의 Tools.zip",
            "전체 고객.zip",
            "전체아이디_검색결과.xlsx.zip",
            "접속기록_위변조방지_구축.zip",
            "제니엘시스템.zip",
            "차세대 발송 대상.xlsx.zip",
            "참고.zip",
            "최종_[요약] 정산 1차데이터 요청 — 내부 관리용.zip",
            "케이티_정상고객사_리스트.xlsx.zip",
            "크로샷.zip",
            "특수한 유형의 부가통신사업자_관련자료.zip",
            "특수한유형의부가통신사업자 수수료(22~24년)_241221(1차).xlsx.zip",
            "특수한유형의부가통신사업자.zip",
            "패스워드 변경 자료.zip",
            "현대홈쇼핑_13일.zip",
            "현대홈쇼핑_CAMP_13일.zip",
            "홈쇼핑 13일 처리건.zip",
            "odyssey SBOM.zip",
        ];
        let items: Vec<Item> = names.iter().map(|n| item(n)).collect();
        let gs = groups(&items, 3);
        let show: Vec<(String, Vec<&str>, bool)> = gs
            .iter()
            .map(|g| {
                (
                    g.label.clone(),
                    g.members.iter().map(|&i| names[i]).collect(),
                    g.company,
                )
            })
            .collect();
        assert_eq!(gs.len(), 2, "{show:?}");
        let hd = gs
            .iter()
            .find(|g| g.label == "현대홈쇼핑")
            .expect("현대홈쇼핑");
        assert!(hd.company);
        assert_eq!(hd.members.len(), 3, "{show:?}");
        let sp = gs
            .iter()
            .find(|g| norm(&g.label) == "특수한유형의부가통신사업자")
            .expect("부가통신사업자");
        assert!(!sp.company);
        assert_eq!(sp.members.len(), 3);
        // Lone archives: a company name stands alone; others stay.
        assert_eq!(
            company_in("제니엘시스템.zip").as_deref(),
            Some("제니엘시스템")
        );
        assert_eq!(
            company_in("케이티_정상고객사_리스트.xlsx.zip").as_deref(),
            Some("케이티")
        );
        for n in [
            "odyssey SBOM.zip",
            "전체 고객.zip",
            "참고.zip",
            "전설의 Tools.zip",
        ] {
            assert_eq!(company_in(n), None, "{n}");
        }
    }

    #[test]
    fn stop_words_do_not_pair() {
        let items: Vec<Item> = [
            "관련자료 정리.zip",
            "회의 관련자료.zip",
            "고객 리스트 정리.zip",
        ]
        .iter()
        .map(|n| item(n))
        .collect();
        assert!(groups(&items, 2).is_empty());
        // A small topic needs min_topic members; a company does not.
        let two: Vec<Item> = ["부가통신사업자 신고.zip", "부가통신사업자 수수료.zip"]
            .iter()
            .map(|n| item(n))
            .collect();
        assert!(groups(&two, 3).is_empty());
        assert_eq!(groups(&two, 2).len(), 1);
        let co: Vec<Item> = ["가나다텔레콤 정산.zip", "가나다텔레콤_계약.zip"]
            .iter()
            .map(|n| item(n))
            .collect();
        let g = groups(&co, 3);
        assert_eq!(g.len(), 1);
        assert!(g[0].company);
        assert_eq!(g[0].label, "가나다텔레콤");
    }

    #[test]
    fn inner_folder_joins() {
        let items = vec![
            item("현대홈쇼핑_정산.zip"),
            item("현대홈쇼핑 계약.zip"),
            Item {
                name: "참고.zip".into(),
                inner: inner_subject(&["현대홈쇼핑_요청/".into(), "현대홈쇼핑_요청/a.xlsx".into()]),
            },
        ];
        let g = groups(&items, 3);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].members, vec![0, 1, 2]);
        assert_eq!(inner_subject(&["a.txt".into(), "b/c.txt".into()]), None);
    }

    #[test]
    fn spelling_keeps_spaces() {
        assert_eq!(
            spelling(
                "특수한 유형의 부가통신사업자 관련자료",
                "특수한유형의부가통신사업자"
            )
            .as_deref(),
            Some("특수한 유형의 부가통신사업자")
        );
        assert_eq!(
            spelling("현대홈쇼핑 CAMP", "현대홈쇼핑").as_deref(),
            Some("현대홈쇼핑")
        );
        assert_eq!(
            spelling("Odyssey SBOM", "odyssey").as_deref(),
            Some("Odyssey")
        );
    }

    #[test]
    fn topic_dir_by_name() {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(t.path().join("기타/특수한 유형의 부가통신사업자")).unwrap();
        std::fs::create_dir_all(t.path().join("기타/개발")).unwrap();
        assert_eq!(
            topic_dir(t.path(), "기타", "특수한유형의부가통신사업자_2차.zip"),
            Some(t.path().join("기타/특수한 유형의 부가통신사업자"))
        );
        assert_eq!(topic_dir(t.path(), "기타", "개발환경.zip"), None);
    }
}
