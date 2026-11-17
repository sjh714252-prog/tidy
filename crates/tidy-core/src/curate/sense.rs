//! (0.18.0, SPEC §14.24) What a file is, read from its name — and, when
//! the name is not enough, from inside (a zip's file list, the first lines
//! of a text file). Offline, no AI; used while watching and by "정리 위치
//! 새로 짜기" so archives and text files land somewhere that says what
//! they are instead of a flat `압축파일/` or `텍스트/`.
//!
//! Three shapes:
//! - **로그 묶음**: `현대홈쇼핑_13일.zip`, `현대홈쇼핑_CAMP_13일.zip`,
//!   `m2x_rcs_2026-06-28_1.log.zip`. Grouped by date:
//!   `{고객사}/로그/2026-11-13 Agent 로그/`. Without a customer the user
//!   is asked which customer it belongs to (확인 대기).
//! - **매뉴얼**: `KTRCS_MANUAL_v2.0.zip` → `서비스/RCS/매뉴얼/` (a
//!   customer's manual → `{고객사}/매뉴얼/`).
//! - **실행 환경·설치 파일**: `jre-8u121-linux-x64.zip` →
//!   `설치파일/Java/Linux/`. (0.18.3) A vendor before the name
//!   (`amazon-corretto-8-aarch64-linux-jdk.zip`) and the CPU (x64, ARM64)
//!   are read too; builds for other CPUs share the OS folder.

use std::io::Read;
use std::path::{Path, PathBuf};

use chrono::{Datelike, Local, NaiveDate};
use regex::Regex;

use super::existing::{norm, reuse_existing};
use super::{clean, ContextDict};

/// Folder (under a customer) that holds log bundles.
pub const LOG_KIND: &str = "로그";
/// Folder (under a customer or a service) that holds manuals.
pub const MANUAL_KIND: &str = "매뉴얼";
/// Top folder for services' material.
pub const SERVICE_DIR: &str = "서비스";
/// Top folder for installers and runtimes.
pub const SETUP_DIR: &str = "설치파일";
/// Rule name of the "어느 고객사 로그인가요?" question in 확인 대기.
pub const ASK_CUSTOMER_RULE: &str = "로그 · 어느 고객사?";

const ARCHIVES: &[&str] = &[
    "zip", "7z", "rar", "tar", "gz", "tgz", "bz2", "xz", "egg", "alz",
];
const LOG_EXTS: &[&str] = &["log", "out"];
const TEXT_EXTS: &[&str] = &["txt", "log", "out"];
/// Service names known without a dictionary (messaging, the user's field).
const BUILTIN_SERVICES: &[&str] = &[
    "RCS",
    "SMS",
    "MMS",
    "LMS",
    "IMS",
    "VoLTE",
    "알림톡",
    "친구톡",
];
const MANUAL_WORDS: &[&str] = &[
    "manual",
    "manuals",
    "매뉴얼",
    "메뉴얼",
    "설명서",
    "사용설명서",
    "사용법",
    "가이드",
    "guide",
    "userguide",
    "handbook",
];
/// (canonical name, first word of the file name). The word must be the
/// whole first word, or the first word minus a version (`go1.21` → `go`).
const RUNTIMES: &[(&str, &[&str])] = &[
    (
        "Java",
        &[
            "jre",
            "jdk",
            "openjdk",
            "java",
            "zulu",
            "temurin",
            "corretto",
            "ojdk",
            "liberica",
            "semeru",
            "sapmachine",
            "dragonwell",
            "graalvm",
            "jbr",
            "kona",
            "adoptopenjdk",
            "openlogic",
        ],
    ),
    ("Node.js", &["node", "nodejs"]),
    ("Python", &["python"]),
    ("Tomcat", &["tomcat", "apache-tomcat"]),
    ("Maven", &["maven", "apache-maven"]),
    ("Gradle", &["gradle"]),
    ("Nginx", &["nginx"]),
    ("MySQL", &["mysql"]),
    ("MariaDB", &["mariadb"]),
    ("PostgreSQL", &["postgresql", "postgres"]),
    ("Redis", &["redis"]),
    ("Git", &["git"]),
    ("Docker", &["docker"]),
    ("Go", &["go"]),
    (".NET", &["dotnet"]),
];
/// (0.18.3) Runtime names that only ever mean a runtime (a distribution
/// name, never a plain word): a version anywhere in the name is enough
/// (`amazon-corretto-jdk_11.0.21_x64.msi`).
const DISTROS: &[&str] = &[
    "openjdk",
    "zulu",
    "temurin",
    "corretto",
    "liberica",
    "semeru",
    "sapmachine",
    "dragonwell",
    "graalvm",
    "adoptopenjdk",
    "openlogic",
];
/// (0.18.3) Vendor words that may come before the runtime's name
/// (`amazon-corretto-…`, `microsoft-jdk-…`, `server-jre-…`).
const VENDORS: &[&str] = &[
    "amazon",
    "aws",
    "eclipse",
    "adoptium",
    "microsoft",
    "msft",
    "bellsoft",
    "azul",
    "ibm",
    "sap",
    "alibaba",
    "oracle",
    "redhat",
    "openlogic",
    "tencent",
    "jetbrains",
    "server",
    "apache",
];
/// Words between a vendor / runtime name and its version that do not
/// change what it is (`ibm-semeru-open-jdk_17…`, `zulu…-ca-jdk8…`).
const RUNTIME_FILLERS: &[&str] = &["open", "jdk", "jre", "ca", "ce", "ee", "hotspot", "fx"];
/// (0.18.3) CPU kinds as shown, with the spellings in file names. Longer
/// spellings first (`x86_64` before `x86`).
const CPUS: &[(&str, &[&str])] = &[
    (
        "x64",
        &["x86_64", "x86-64", "amd64", "x64", "x86x64", "win64"],
    ),
    ("ARM64", &["aarch64", "arm64", "armv8", "arm64v8"]),
    (
        "ARM32",
        &[
            "aarch32", "armhf", "armv7", "armv7l", "arm32", "armel", "arm",
        ],
    ),
    ("x86", &["i386", "i586", "i686", "x86", "x32", "win32"]),
    ("ppc64le", &["ppc64le"]),
    ("ppc64", &["ppc64"]),
    ("s390x", &["s390x"]),
    ("RISC-V", &["riscv64"]),
];
const OSES: &[(&str, &[&str])] = &[
    (
        "Linux",
        &[
            "linux", "el7", "el8", "el9", "rhel", "centos", "ubuntu", "alpine", "musl", "debian",
        ],
    ),
    (
        "Windows",
        &["windows", "win", "win32", "win64", "x64-windows"],
    ),
    ("macOS", &["mac", "macos", "macosx", "osx", "darwin"]),
    ("Solaris", &["solaris", "sunos"]),
    ("AIX", &["aix"]),
];

/// What a file is, found from its name or contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shape {
    /// A log (or a bundle of logs) from one day.
    Log {
        date: NaiveDate,
        /// "로그" or "Agent 로그".
        label: String,
        /// The name or contents say "log" (not just "customer + date").
        strong: bool,
        /// The name had the date (not taken from the modified time).
        dated: bool,
    },
    Manual {
        service: Option<String>,
    },
    Runtime {
        name: &'static str,
        os: Option<&'static str>,
        /// (0.18.3) CPU kind ("x64", "ARM64"): shown, and used as a
        /// folder below the OS only when the user already made one.
        cpu: Option<&'static str>,
    },
}

impl Shape {
    /// Folder name of a log day: "2026-11-13 Agent 로그".
    pub fn log_folder(date: NaiveDate, label: &str) -> String {
        format!("{} {label}", date.format("%Y-%m-%d"))
    }

    /// Short Korean description ("로그 묶음 2026-11-13", "RCS 매뉴얼").
    pub fn describe(&self) -> String {
        match self {
            Shape::Log { date, label, .. } => format!("{label} {}", date.format("%Y-%m-%d")),
            Shape::Manual { service: Some(s) } => format!("{s} {MANUAL_KIND}"),
            Shape::Manual { service: None } => MANUAL_KIND.to_string(),
            Shape::Runtime { name, os, cpu } => {
                match [*os, *cpu].into_iter().flatten().collect::<Vec<_>>() {
                    v if v.is_empty() => format!("{name} 설치 파일"),
                    v => format!("{name} ({}) 설치 파일", v.join(" · ")),
                }
            }
        }
    }
}

/// Names a file can be matched against.
#[derive(Debug, Clone, Default)]
pub struct Clues {
    /// Customers: (display name, spellings normalized by [`norm`]).
    pub customers: Vec<(String, Vec<String>)>,
    /// Known services (dictionary + builtin + existing `서비스/*`).
    pub services: Vec<String>,
}

fn usable(k: &str) -> bool {
    let n = k.chars().count();
    n >= 3 || (n == 2 && !k.is_ascii())
}

impl Clues {
    pub fn from_dict(dict: Option<&ContextDict>, extra: &[String]) -> Self {
        let mut c = Clues::default();
        if let Some(d) = dict {
            for e in &d.entities {
                let mut keys = vec![norm(&e.name)];
                keys.extend(e.aliases.iter().map(|a| norm(a)));
                c.add_customer(&e.name, keys);
            }
            for s in &d.services {
                c.add_service(s);
            }
        }
        for n in extra {
            c.add_customer(n, vec![norm(n)]);
        }
        for s in BUILTIN_SERVICES {
            c.add_service(s);
        }
        c
    }

    pub fn add_customer(&mut self, name: &str, keys: Vec<String>) {
        let k0 = norm(name);
        if k0.is_empty() {
            return;
        }
        let i = match self.customers.iter().position(|(_, ks)| ks.contains(&k0)) {
            Some(i) => i,
            None => {
                self.customers.push((name.to_string(), vec![k0]));
                self.customers.len() - 1
            }
        };
        for k in keys {
            if !k.is_empty() && !self.customers[i].1.contains(&k) {
                self.customers[i].1.push(k);
            }
        }
    }

    pub fn add_service(&mut self, s: &str) {
        let s = s.trim();
        if !s.is_empty() && !self.services.iter().any(|x| norm(x) == norm(s)) {
            self.services.push(s.to_string());
        }
    }

    /// Services already filed under `{anchor}/서비스/` (and top folders
    /// named like a known service).
    pub fn add_service_dirs(&mut self, anchor: &Path) {
        for n in super::existing::sub_dirs(&anchor.join(SERVICE_DIR)) {
            self.add_service(&n);
        }
    }

    /// The customer `text` mentions (longest spelling wins).
    pub fn customer_in(&self, text: &str) -> Option<String> {
        let hay = norm(text);
        let mut best: Option<(usize, usize)> = None;
        for (i, (_, keys)) in self.customers.iter().enumerate() {
            for k in keys {
                if usable(k) && hay.contains(k.as_str()) && best.is_none_or(|(_, l)| k.len() > l) {
                    best = Some((i, k.len()));
                }
            }
        }
        best.map(|(i, _)| self.customers[i].0.clone())
    }

    /// The one customer the entries name (none, or several → `None`).
    pub fn customer_in_list(&self, entries: &[String]) -> Option<String> {
        let mut found: Option<String> = None;
        for e in entries {
            if let Some(c) = self.customer_in(e) {
                match &found {
                    Some(f) if *f != c => return None,
                    _ => found = Some(c),
                }
            }
        }
        found
    }
}

/// Extensions from the end (`a.log.zip` → ["zip", "log"]) and the stem
/// without them.
fn split_exts(name: &str) -> (String, Vec<String>) {
    let mut stem = name.to_string();
    let mut exts = Vec::new();
    for _ in 0..3 {
        let Some((s, e)) = stem.rsplit_once('.') else {
            break;
        };
        let e = e.to_ascii_lowercase();
        let known = ARCHIVES.contains(&e.as_str())
            || LOG_EXTS.contains(&e.as_str())
            || TEXT_EXTS.contains(&e.as_str())
            || matches!(
                e.as_str(),
                "exe"
                    | "msi"
                    | "pdf"
                    | "docx"
                    | "doc"
                    | "hwp"
                    | "pptx"
                    | "rpm"
                    | "deb"
                    | "dmg"
                    | "pkg"
                    | "jar"
                    | "bin"
                    | "sh"
            );
        // `log.1` style rotations: a number after .log.
        let rot = e.chars().all(|c| c.is_ascii_digit()) && s.to_ascii_lowercase().ends_with(".log");
        if s.is_empty() || !(known || rot) {
            break;
        }
        exts.push(e);
        stem = s.to_string();
    }
    (stem, exts)
}

fn words(stem: &str) -> Vec<String> {
    stem.split(|c: char| !c.is_alphanumeric() && c != '.')
        .flat_map(|w| {
            // "v2.0" stays one word; "a.b" names split.
            if w.chars().filter(|c| *c == '.').count() > 0
                && !w
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_digit() || c == 'v' || c == 'V')
            {
                w.split('.').map(str::to_string).collect::<Vec<_>>()
            } else {
                vec![w.to_string()]
            }
        })
        .filter(|w| !w.is_empty())
        .collect()
}

fn re(p: &str) -> Regex {
    Regex::new(p).expect("valid regex")
}

/// A date in the name; month / year missing are taken from `near` (the
/// modified date), never after it.
fn date_in(stem: &str, near: NaiveDate) -> Option<NaiveDate> {
    let full = re(r"(20\d{2})[-_.]?(0[1-9]|1[0-2])[-_.]?(0[1-9]|[12]\d|3[01])(?:\D|$)");
    if let Some(c) = full.captures(stem) {
        let (y, m, d) = (c[1].parse().ok()?, c[2].parse().ok()?, c[3].parse().ok()?);
        if let Some(dt) = NaiveDate::from_ymd_opt(y, m, d) {
            return Some(dt);
        }
    }
    // "0613" (month + day) — year from the modified date.
    if let Some(c) =
        re(r"(?:^|[^0-9])(0[1-9]|1[0-2])(0[1-9]|[12][0-9]|3[01])(?:[^0-9]|$)").captures(stem)
    {
        let (m, d): (u32, u32) = (c[1].parse().ok()?, c[2].parse().ok()?);
        let mut dt = NaiveDate::from_ymd_opt(near.year(), m, d)?;
        if dt > near {
            dt = NaiveDate::from_ymd_opt(near.year() - 1, m, d)?;
        }
        return Some(dt);
    }
    if let Some(c) = re(r"(\d{1,2})\s*월\s*(\d{1,2})\s*일").captures(stem) {
        let (m, d): (u32, u32) = (c[1].parse().ok()?, c[2].parse().ok()?);
        let mut y = near.year();
        let mut dt = NaiveDate::from_ymd_opt(y, m, d)?;
        if dt > near {
            y -= 1;
            dt = NaiveDate::from_ymd_opt(y, m, d)?;
        }
        return Some(dt);
    }
    if let Some(c) = re(r"(?:^|\D)(\d{1,2})\s*일(?:\D|$)").captures(stem) {
        let d: u32 = c[1].parse().ok()?;
        let (mut y, mut m) = (near.year(), near.month());
        for _ in 0..12 {
            if let Some(dt) = NaiveDate::from_ymd_opt(y, m, d) {
                if dt <= near {
                    return Some(dt);
                }
            }
            if m == 1 {
                y -= 1;
                m = 12;
            } else {
                m -= 1;
            }
        }
    }
    None
}

fn is_date_word(w: &str) -> bool {
    let digits = w.chars().filter(char::is_ascii_digit).count();
    (w.ends_with('일') || w.ends_with('월')) && digits > 0 && digits + 1 == w.chars().count()
        || (digits >= 4 && w.chars().all(|c| c.is_ascii_digit()))
}

fn is_version(w: &str) -> bool {
    let w = w.trim_start_matches(['v', 'V']);
    let w = w.strip_prefix("er").unwrap_or(w);
    !w.is_empty()
        && w.chars()
            .all(|c| c.is_ascii_digit() || c == '.' || c == 'u')
        && w.chars().next().is_some_and(|c| c.is_ascii_digit())
}

/// A zip's entry names (at most `max`). Other archives: empty.
pub fn zip_entries(path: &Path, max: usize) -> Vec<String> {
    if crate::paths::ext_lower(path) != "zip" {
        return Vec::new();
    }
    let Ok(f) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let Ok(z) = zip::ZipArchive::new(std::io::BufReader::new(f)) else {
        return Vec::new();
    };
    z.file_names().take(max).map(str::to_string).collect()
}

/// The first `max` bytes of a text file, lossily decoded.
fn text_head(path: &Path, max: u64) -> String {
    let Ok(f) = std::fs::File::open(path) else {
        return String::new();
    };
    let mut buf = Vec::new();
    let _ = f.take(max).read_to_end(&mut buf);
    String::from_utf8_lossy(&buf).into_owned()
}

/// Most of the first lines start with a date / time stamp.
pub fn looks_like_log(text: &str) -> bool {
    let stamp =
        re(r"^\[?\s*(\d{4}[-/.]\d{2}[-/.]\d{2}|\d{2}:\d{2}:\d{2}|\d{2}[-/]\d{2}\s+\d{2}:\d{2})");
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .take(30)
        .collect();
    lines.len() >= 5 && lines.iter().filter(|l| stamp.is_match(l)).count() * 2 >= lines.len()
}

fn modified_date(path: &Path) -> NaiveDate {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .map(|t| chrono::DateTime::<Local>::from(t).date_naive())
        .unwrap_or_else(|_| Local::now().date_naive())
}

/// The runtime / installer a name shows (step 1 of [`shape`]).
fn runtime_in(lower: &[String], ext0: &str, archive: bool) -> Option<Shape> {
    let installer = archive
        || matches!(
            ext0,
            "exe" | "msi" | "rpm" | "deb" | "dmg" | "pkg" | "bin" | "sh"
        );
    if !installer {
        return None;
    }
    // Vendor words first (at most two), then the runtime's name.
    let at = lower
        .iter()
        .take(2)
        .take_while(|w| VENDORS.contains(&w.as_str()))
        .count();
    let first = lower.get(at)?;
    // The name without its version: `OpenJDK8U` → `openjdk`, `zulu8` →
    // `zulu` (a `u` only after digits — zulu, semeru end in one).
    let num = |c: char| c.is_ascii_digit() || c == '.';
    let mut base = first.trim_end_matches(num);
    if let Some(b) = base
        .strip_suffix('u')
        .filter(|b| b.ends_with(|c: char| c.is_ascii_digit()))
    {
        base = b.trim_end_matches(num);
    }
    let two = lower.get(at + 1).map(|s| format!("{first}-{s}"));
    let mut words_used = 1;
    let mut word = "";
    let (rt, _) = RUNTIMES.iter().find(|(_, ws)| {
        ws.iter().any(|w| {
            if two.as_deref() == Some(*w) {
                words_used = 2;
                word = w;
                true
            } else if *w == first || *w == base {
                word = w;
                true
            } else {
                false
            }
        })
    })?;
    // A version right there: in the word (`go1.21`, `OpenJDK8U`) or the
    // next one (`jre-8u121`, `node-v18`, `corretto-jdk-11.0.21`) — not
    // `go_live_2026.zip`. A distribution name (corretto, temurin): a
    // version anywhere after it.
    let versioned = |w: &str| {
        let w = w.trim_start_matches('v');
        w.chars().next().is_some_and(|c| c.is_ascii_digit())
    };
    let rest = &lower[at + words_used..];
    let has_digit = first.chars().any(|c| c.is_ascii_digit())
        || rest
            .iter()
            .find(|w| !RUNTIME_FILLERS.contains(&w.as_str()))
            .is_some_and(|w| versioned(w))
        || (DISTROS.contains(&word) && rest.iter().any(|w| versioned(w)));
    if !has_digit {
        return None;
    }
    let os = OSES
        .iter()
        .find(|(_, keys)| lower.iter().any(|w| keys.contains(&w.as_str())))
        .map(|(o, _)| *o)
        .or(match ext0 {
            "exe" | "msi" => Some("Windows"),
            "rpm" | "deb" => Some("Linux"),
            "dmg" | "pkg" => Some("macOS"),
            _ => None,
        });
    Some(Shape::Runtime {
        name: rt,
        os,
        cpu: cpu_in(lower),
    })
}

/// The CPU kind a name's words show (`x86_64` is split into two words).
fn cpu_in(lower: &[String]) -> Option<&'static str> {
    let joined: Vec<String> = lower
        .windows(2)
        .map(|w| format!("{}_{}", w[0], w[1]))
        .collect();
    CPUS.iter()
        .find(|(_, keys)| {
            keys.iter().any(|k| {
                lower.iter().any(|w| w == k)
                    || (k.contains(['_', '-']) && joined.iter().any(|j| *j == k.replace('-', "_")))
            })
        })
        .map(|(c, _)| *c)
}

/// What `path` is. `customer`: the customer the caller already found for
/// it (its spellings help tell "customer + date" names from others).
/// `entries`: a zip's list when the caller has it (else read here).
pub fn shape(
    path: &Path,
    clues: &Clues,
    customer: Option<&str>,
    entries: Option<&[String]>,
) -> Option<Shape> {
    let name = path.file_name()?.to_string_lossy().into_owned();
    let (stem, exts) = split_exts(&name);
    let ws = words(&stem);
    let lower: Vec<String> = ws.iter().map(|w| w.to_lowercase()).collect();
    let archive = exts.first().is_some_and(|e| ARCHIVES.contains(&e.as_str()));
    let ext0 = exts.first().cloned().unwrap_or_default();

    // 1. Installers and runtimes: the first word names one — or (0.18.3)
    //    the first after vendor words (`amazon-corretto-8-…`).
    if let Some(rt) = runtime_in(&lower, &ext0, archive) {
        return Some(rt);
    }

    // 2. Manuals.
    let joined = norm(&stem);
    let manual = lower.iter().any(|w| MANUAL_WORDS.contains(&w.as_str()))
        || ["매뉴얼", "메뉴얼", "사용설명서", "manual", "userguide"]
            .iter()
            .any(|m| joined.contains(m));
    if manual {
        return Some(Shape::Manual {
            service: service_of(&ws, clues, customer),
        });
    }

    // 3. Logs.
    let near = modified_date(path);
    let date = date_in(&stem, near);
    let listing: Vec<String>;
    let entries = match entries {
        Some(e) => e,
        None => {
            listing = if archive {
                zip_entries(path, 2000)
            } else {
                Vec::new()
            };
            &listing
        }
    };
    let files: Vec<&String> = entries.iter().filter(|e| !e.ends_with('/')).collect();
    let log_files = files
        .iter()
        .filter(|e| {
            let l = e.to_lowercase();
            l.ends_with(".log")
                || l.ends_with(".out")
                || re(r"\.log\.\d+$").is_match(&l)
                || l.contains("/log/")
                || l.contains("/logs/")
        })
        .count();
    let listed_logs = !files.is_empty() && log_files * 2 >= files.len();
    let agent = lower
        .iter()
        .any(|w| w.contains("agent") || w.contains("에이전트"))
        || entries.iter().any(|e| e.to_lowercase().contains("agent"));
    let text_log = !archive
        && TEXT_EXTS.contains(&ext0.as_str())
        && !LOG_EXTS.contains(&ext0.as_str())
        && looks_like_log(&text_head(path, 16 * 1024));
    let strong = exts.iter().any(|e| LOG_EXTS.contains(&e.as_str()))
        || (agent && date.is_some() && archive)
        || lower.iter().any(|w| {
            matches!(
                w.as_str(),
                "log" | "logs" | "로그" | "agentlog" | "agentlogs"
            )
        })
        || listed_logs
        || text_log;
    // "현대홈쇼핑_CAMP_13일.zip": only a customer, short tags and a date.
    let bare = archive
        && date.is_some()
        && customer.is_some_and(|c| {
            let keys: Vec<String> = clues
                .customers
                .iter()
                .find(|(n, _)| n == c)
                .map(|(_, k)| k.clone())
                .unwrap_or_else(|| vec![norm(c)]);
            ws.iter().all(|w| {
                let n = norm(w);
                keys.iter()
                    .any(|k| !k.is_empty() && (n.contains(k.as_str()) || k.contains(n.as_str())))
                    || is_date_word(w)
                    || (w.is_ascii() && w.len() <= 8 && !w.chars().any(|c| c.is_ascii_lowercase()))
                    || w.chars().all(|c| c.is_ascii_digit())
            })
        });
    if strong || bare {
        return Some(Shape::Log {
            date: date.unwrap_or(near),
            label: if agent {
                "Agent 로그".into()
            } else {
                "로그".into()
            },
            strong,
            dated: date.is_some(),
        });
    }
    None
}

/// The service a manual's name shows: a known one inside a word
/// (`KTRCS` → `RCS`), else the word left after a customer prefix, manual
/// words, versions and dates.
fn service_of(ws: &[String], clues: &Clues, customer: Option<&str>) -> Option<String> {
    let rest: Vec<&String> = ws
        .iter()
        .filter(|w| {
            let l = w.to_lowercase();
            !MANUAL_WORDS.contains(&l.as_str())
                && !is_version(w)
                && !is_date_word(w)
                && !l.chars().all(|c| c.is_ascii_digit())
        })
        .collect();
    for w in &rest {
        let n = norm(w);
        if let Some(s) = clues
            .services
            .iter()
            .filter(|s| !norm(s).is_empty() && n.contains(&norm(s)))
            .max_by_key(|s| norm(s).len())
        {
            return Some(s.clone());
        }
    }
    let cust_keys: Vec<String> = customer
        .and_then(|c| {
            clues
                .customers
                .iter()
                .find(|(n, _)| n == c)
                .map(|(_, k)| k.clone())
        })
        .unwrap_or_default();
    rest.iter()
        .filter(|w| {
            let n = norm(w);
            !cust_keys.contains(&n) && clues.customer_in(w).is_none()
        })
        .map(|w| clean(w))
        .find(|w| !w.is_empty())
}

/// Where a shape goes under `anchor`. `customer`: the customer folder's
/// name when the file belongs to one. `None` for a log without a customer
/// (the user is asked). The bool: the customer folder exists already
/// (a new customer folder is asked for, not made).
pub fn dest(anchor: &Path, s: &Shape, customer: Option<&str>) -> Option<(PathBuf, bool)> {
    let reuse = |p: PathBuf| reuse_existing(&p, anchor, 0);
    match (s, customer) {
        (Shape::Log { date, label, .. }, Some(c)) => {
            let (dir, known) = super::kinds::customer_kind_dir(anchor, c, LOG_KIND);
            Some((dir.join(Shape::log_folder(*date, label)), known))
        }
        (Shape::Log { .. }, None) => None,
        (Shape::Manual { .. }, Some(c)) => {
            Some(super::kinds::customer_kind_dir(anchor, c, MANUAL_KIND))
        }
        (Shape::Manual { service }, None) => {
            let svc = service.as_deref().map(clean).filter(|s| !s.is_empty());
            Some((
                match svc {
                    // A top folder of that name made by the user is kept.
                    Some(s)
                        if super::existing::sub_dirs(anchor)
                            .iter()
                            .any(|d| norm(d) == norm(&s)) =>
                    {
                        reuse(anchor.join(&s).join(MANUAL_KIND))
                    }
                    Some(s) => reuse(anchor.join(SERVICE_DIR).join(s).join(MANUAL_KIND)),
                    None => reuse(anchor.join(SERVICE_DIR).join(MANUAL_KIND)),
                },
                true,
            ))
        }
        (Shape::Runtime { name, os, cpu }, _) => {
            let mut p = reuse(anchor.join(SETUP_DIR)).join(clean(name));
            if let Some(o) = os {
                p.push(o);
            }
            let mut p = reuse(p);
            // (0.18.3) Builds for each CPU share the OS folder (no folder
            // of one or two files, §14.19) — unless the user made one.
            if let Some(dir) = cpu.and_then(|c| cpu_dir(&p, c)) {
                p.push(dir);
            }
            Some((p, true))
        }
    }
}

/// An existing sub-folder of `dir` named for CPU kind `cpu` (`x64`,
/// `amd64`, `ARM64`, `aarch64` …, case and spaces ignored).
fn cpu_dir(dir: &Path, cpu: &str) -> Option<String> {
    let keys: Vec<String> = CPUS
        .iter()
        .find(|(c, _)| *c == cpu)
        .map(|(c, ks)| {
            std::iter::once(*c)
                .chain(ks.iter().copied())
                .map(norm)
                .collect()
        })
        .unwrap_or_default();
    super::existing::sub_dirs(dir)
        .into_iter()
        .find(|d| keys.contains(&norm(d)))
}

/// For a log without a customer: the folder below the customer's
/// (`로그/2026-06-28 로그`) the user's answer completes.
pub fn customer_sub(s: &Shape) -> Option<PathBuf> {
    match s {
        Shape::Log { date, label, .. } => {
            Some(Path::new(LOG_KIND).join(Shape::log_folder(*date, label)))
        }
        Shape::Manual { .. } => Some(PathBuf::from(MANUAL_KIND)),
        Shape::Runtime { .. } => None,
    }
}

/// A log that should be asked about when no customer is known: says "log"
/// itself and is a bundle or dated (`service_out.log` stays with the rules).
pub fn ask_customer(s: &Shape, path: &Path) -> bool {
    let archive = ARCHIVES.contains(&crate::paths::ext_lower(path).as_str());
    matches!(s, Shape::Log { strong: true, dated, .. } if archive || *dated)
}

/// The words a family of logs shares, before its date / number
/// (`m2x_rcs_2026-06-28_1.log.zip` → `m2x_rcs`). Remembered as the
/// customer's alias when the user answers "어느 고객사?".
pub fn name_prefix(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_string_lossy().into_owned();
    let (stem, _) = split_exts(&name);
    let mut out: Vec<String> = Vec::new();
    for w in words(&stem) {
        if w.chars().all(|c| c.is_ascii_digit() || c == '.') || is_date_word(&w) {
            break;
        }
        out.push(w);
    }
    let p = out.join("_");
    (norm(&p).chars().count() >= 3).then_some(p)
}

/// A top folder of an organize root that holds a customer's material:
/// it has a dated issue folder or a `로그` / `매뉴얼` folder.
fn customer_like(dir: &Path) -> bool {
    super::existing::sub_dirs(dir).iter().any(|n| {
        super::existing::split_dated(n).is_some()
            || norm(n) == norm(LOG_KIND)
            || norm(n) == norm(MANUAL_KIND)
    })
}

fn layout_top(name: &str) -> bool {
    let k = norm(name);
    k.is_empty()
        || [SERVICE_DIR, SETUP_DIR, "기타", LOG_KIND, MANUAL_KIND]
            .iter()
            .any(|x| norm(x) == k)
        || super::kinds::is_kind_name(None, name)
        || name
            .chars()
            .all(|c| c.is_ascii_digit() || c == '-' || c == '_')
}

/// Clues for files organized into `anchor`: the dictionary's customers and
/// services, the cache's customers, customer-like top folders, and the
/// services already under `서비스/`.
pub fn clues_for(
    dict: Option<&ContextDict>,
    store: Option<&crate::store::Store>,
    anchor: &Path,
) -> Clues {
    let names = dict
        .map(|d| super::customer_names(d, store))
        .unwrap_or_default();
    let mut c = Clues::from_dict(dict, &names);
    for n in super::existing::sub_dirs(anchor) {
        if !layout_top(&n) && customer_like(&anchor.join(&n)) {
            c.add_customer(&n, vec![norm(&n)]);
        }
    }
    c.add_service_dirs(anchor);
    c
}

/// One answer to "어느 고객사 자료인가요?".
#[derive(Debug, Clone, serde::Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CustomerChoice {
    pub name: String,
    /// Its folder under the organize root (made when chosen if missing).
    pub dir: PathBuf,
    pub exists: bool,
    /// In the dictionary or the cache (not only a folder).
    pub known: bool,
}

/// Customers to offer for a file organized into `anchor`: known customers
/// (dictionary, cache) first, then the other top folders that are not
/// Tidy's layout (`문서`, `2026`, `기타`, `서비스`, `설치파일`).
pub fn customer_choices(
    dict: Option<&ContextDict>,
    store: Option<&crate::store::Store>,
    anchor: &Path,
) -> Vec<CustomerChoice> {
    let mut out: Vec<CustomerChoice> = Vec::new();
    let tops = super::existing::sub_dirs(anchor);
    let names = dict
        .map(|d| super::customer_names(d, store))
        .unwrap_or_default();
    for n in names {
        let k = norm(&n);
        if k.is_empty() || out.iter().any(|c| norm(&c.name) == k) {
            continue;
        }
        let dir = tops
            .iter()
            .find(|t| norm(t) == k)
            .map(|t| anchor.join(t))
            .unwrap_or_else(|| anchor.join(clean(&n)));
        out.push(CustomerChoice {
            exists: dir.is_dir(),
            name: n,
            dir,
            known: true,
        });
    }
    for t in tops {
        if layout_top(&t) || t.starts_with('.') || out.iter().any(|c| norm(&c.name) == norm(&t)) {
            continue;
        }
        out.push(CustomerChoice {
            dir: anchor.join(&t),
            name: t,
            exists: true,
            known: false,
        });
    }
    out
}

/// The "어느 고객사?" answer: `customer` (a choice's name, or a new name)
/// → the folder the file goes to. `suggested` is the queue's suggestion
/// (`{정리 위치}/로그/2026-06-28 로그`); its last two steps go below the
/// customer's folder.
pub fn answer_dest(
    dict: Option<&ContextDict>,
    store: Option<&crate::store::Store>,
    suggested: &Path,
    customer: &str,
) -> Option<PathBuf> {
    let folder = suggested.file_name()?;
    let kind = suggested.parent()?;
    let anchor = kind.parent()?;
    let k = norm(customer);
    if k.is_empty() {
        return None;
    }
    let dir = customer_choices(dict, store, anchor)
        .into_iter()
        .find(|c| norm(&c.name) == k)
        .map(|c| c.dir)
        .unwrap_or_else(|| anchor.join(clean(customer)));
    Some(reuse_existing(&dir.join(kind.file_name()?), anchor, 0).join(folder))
}

/// Remember the answer: the log family's name ([`name_prefix`]) becomes
/// the customer's alias in the dictionary (the customer is added when
/// new), so the next such log goes there without asking. Returns the alias.
pub fn remember_answer(
    dict_path: &Path,
    customer: &str,
    file: &Path,
) -> Result<Option<String>, super::ContextError> {
    let Some(prefix) = name_prefix(file) else {
        return Ok(None);
    };
    let mut d = ContextDict::load(dict_path)?;
    let known = d.entities.iter().any(|e| {
        norm(&e.name) == norm(customer) || e.aliases.iter().any(|a| norm(a) == norm(customer))
    });
    if known {
        d.add_alias(customer, &prefix)?;
    } else {
        d.add_entity(customer, std::slice::from_ref(&prefix))?;
    }
    d.save(dict_path)?;
    Ok(Some(prefix))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clues() -> Clues {
        let mut d = ContextDict::default();
        d.entities.push(super::super::Entity {
            name: "현대홈쇼핑".into(),
            aliases: vec!["HMALL".into()],
            notes: String::new(),
        });
        d.entities.push(super::super::Entity {
            name: "KT".into(),
            aliases: vec![],
            notes: String::new(),
        });
        Clues::from_dict(Some(&d), &[])
    }

    fn at(dir: &Path, name: &str, body: &[u8]) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn customer_day_bundles_are_logs_of_that_day() {
        let t = tempfile::tempdir().unwrap();
        let c = clues();
        let p = at(t.path(), "현대홈쇼핑_CAMP_13일.zip", b"x");
        let cust = c.customer_in("현대홈쇼핑_CAMP_13일");
        assert_eq!(cust.as_deref(), Some("현대홈쇼핑"));
        let s = shape(&p, &c, cust.as_deref(), Some(&[])).unwrap();
        let Shape::Log {
            date,
            label,
            strong,
            ..
        } = &s
        else {
            panic!("{s:?}")
        };
        assert_eq!(date.day(), 13);
        assert!(*date <= Local::now().date_naive());
        assert_eq!(label, "로그");
        assert!(!strong);
        let (d, known) = dest(t.path(), &s, cust.as_deref()).unwrap();
        assert!(!known);
        assert!(d.ends_with(format!(
            "현대홈쇼핑/로그/{}",
            Shape::log_folder(*date, "로그")
        )));
        // The zip's list says agent → "Agent 로그".
        let e = vec!["agent/".to_string(), "agent/rcs_agent.log".to_string()];
        let s = shape(&p, &c, cust.as_deref(), Some(&e)).unwrap();
        assert!(matches!(&s, Shape::Log { label, strong: true, .. } if label == "Agent 로그"));
        // A customer's ordinary archive is not a log.
        let q = at(t.path(), "현대홈쇼핑 제안서 자료.zip", b"x");
        assert_eq!(shape(&q, &c, cust.as_deref(), Some(&[])), None);
    }

    #[test]
    fn dated_log_archive_without_customer_is_asked() {
        let t = tempfile::tempdir().unwrap();
        let c = clues();
        let p = at(t.path(), "m2x_rcs_2026-06-28_1.log.zip", b"x");
        assert_eq!(c.customer_in("m2x_rcs_2026-06-28_1.log"), None);
        let s = shape(&p, &c, None, Some(&[])).unwrap();
        assert_eq!(
            s,
            Shape::Log {
                date: NaiveDate::from_ymd_opt(2026, 6, 28).unwrap(),
                label: "로그".into(),
                strong: true,
                dated: true
            }
        );
        assert!(ask_customer(&s, &p));
        assert_eq!(dest(t.path(), &s, None), None);
        assert_eq!(
            customer_sub(&s).unwrap(),
            Path::new("로그").join("2026-06-28 로그")
        );
        // An undated plain .log stays with the rules.
        let q = at(t.path(), "service_out.log", b"x");
        let s = shape(&q, &c, None, None).unwrap();
        assert!(!ask_customer(&s, &q));
    }

    #[test]
    fn manuals_go_to_their_service() {
        let t = tempfile::tempdir().unwrap();
        let c = clues();
        let p = at(t.path(), "KTRCS_MANUAL_v2.0.zip", b"x");
        let s = shape(&p, &c, None, Some(&[])).unwrap();
        assert_eq!(
            s,
            Shape::Manual {
                service: Some("RCS".into())
            }
        );
        let (d, _) = dest(t.path(), &s, None).unwrap();
        assert_eq!(d, t.path().join("서비스").join("RCS").join("매뉴얼"));
        let q = at(t.path(), "가온 알림톡 사용설명서 v1.2.pdf", b"x");
        assert_eq!(
            shape(&q, &c, None, None).unwrap(),
            Shape::Manual {
                service: Some("알림톡".into())
            }
        );
        let r = at(t.path(), "Bizgate_userguide_2.1.pdf", b"x");
        assert_eq!(
            shape(&r, &c, None, None).unwrap(),
            Shape::Manual {
                service: Some("Bizgate".into())
            }
        );
        // A user-made top folder "RCS" is used as it is.
        std::fs::create_dir(t.path().join("RCS")).unwrap();
        let s = Shape::Manual {
            service: Some("RCS".into()),
        };
        assert_eq!(
            dest(t.path(), &s, None).unwrap().0,
            t.path().join("RCS").join("매뉴얼")
        );
    }

    #[test]
    fn runtimes_go_to_setup_by_os() {
        let t = tempfile::tempdir().unwrap();
        let c = clues();
        let p = at(t.path(), "jre-8u121-linux-x64.zip", b"x");
        let s = shape(&p, &c, None, Some(&[])).unwrap();
        assert_eq!(
            s,
            Shape::Runtime {
                name: "Java",
                os: Some("Linux"),
                cpu: Some("x64"),
            }
        );
        assert_eq!(
            dest(t.path(), &s, None).unwrap().0,
            t.path().join("설치파일").join("Java").join("Linux")
        );
        assert_eq!(s.describe(), "Java (Linux · x64) 설치 파일");
        for (n, rt, os, cpu) in [
            (
                "jdk-11.0.2_windows-x64_bin.zip",
                "Java",
                Some("Windows"),
                Some("x64"),
            ),
            (
                "OpenJDK8U-jdk_x64_mac_hotspot_8u412.tar.gz",
                "Java",
                Some("macOS"),
                Some("x64"),
            ),
            (
                "node-v18.17.0-x64.msi",
                "Node.js",
                Some("Windows"),
                Some("x64"),
            ),
            ("apache-tomcat-9.0.85.tar.gz", "Tomcat", None, None),
            (
                "go1.21.5.linux-amd64.tar.gz",
                "Go",
                Some("Linux"),
                Some("x64"),
            ),
        ] {
            let q = at(t.path(), n, b"x");
            assert_eq!(
                shape(&q, &c, None, Some(&[])),
                Some(Shape::Runtime { name: rt, os, cpu }),
                "{n}"
            );
        }
        // Not a runtime: no version, or a word that only starts like one.
        for n in [
            "java 교육 자료.zip",
            "nodemon 설정.txt",
            "github_backup.zip",
        ] {
            let q = at(t.path(), n, b"x");
            assert!(
                !matches!(shape(&q, &c, None, Some(&[])), Some(Shape::Runtime { .. })),
                "{n}"
            );
        }
    }

    /// (0.18.3) Vendor names before the runtime and CPU kinds: the same
    /// Java build for two CPUs lands in one OS folder.
    #[test]
    fn java_builds_by_vendor_and_cpu() {
        let t = tempfile::tempdir().unwrap();
        let c = clues();
        let linux = t.path().join("설치파일").join("Java").join("Linux");
        for (n, os, cpu) in [
            (
                "amazon-corretto-8-aarch64-linux-jdk.zip",
                Some("Linux"),
                Some("ARM64"),
            ),
            (
                "amazon-corretto-8-x64-linux-jdk.zip",
                Some("Linux"),
                Some("x64"),
            ),
            (
                "amazon-corretto-17.0.9.8.1-linux-x64.tar.gz",
                Some("Linux"),
                Some("x64"),
            ),
            (
                "amazon-corretto-11-x64-windows-jdk.msi",
                Some("Windows"),
                Some("x64"),
            ),
            (
                "amazon-corretto-jdk_11.0.21.9-1_amd64.deb",
                Some("Linux"),
                Some("x64"),
            ),
            (
                "OpenJDK17U-jdk_aarch64_linux_hotspot_17.0.9_9.tar.gz",
                Some("Linux"),
                Some("ARM64"),
            ),
            (
                "OpenJDK11U-jre_x86-32_windows_hotspot_11.0.21_9.zip",
                Some("Windows"),
                Some("x86"),
            ),
            (
                "microsoft-jdk-17.0.9-windows-aarch64.zip",
                Some("Windows"),
                Some("ARM64"),
            ),
            (
                "microsoft-jdk-21.0.1-macOS-x64.pkg",
                Some("macOS"),
                Some("x64"),
            ),
            (
                "zulu8.74.0.17-ca-jdk8.0.392-linux_x64.tar.gz",
                Some("Linux"),
                Some("x64"),
            ),
            (
                "zulu17.46.19-ca-jdk17.0.9-linux_aarch64.zip",
                Some("Linux"),
                Some("ARM64"),
            ),
            (
                "bellsoft-jdk17.0.9+11-linux-aarch64.tar.gz",
                Some("Linux"),
                Some("ARM64"),
            ),
            (
                "bellsoft-jre8u392+9-windows-i586.msi",
                Some("Windows"),
                Some("x86"),
            ),
            (
                "ibm-semeru-open-jdk_x64_linux_17.0.9_9_openj9-0.41.0.tar.gz",
                Some("Linux"),
                Some("x64"),
            ),
            (
                "graalvm-community-jdk-21.0.1_linux-x64_bin.tar.gz",
                Some("Linux"),
                Some("x64"),
            ),
            (
                "server-jre-8u202-linux-x64.tar.gz",
                Some("Linux"),
                Some("x64"),
            ),
            (
                "jdk-8u202-linux-arm64-vfp-hflt.tar.gz",
                Some("Linux"),
                Some("ARM64"),
            ),
            ("jdk-8u202-linux-i586.rpm", Some("Linux"), Some("x86")),
            (
                "jdk-17.0.9_linux-x86_64_bin.tar.gz",
                Some("Linux"),
                Some("x64"),
            ),
            (
                "jre-8u391-linux-ppc64le.tar.gz",
                Some("Linux"),
                Some("ppc64le"),
            ),
            (
                "OpenJDK21U-jdk_s390x_linux_hotspot_21.0.1_12.tar.gz",
                Some("Linux"),
                Some("s390x"),
            ),
        ] {
            let q = at(t.path(), n, b"x");
            let s = shape(&q, &c, None, Some(&[]));
            assert_eq!(
                s,
                Some(Shape::Runtime {
                    name: "Java",
                    os,
                    cpu
                }),
                "{n}"
            );
            if os == Some("Linux") {
                assert_eq!(dest(t.path(), &s.unwrap(), None).unwrap().0, linux, "{n}");
            }
        }
        // A CPU folder the user made is used (any spelling of that CPU).
        std::fs::create_dir_all(linux.join("aarch64")).unwrap();
        let q = at(t.path(), "amazon-corretto-8-aarch64-linux-jdk.zip", b"x");
        let s = shape(&q, &c, None, Some(&[])).unwrap();
        assert_eq!(dest(t.path(), &s, None).unwrap().0, linux.join("aarch64"));
        let q = at(t.path(), "amazon-corretto-8-x64-linux-jdk.zip", b"x");
        let s = shape(&q, &c, None, Some(&[])).unwrap();
        assert_eq!(dest(t.path(), &s, None).unwrap().0, linux);
        assert_eq!(s.describe(), "Java (Linux · x64) 설치 파일");
        // Vendor words alone, or a vendor before a plain word: not a runtime.
        for n in [
            "amazon 주문내역 2026.zip",
            "microsoft-teams-backup.zip",
            "server_backup_20261101.zip",
            "oracle-java-교육자료.zip",
            "corretto 소개.pdf",
            "armory-2.0.zip",
        ] {
            let q = at(t.path(), n, b"x");
            assert!(
                !matches!(shape(&q, &c, None, Some(&[])), Some(Shape::Runtime { .. })),
                "{n}"
            );
        }
    }

    #[test]
    fn prefix_and_choices() {
        assert_eq!(
            name_prefix(Path::new("m2x_rcs_2026-06-28_1.log.zip")).as_deref(),
            Some("m2x_rcs")
        );
        assert_eq!(name_prefix(Path::new("12_3.log")), None);
        let t = tempfile::tempdir().unwrap();
        for d in [
            "가람정보/20261014_과금",
            "문서/2026",
            "기타",
            "설치파일",
            "현대홈쇼핑/로그",
            "개인 메모",
        ] {
            std::fs::create_dir_all(t.path().join(d)).unwrap();
        }
        let mut dict = ContextDict::default();
        dict.add_entity("신세계", &[]).unwrap();
        let ch = customer_choices(Some(&dict), None, t.path());
        let names: Vec<&str> = ch.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["신세계", "가람정보", "개인 메모", "현대홈쇼핑"]);
        assert!(!ch[0].exists && ch[0].known);
        let c = clues_for(Some(&dict), None, t.path());
        assert_eq!(
            c.customer_in("현대홈쇼핑_13일").as_deref(),
            Some("현대홈쇼핑")
        );
        assert_eq!(c.customer_in("개인 메모 정리").as_deref(), None);
    }

    #[test]
    fn answer_goes_under_the_customer_and_is_remembered() {
        let t = tempfile::tempdir().unwrap();
        let a = t.path().join("정리");
        std::fs::create_dir_all(a.join("가람정보").join("로그")).unwrap();
        let sug = a.join("로그").join("2026-06-28 로그");
        assert_eq!(
            answer_dest(None, None, &sug, "가람 정보").unwrap(),
            a.join("가람정보/로그/2026-06-28 로그")
        );
        assert_eq!(
            answer_dest(None, None, &sug, "새 고객").unwrap(),
            a.join("새 고객/로그/2026-06-28 로그")
        );
        let dict = t.path().join("context.toml");
        let f = Path::new("m2x_rcs_2026-06-28_1.log.zip");
        assert_eq!(
            remember_answer(&dict, "가람정보", f).unwrap().as_deref(),
            Some("m2x_rcs")
        );
        let d = ContextDict::load(&dict).unwrap();
        let c = Clues::from_dict(Some(&d), &[]);
        assert_eq!(
            c.customer_in("m2x_rcs_2026-07-01_3.log").as_deref(),
            Some("가람정보")
        );
    }

    #[test]
    fn text_with_time_stamps_is_a_log() {
        let t = tempfile::tempdir().unwrap();
        let c = clues();
        let body = (0..10)
            .map(|i| format!("2026-06-28 10:00:{i:02} INFO send ok"))
            .collect::<Vec<_>>()
            .join("\n");
        let p = at(t.path(), "rcs_20260628.txt", body.as_bytes());
        let s = shape(&p, &c, None, None).unwrap();
        assert!(matches!(
            s,
            Shape::Log {
                strong: true,
                dated: true,
                ..
            }
        ));
        let q = at(
            t.path(),
            "회의록 20260628.txt",
            "안건\n1. 일정\n2. 예산\n3. 기타\n4. 끝\n".as_bytes(),
        );
        assert_eq!(shape(&q, &c, None, None), None);
    }

    #[test]
    fn zip_list_names_the_customer() {
        let t = tempfile::tempdir().unwrap();
        let c = clues();
        let p = t.path().join("logs_0628.zip");
        {
            let f = std::fs::File::create(&p).unwrap();
            let mut z = zip::ZipWriter::new(f);
            let o: zip::write::SimpleFileOptions = Default::default();
            for n in ["HMALL/agent.log", "HMALL/agent.log.1"] {
                z.start_file(n, o).unwrap();
                std::io::Write::write_all(&mut z, b"x").unwrap();
            }
            z.finish().unwrap();
        }
        let e = zip_entries(&p, 100);
        assert_eq!(c.customer_in_list(&e).as_deref(), Some("현대홈쇼핑"));
        let s = shape(&p, &c, Some("현대홈쇼핑"), None).unwrap();
        assert!(matches!(&s, Shape::Log { label, strong: true, .. } if label == "Agent 로그"));
    }
}
