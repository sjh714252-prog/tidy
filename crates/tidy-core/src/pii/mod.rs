//! Personal information detection (SPEC §14.2, M9). Offline and regex
//! based: it reads the body text the indexer already extracts (§5.5) plus
//! the file name, and counts four kinds of findings:
//!
//! - **주민등록번호**: `YYMMDD-[1-4]XXXXXX` with a valid date and check digit.
//! - **계좌번호**: 10–14 digits (hyphens allowed, 1–4 groups) on a line
//!   that mentions an account ("계좌", "입금", "예금주", …) or a bank, or
//!   inside a table whose header does. Phone numbers, resident numbers
//!   and business numbers (3-2-5) are not accounts.
//! - **이름 + 휴대전화**: a Korean name and `01[016789]-XXXX-XXXX` on the
//!   same line (a spreadsheet row is one line, its cells tab-separated).
//! - **이름 + 이메일**: a Korean name and an e-mail address on one line.
//!
//! A name is a 3-syllable word that starts with a common surname (4 with a
//! two-syllable surname such as 남궁), not a common word; or any 2–4
//! syllable word right after a label such as "이름:" / "성명". Found values
//! matching an exclude pattern are not counted. A file is personal when
//! any enabled kind reaches its minimum count.
//!
//! Nothing here reads or writes files except [`Detector::judge_path`],
//! which extracts text the same way the indexer does.

use std::path::Path;
use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::config::PiiCfg;

/// `files.pii_flags` bits.
pub const FLAG_RRN: u32 = 1;
pub const FLAG_ACCOUNT: u32 = 2;
pub const FLAG_NAME_PHONE: u32 = 4;
pub const FLAG_NAME_EMAIL: u32 = 8;

/// One kind of personal information.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PiiKind {
    Rrn,
    Account,
    NamePhone,
    NameEmail,
}

impl PiiKind {
    pub const ALL: [PiiKind; 4] = [
        PiiKind::Rrn,
        PiiKind::Account,
        PiiKind::NamePhone,
        PiiKind::NameEmail,
    ];

    pub fn flag(self) -> u32 {
        match self {
            PiiKind::Rrn => FLAG_RRN,
            PiiKind::Account => FLAG_ACCOUNT,
            PiiKind::NamePhone => FLAG_NAME_PHONE,
            PiiKind::NameEmail => FLAG_NAME_EMAIL,
        }
    }

    /// Short label for badges ("주민번호 2 · 이름+휴대전화 15").
    pub fn label(self) -> &'static str {
        match self {
            PiiKind::Rrn => "주민번호",
            PiiKind::Account => "계좌번호",
            PiiKind::NamePhone => "이름+휴대전화",
            PiiKind::NameEmail => "이름+이메일",
        }
    }
}

/// How many of each kind were found (before thresholds).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PiiCounts {
    pub rrn: u32,
    pub account: u32,
    pub name_phone: u32,
    pub name_email: u32,
}

impl PiiCounts {
    pub fn get(&self, k: PiiKind) -> u32 {
        match k {
            PiiKind::Rrn => self.rrn,
            PiiKind::Account => self.account,
            PiiKind::NamePhone => self.name_phone,
            PiiKind::NameEmail => self.name_email,
        }
    }

    fn add(&mut self, o: &PiiCounts) {
        self.rrn += o.rrn;
        self.account += o.account;
        self.name_phone += o.name_phone;
        self.name_email += o.name_email;
    }

    pub fn is_empty(&self) -> bool {
        *self == PiiCounts::default()
    }
}

/// The verdict for one file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PiiStatus {
    /// At least one enabled kind reached its minimum.
    Pii,
    Clean,
    /// No body text to read (image, encrypted or unsupported file,
    /// detection off): "판정 불가".
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PiiFinding {
    pub status: PiiStatus,
    /// Kinds over their threshold (`files.pii_flags`).
    pub flags: u32,
    pub counts: PiiCounts,
}

impl PiiFinding {
    pub fn unknown() -> Self {
        Self {
            status: PiiStatus::Unknown,
            flags: 0,
            counts: PiiCounts::default(),
        }
    }

    pub fn is_pii(&self) -> bool {
        self.status == PiiStatus::Pii
    }

    /// Kinds that made this file personal, in display order.
    pub fn kinds(&self) -> Vec<PiiKind> {
        PiiKind::ALL
            .into_iter()
            .filter(|k| self.flags & k.flag() != 0)
            .collect()
    }

    /// "주민번호 2 · 이름+휴대전화 15" (flagged kinds only).
    pub fn summary(&self) -> String {
        summary(self.flags, &self.counts)
    }

    /// Rebuild from what `files` stores (`pii_flags` NULL = unknown).
    pub fn from_stored(flags: Option<u32>, counts: Option<PiiCounts>) -> Self {
        match flags {
            None => Self::unknown(),
            Some(f) => Self {
                status: if f != 0 {
                    PiiStatus::Pii
                } else {
                    PiiStatus::Clean
                },
                flags: f,
                counts: counts.unwrap_or_default(),
            },
        }
    }

    /// `files.pii_flags` value: `None` for unknown.
    pub fn stored_flags(&self) -> Option<u32> {
        (self.status != PiiStatus::Unknown).then_some(self.flags)
    }
}

/// "주민번호 2 · 이름+휴대전화 15" for the kinds in `flags`.
pub fn summary(flags: u32, counts: &PiiCounts) -> String {
    PiiKind::ALL
        .into_iter()
        .filter(|k| flags & k.flag() != 0)
        .map(|k| format!("{} {}", k.label(), counts.get(k)))
        .collect::<Vec<_>>()
        .join(" · ")
}

/// Compiled detection settings.
#[derive(Debug, Clone)]
pub struct Detector {
    cfg: PiiCfg,
    exclude: Vec<Regex>,
}

impl Detector {
    /// Fails on an invalid exclude pattern (see [`Detector::lenient`]).
    pub fn new(cfg: &PiiCfg) -> Result<Self, regex::Error> {
        let exclude = cfg
            .exclude
            .iter()
            .filter(|e| !e.trim().is_empty())
            .map(|e| Regex::new(e))
            .collect::<Result<_, _>>()?;
        Ok(Self {
            cfg: cfg.clone(),
            exclude,
        })
    }

    /// Like [`Detector::new`], dropping invalid exclude patterns (the
    /// config warning reports them) so detection never stops.
    pub fn lenient(cfg: &PiiCfg) -> Self {
        Self {
            cfg: cfg.clone(),
            exclude: cfg
                .exclude
                .iter()
                .filter(|e| !e.trim().is_empty())
                .filter_map(|e| Regex::new(e).ok())
                .collect(),
        }
    }

    pub fn enabled(&self) -> bool {
        self.cfg.enabled
    }

    pub fn config(&self) -> &PiiCfg {
        &self.cfg
    }

    fn excluded(&self, raw: &str) -> bool {
        if self.exclude.is_empty() {
            return false;
        }
        let digits: String = raw.chars().filter(char::is_ascii_digit).collect();
        self.exclude
            .iter()
            .any(|re| re.is_match(raw) || (!digits.is_empty() && re.is_match(&digits)))
    }

    /// Count findings in `text` (no thresholds).
    pub fn count(&self, text: &str) -> PiiCounts {
        let mut total = PiiCounts::default();
        let mut account_block = false;
        for line in text.lines() {
            let t = line.trim();
            if t.is_empty() || t.starts_with("[sheet ") || t.starts_with("[slide ") {
                account_block = false;
                if t.is_empty() {
                    continue;
                }
            }
            let ctx = mentions_account(t);
            // A header line naming accounts makes the rows below it (up
            // to a blank line / the next sheet) account context too.
            if ctx && !has_long_number(t) {
                account_block = true;
            }
            total.add(&self.count_line(t, ctx || account_block));
        }
        total
    }

    fn count_line(&self, line: &str, account_ctx: bool) -> PiiCounts {
        let mut c = PiiCounts::default();
        let mut phones = 0u32;
        for m in re().number.find_iter(line) {
            let raw = m.as_str();
            let plus = line[..m.start()].ends_with('+');
            let tok = NumberToken::parse(raw, plus);
            if tok.is_rrn() {
                if !self.excluded(raw) {
                    c.rrn += 1;
                }
            } else if tok.is_mobile() {
                if !self.excluded(raw) {
                    phones += 1;
                }
            } else if account_ctx && tok.is_account() && !self.excluded(raw) {
                c.account += 1;
            }
        }
        let emails = re()
            .email
            .find_iter(line)
            .filter(|m| !self.excluded(m.as_str()))
            .count() as u32;
        if phones > 0 || emails > 0 {
            let names = count_names(line);
            if names > 0 {
                c.name_phone = phones.min(names);
                c.name_email = emails.min(names);
            }
        }
        c
    }

    /// Apply the enabled kinds and thresholds to raw counts.
    pub fn judge_counts(&self, counts: PiiCounts) -> PiiFinding {
        let cfg = &self.cfg;
        let mut flags = 0;
        for (k, on, min) in [
            (PiiKind::Rrn, cfg.rrn, cfg.min_rrn),
            (PiiKind::Account, cfg.account, cfg.min_account),
            (PiiKind::NamePhone, cfg.name_phone, cfg.min_name_phone),
            (PiiKind::NameEmail, cfg.name_email, cfg.min_name_email),
        ] {
            if on && counts.get(k) >= min.max(1) {
                flags |= k.flag();
            }
        }
        PiiFinding {
            status: if flags != 0 {
                PiiStatus::Pii
            } else {
                PiiStatus::Clean
            },
            flags,
            counts,
        }
    }

    /// Verdict for a file from its name and extracted body text. Without
    /// body text the file is "판정 불가" unless its name alone is personal.
    pub fn judge(&self, file_name: &str, body: Option<&str>) -> PiiFinding {
        if !self.cfg.enabled {
            return PiiFinding::unknown();
        }
        let mut counts = self.count(file_name);
        match body {
            Some(b) => {
                counts.add(&self.count(b));
                self.judge_counts(counts)
            }
            None => {
                let f = self.judge_counts(counts);
                if f.is_pii() {
                    f
                } else {
                    PiiFinding::unknown()
                }
            }
        }
    }

    /// Extract the text of `path` (same extractors and size limit as the
    /// indexer) and judge it. Read-only.
    pub fn judge_path(&self, path: &Path, max_bytes: u64, max_chars: usize) -> PiiFinding {
        if !self.cfg.enabled {
            return PiiFinding::unknown();
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        let body = if size > max_bytes {
            None
        } else {
            match crate::index::Extractors::default_set().extract(path) {
                Some(Ok(t)) => Some(match t.char_indices().nth(max_chars) {
                    Some((i, _)) => t[..i].to_string(),
                    None => t,
                }),
                _ => None,
            }
        };
        self.judge(&name, body.as_deref())
    }
}

// ── Numbers ─────────────────────────────────────────────────────────────

struct Patterns {
    /// Maximal runs of digits joined by single hyphens.
    number: Regex,
    email: Regex,
    /// Maximal runs of Hangul syllables.
    hangul: Regex,
}

fn re() -> &'static Patterns {
    static P: OnceLock<Patterns> = OnceLock::new();
    P.get_or_init(|| Patterns {
        number: Regex::new(r"[0-9]+(?:-[0-9]+)*").expect("number regex"),
        email: Regex::new(r"[A-Za-z0-9._%+\-]+@[A-Za-z0-9\-]+(?:\.[A-Za-z0-9\-]+)*\.[A-Za-z]{2,}")
            .expect("email regex"),
        hangul: Regex::new(r"[가-힣]+").expect("hangul regex"),
    })
}

struct NumberToken {
    digits: String,
    groups: Vec<usize>,
}

impl NumberToken {
    fn parse(raw: &str, after_plus: bool) -> Self {
        let mut groups: Vec<usize> = raw.split('-').map(str::len).collect();
        let mut digits: String = raw.chars().filter(char::is_ascii_digit).collect();
        // +82-10-1234-5678 → 010-1234-5678
        if after_plus && digits.starts_with("82") && groups.first() == Some(&2) {
            digits = format!("0{}", &digits[2..]);
            groups.remove(0);
            if let Some(g) = groups.first_mut() {
                *g += 1;
            }
        }
        Self { digits, groups }
    }

    fn is_rrn(&self) -> bool {
        let g = &self.groups;
        if !(g == &[6, 7] || g == &[13]) {
            return false;
        }
        let d: Vec<u32> = self.digits.chars().filter_map(|c| c.to_digit(10)).collect();
        let month = d[2] * 10 + d[3];
        let day = d[4] * 10 + d[5];
        if !(1..=12).contains(&month) || !(1..=31).contains(&day) || !(1..=4).contains(&d[6]) {
            return false;
        }
        const W: [u32; 12] = [2, 3, 4, 5, 6, 7, 8, 9, 2, 3, 4, 5];
        let sum: u32 = d.iter().zip(W).map(|(a, w)| a * w).sum();
        (11 - sum % 11) % 10 == d[12]
    }

    fn is_mobile(&self) -> bool {
        let d = &self.digits;
        let n = d.len();
        let prefix = d.len() >= 3
            && d.starts_with("01")
            && matches!(d.as_bytes()[2], b'0' | b'1' | b'6' | b'7' | b'8' | b'9');
        prefix
            && (n == 10 || n == 11)
            && match self.groups.as_slice() {
                [_] => true,
                [3, m, 4] => *m == 3 || *m == 4,
                _ => false,
            }
    }

    /// Any phone shape (mobile, area code, 15xx/16xx) — never an account.
    fn is_phone_like(&self) -> bool {
        if self.is_mobile() {
            return true;
        }
        let g = self.groups.as_slice();
        self.digits.starts_with('0') && g.len() == 3 && g[0] <= 3 && g[1] <= 4 && g[2] == 4
    }

    fn is_account(&self) -> bool {
        let n = self.digits.len();
        let g = self.groups.as_slice();
        (10..=14).contains(&n)
            && g.len() <= 4
            && (g.len() == 1 || g.iter().all(|&x| x >= 2))
            && g != [3, 2, 5] // 사업자등록번호
            && !self.is_phone_like()
            && !self.is_rrn()
    }
}

fn has_long_number(line: &str) -> bool {
    re().number
        .find_iter(line)
        .any(|m| m.as_str().chars().filter(char::is_ascii_digit).count() >= 10)
}

const ACCOUNT_WORDS: &[&str] = &["계좌", "입금", "예금주", "이체", "송금", "account"];
const BANKS: &[&str] = &[
    "은행",
    "국민",
    "신한",
    "우리",
    "하나",
    "농협",
    "기업",
    "카카오뱅크",
    "케이뱅크",
    "토스뱅크",
    "SC제일",
    "씨티",
    "수협",
    "새마을",
    "신협",
    "우체국",
    "KB",
    "NH",
    "IBK",
];

fn mentions_account(line: &str) -> bool {
    let lower = line.to_lowercase();
    ACCOUNT_WORDS.iter().any(|w| lower.contains(w)) || BANKS.iter().any(|b| line.contains(b))
}

// ── Names ───────────────────────────────────────────────────────────────

const SURNAMES: &str = "김이박최정강조윤장임한오서신권황안송류유홍전고문양손배백허남심노하곽성차주우구민진나지엄채원천방공현함변염여추도소석선설마길연위표명기반왕금옥육인맹제모탁국어은편용예경봉";
const DOUBLE_SURNAMES: &[&str] = &[
    "남궁", "황보", "제갈", "선우", "독고", "사공", "서문", "동방",
];
/// Labels that put a name right after them ("이름: 김구", "성명 홍길동").
const NAME_LABELS: &[&str] = &[
    "이름",
    "성명",
    "성함",
    "고객명",
    "담당자",
    "예금주",
    "수령인",
    "신청인",
    "작성자",
    "보호자",
];
/// Common words that look like surname + 2 syllables.
const NOT_NAMES: &[&str] = &[
    "연락처",
    "이메일",
    "이름은",
    "이름을",
    "이번에",
    "이용자",
    "이용료",
    "이상의",
    "이하의",
    "정보를",
    "정리함",
    "전화로",
    "전화번",
    "주소지",
    "주소는",
    "주민번",
    "주민등",
    "주문자",
    "주문번",
    "신청서",
    "신청인",
    "신청자",
    "신청일",
    "성명란",
    "성명은",
    "성함은",
    "안내문",
    "안녕하",
    "한국어",
    "한국인",
    "한글판",
    "문의처",
    "문의는",
    "문서함",
    "방문자",
    "서비스",
    "서명란",
    "조회수",
    "강의실",
    "장소는",
    "우체국",
    "최근에",
    "최대한",
    "최소한",
    "최종본",
    "한번에",
    "하루에",
    "고객명",
    "고객님",
    "홍보용",
    "송금액",
    "입금액",
    "지급액",
    "지점장",
    "차량번",
    "배송지",
    "배송비",
    "원본은",
    "임대인",
    "임차인",
    "사용자",
    "사업자",
    "담당자",
    "예금주",
    "수령인",
    "작성자",
    "보호자",
    "전자우",
    "정기적",
    "정상적",
    "유의사",
    "기타사",
    "비고란",
    "부서명",
    "소속은",
    "직급은",
    "나이는",
    "성별은",
    "휴대폰",
    "전화는",
    "메일은",
    "공지사",
    "오늘은",
    "내일은",
    "어제는",
    "우리는",
    "우리의",
    "하나의",
    "신한은",
    "국민은",
    "농협은",
    "기업은",
    "하나은",
    "우리은",
    "이체일",
    "계좌는",
    "금액은",
    "합계는",
    "인터넷",
    "양식은",
    "방법은",
    "구분은",
    "인원수",
    "위원회",
    "위원장",
    "표준안",
    "명세서",
    "기간은",
    "모바일",
    "전체를",
];

/// Verb endings and particles: "주세요", "보냅니다", "회의에서" are not names.
const NOT_NAME_ENDINGS: &[&str] = &[
    "세요", "니다", "해요", "하다", "이다", "에서", "으로", "에게", "까지", "부터", "습니", "합니",
];

fn count_names(line: &str) -> u32 {
    let mut n = 0;
    let mut after_label = false;
    for m in re().hangul.find_iter(line) {
        let w = m.as_str();
        if NAME_LABELS.iter().any(|l| w == *l || w.starts_with(l)) && !is_label_with_name(w) {
            after_label = true;
            continue;
        }
        let len = w.chars().count();
        let labelled =
            std::mem::take(&mut after_label) && (2..=4).contains(&len) && !NOT_NAMES.contains(&w);
        if labelled || is_name(w) {
            n += 1;
        }
    }
    n
}

/// "이름홍길동" glued together: the label followed by a name.
fn is_label_with_name(w: &str) -> bool {
    NAME_LABELS
        .iter()
        .any(|l| w.strip_prefix(l).is_some_and(is_name))
}

fn is_name(w: &str) -> bool {
    let w = w
        .strip_suffix('님')
        .or_else(|| w.strip_suffix('씨'))
        .unwrap_or(w);
    let chars: Vec<char> = w.chars().collect();
    if NOT_NAMES.contains(&w) || NOT_NAME_ENDINGS.iter().any(|e| w.ends_with(e)) {
        return false;
    }
    match chars.len() {
        3 => SURNAMES.contains(chars[0]),
        4 => DOUBLE_SURNAMES.iter().any(|s| w.starts_with(s)),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn det() -> Detector {
        Detector::new(&PiiCfg::default()).unwrap()
    }

    /// A valid resident number for a birth date (check digit computed).
    pub(crate) fn rrn(front: &str, back6: &str, gender: u32) -> String {
        let s = format!("{front}{gender}{back6}");
        let d: Vec<u32> = s.chars().map(|c| c.to_digit(10).unwrap()).collect();
        const W: [u32; 12] = [2, 3, 4, 5, 6, 7, 8, 9, 2, 3, 4, 5];
        let sum: u32 = d.iter().zip(W).map(|(a, w)| a * w).sum();
        format!("{front}-{gender}{back6}{}", (11 - sum % 11) % 10)
    }

    #[test]
    fn rrn_needs_date_gender_and_check_digit() {
        let ok = rrn("900101", "23456", 1);
        assert_eq!(det().count(&format!("주민등록번호: {ok}")).rrn, 1);
        assert_eq!(det().count(&ok.replace('-', "")).rrn, 1, "without hyphen");
        // Wrong check digit.
        let mut bad = ok.clone();
        let last = bad.pop().unwrap().to_digit(10).unwrap();
        bad.push(char::from_digit((last + 1) % 10, 10).unwrap());
        assert_eq!(det().count(&bad).rrn, 0);
        // Month 13, gender 5, masked, inside a longer number.
        assert_eq!(det().count(&rrn("901301", "23456", 1)).rrn, 0);
        assert_eq!(det().count("900101-5234567").rrn, 0);
        assert_eq!(det().count("900101-1******").rrn, 0);
        assert_eq!(det().count(&format!("{ok}9")).rrn, 0);
    }

    #[test]
    fn account_needs_context() {
        assert_eq!(det().count("입금 계좌: 123456-12-123456").account, 1);
        assert_eq!(det().count("국민 123456-12-123456 홍길동").account, 1);
        assert_eq!(det().count("신한은행 110123456789").account, 1);
        // No account words: an order or tracking number.
        assert_eq!(det().count("주문번호 123456-12-123456").account, 0);
        // Phone, date, business number are never accounts.
        assert_eq!(det().count("계좌 문의 010-1234-5678").account, 0);
        assert_eq!(det().count("입금일 2026-10-19").account, 0);
        assert_eq!(det().count("입금처 사업자 123-45-67890").account, 0);
        assert_eq!(det().count("계좌 02-123-4567").account, 0);
    }

    #[test]
    fn account_header_covers_table_rows() {
        let table = "[sheet 급여]\n이름\t은행\t계좌번호\n홍길동\t\t1002-123-456789\n김영희\t\t3333-01-1234567\n\n다른 표 1002-123-456789";
        assert_eq!(det().count(table).account, 2);
    }

    #[test]
    fn name_and_phone_on_one_line() {
        assert_eq!(det().count("홍길동 010-1234-5678").name_phone, 1);
        assert_eq!(
            det().count("담당: 김철수님 (010.1234.5678)").name_phone,
            0,
            "dots are not a phone"
        );
        assert_eq!(det().count("김철수님 01012345678").name_phone, 1);
        assert_eq!(det().count("+82-10-1234-5678 남궁민수").name_phone, 1);
        assert_eq!(
            det().count("성명: 김구\t휴대폰 010-9876-5432").name_phone,
            1
        );
        // Different lines, no name, common words.
        assert_eq!(det().count("홍길동\n010-1234-5678").name_phone, 0);
        assert_eq!(det().count("연락처 010-1234-5678").name_phone, 0);
        assert_eq!(det().count("고객센터 010-1234-5678").name_phone, 0);
        // Landline is not a mobile number.
        assert_eq!(det().count("홍길동 02-123-4567").name_phone, 0);
        // Excluded test number.
        assert_eq!(det().count("홍길동 010-0000-0000").name_phone, 0);
    }

    #[test]
    fn name_and_email_on_one_line() {
        assert_eq!(det().count("박지성\tjs.park@company.co.kr").name_email, 1);
        assert_eq!(det().count("문의: help@company.co.kr").name_email, 0);
        assert_eq!(det().count("이메일 hong@company.co.kr").name_email, 0);
    }

    #[test]
    fn rows_are_counted_and_thresholds_apply() {
        let rows: String = (0..15)
            .map(|i| format!("홍길동\t010-1234-{:04}\n", 1000 + i))
            .collect();
        let d = det();
        let f = d.judge("명단.xlsx", Some(&rows));
        assert!(f.is_pii());
        assert_eq!(f.counts.name_phone, 15);
        assert_eq!(f.summary(), "이름+휴대전화 15");

        let mut cfg = PiiCfg {
            min_name_phone: 20,
            ..PiiCfg::default()
        };
        assert!(!Detector::new(&cfg)
            .unwrap()
            .judge("a", Some(&rows))
            .is_pii());
        cfg.min_name_phone = 1;
        cfg.name_phone = false;
        assert_eq!(
            Detector::new(&cfg).unwrap().judge("a", Some(&rows)).status,
            PiiStatus::Clean
        );
    }

    #[test]
    fn exclude_patterns_and_unknown() {
        let cfg = PiiCfg {
            exclude: vec!["^01012345678$".into()],
            ..PiiCfg::default()
        };
        let d = Detector::new(&cfg).unwrap();
        assert_eq!(d.count("홍길동 010-1234-5678").name_phone, 0);
        assert_eq!(d.count("홍길동 010-1234-5679").name_phone, 1);
        assert!(Detector::new(&PiiCfg {
            exclude: vec!["(".into()],
            ..PiiCfg::default()
        })
        .is_err());

        // No text: unknown, unless the name itself says enough.
        assert_eq!(det().judge("사진.png", None).status, PiiStatus::Unknown);
        assert!(det().judge("홍길동 010-1234-5678.png", None).is_pii());
        let off = Detector::new(&PiiCfg {
            enabled: false,
            ..PiiCfg::default()
        })
        .unwrap();
        assert_eq!(
            off.judge("x", Some(&rrn("900101", "23456", 1))).status,
            PiiStatus::Unknown
        );
    }

    #[test]
    fn summary_and_stored_round_trip() {
        let c = PiiCounts {
            rrn: 2,
            name_phone: 15,
            ..Default::default()
        };
        let f = det().judge_counts(c);
        assert_eq!(f.summary(), "주민번호 2 · 이름+휴대전화 15");
        assert_eq!(f.flags, FLAG_RRN | FLAG_NAME_PHONE);
        assert_eq!(PiiFinding::from_stored(f.stored_flags(), Some(c)), f);
        assert_eq!(
            PiiFinding::from_stored(None, None).status,
            PiiStatus::Unknown
        );
    }
}
