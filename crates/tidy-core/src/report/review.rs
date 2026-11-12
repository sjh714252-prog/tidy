//! (M19, 0.15.0, SPEC §14.21) "정리 위치 점검": look inside the organize
//! root (`D:\정리`) Tidy has been filling, and propose re-sorting what no
//! longer fits the layout of SPEC §14.19 — as one ordinary plan (preview,
//! tick, apply as one undoable batch). Nothing moves here.
//!
//! What it finds, in this order (each item says which):
//! 1. **같은 폴더 합치기** — folders right below the root whose names differ
//!    only in spacing / case (`KT 스마트메시지` · `KT스마트메시지`), and a
//!    customer folder that ended up under `기타` (`기타/KT스마트메시지`):
//!    the smaller one's contents move into the bigger one.
//! 2. **같은 이슈 합치기** — inside each of those folders, issue folders of
//!    the same issue a few days apart (`20261014_과금` · `20261020_과금`,
//!    within `merge_days`) or spelled differently: into the earliest.
//! 3. **작은 폴더 접기** — [`super::compact`] (kind / sub-topic folders with
//!    fewer than `min_items` files Tidy put there).
//! 4. **맨 위 파일** — loose files directly in the root, by the rules.
//! 5. **제자리 아닌 파일** (AI, only when asked) — files whose name shows a
//!    customer but that sit outside the customer folders (`문서/2026/KT
//!    견적서.pdf`), and loose files no rule takes: placed by work context
//!    with the folders already there reused. Without AI they are counted
//!    (`ai_waiting`) and nothing else happens (CLAUDE.md rule 4).
//!
//! Folders always move whole (never flattened); development projects,
//! hidden folders, watch folders inside the root and protected folders are
//! left alone.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use super::restructure::{plan_from_rules_in, Plan, PlanError, PlanSource, PlannedMove};
use super::scan::ScanError;
use crate::classify::{AiClassifier, RuleSet};
use crate::curate::existing::{norm, split_dated, sub_dirs};
use crate::curate::{customer_names, Curator};
use crate::mover::TMP_MARKER;
use crate::store::Store;
use crate::watcher::IgnoreFilter;

/// Most files sent to the AI in one review.
const MAX_AI_FILES: usize = 300;
/// Files counted per folder when choosing which of two folders to keep.
const COUNT_CAP: usize = 100_000;

/// What a review looks at and with what.
pub struct ReviewOptions<'a> {
    /// Small folders below this many files are folded (0/1 = never).
    pub min_items: usize,
    /// Same issue within this many days → one folder.
    pub merge_days: u32,
    /// The folder for work without a customer (`기타`).
    pub other_dir: String,
    /// Never touched (from, to or inside).
    pub protected: Vec<PathBuf>,
    /// Folders inside the root to leave out (watch folders).
    pub skip: Vec<PathBuf>,
    pub rules: Option<&'a RuleSet>,
    pub filter: Option<&'a IgnoreFilter>,
    /// The dictionary (customer names) — offline.
    pub curator: Option<&'a Curator>,
    /// Set only when the user asked the AI to place the files of step 5.
    pub ai: Option<&'a AiClassifier>,
}

impl ReviewOptions<'_> {
    /// Defaults from `[curate]` and the watch folders inside `root`.
    pub fn base(cfg: &crate::config::Config, root: &Path, protected: Vec<PathBuf>) -> Self {
        let other = cfg
            .curate
            .other_template
            .split(['/', '\\'])
            .next()
            .filter(|s| !s.contains('{') && !s.trim().is_empty())
            .unwrap_or("기타")
            .trim()
            .to_string();
        let canon = dunce::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        let skip = cfg
            .watches
            .iter()
            .map(|w| dunce::canonicalize(&w.path).unwrap_or_else(|_| w.path.clone()))
            .filter(|p| p.starts_with(&canon) && *p != canon)
            .collect();
        ReviewOptions {
            min_items: cfg.curate.min_folder_items,
            merge_days: cfg.curate.merge_days,
            other_dir: other,
            protected,
            skip,
            rules: None,
            filter: None,
            curator: None,
            ai: None,
        }
    }
}

/// The result: a plan (when anything should move) and what was found.
#[derive(Debug, Clone, Serialize)]
pub struct Review {
    pub plan: Option<Plan>,
    /// One line for "nothing to do" / the plan's summary.
    pub summary: String,
    /// Folders merged into another (steps 1–2).
    pub merged: usize,
    /// Small folders folded (step 3).
    pub folded: usize,
    /// Loose top files the rules place (step 4).
    pub by_rules: usize,
    /// Files the AI placed (step 5).
    pub by_ai: usize,
    /// (0.16.0) Customer files outside the customer folders placed
    /// offline into `{고객사}/{종류}` (no AI, or the AI did not place them).
    pub to_customer: usize,
    /// Files the AI could place but was not asked to (no AI, or not
    /// requested) — "AI로 이것도 보기".
    pub ai_waiting: usize,
    /// The AI was asked and failed (the rest of the plan stands).
    pub ai_error: Option<String>,
}

/// A folder (or file) to move into a kept folder.
pub(super) struct Merge {
    pub(super) from: PathBuf,
    pub(super) to: PathBuf,
    pub(super) is_dir: bool,
    pub(super) reason: String,
}

pub(super) fn visible(name: &str) -> bool {
    !name.starts_with('.')
        && !name.contains(TMP_MARKER)
        && !name.eq_ignore_ascii_case("desktop.ini")
}

pub(super) fn count_files(dir: &Path) -> (u64, u64) {
    let (mut n, mut b) = (0u64, 0u64);
    for e in walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .take(COUNT_CAP)
    {
        n += 1;
        b += e.metadata().map(|m| m.len()).unwrap_or(0);
    }
    (n, b)
}

pub(super) fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .map(|r| r.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| p.display().to_string())
}

pub(super) struct Ctx<'a> {
    pub(super) root: PathBuf,
    pub(super) o: &'a ReviewOptions<'a>,
    pub(super) store: &'a Store,
    /// Known customer names (dictionary, aliases, cache).
    pub(super) customers: Vec<String>,
}

impl Ctx<'_> {
    pub(super) fn left_alone(&self, p: &Path) -> bool {
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        !visible(&name)
            || self
                .o
                .protected
                .iter()
                .any(|x| p.starts_with(x) || x.starts_with(p))
            || self
                .o
                .skip
                .iter()
                .any(|x| p.starts_with(x) || x.starts_with(p))
            || self.o.filter.is_some_and(|f| f.is_ignored(p))
    }

    pub(super) fn dirs(&self, dir: &Path) -> Vec<PathBuf> {
        sub_dirs(dir)
            .into_iter()
            .map(|n| dir.join(n))
            .filter(|p| !self.left_alone(p) && !super::relocate::is_dev_project(p))
            .collect()
    }

    /// The customer `name` stands for (dictionary spelling), by name.
    pub(super) fn customer(&self, name: &str) -> Option<String> {
        let k = norm(name);
        if k.is_empty() {
            return None;
        }
        if let Some(c) = self.o.curator {
            if let Some(e) = c.dict().canonical_entity(name) {
                return Some(e.to_string());
            }
            for e in &c.dict().entities {
                if norm(&e.name) == k || e.aliases.iter().any(|a| norm(a) == k) {
                    return Some(e.name.clone());
                }
            }
        }
        self.customers.iter().find(|c| norm(c) == k).cloned()
    }

    /// A customer folder by name or by layout (has `날짜_이슈` folders).
    pub(super) fn is_customer_dir(&self, dir: &Path) -> bool {
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.customer(&name).is_some() || sub_dirs(dir).iter().any(|s| split_dated(s).is_some())
    }
}

/// Merge `members` (existing folders that stand for the same thing) into
/// `keep` (one of them, or a new folder): their contents, folder by
/// folder, with same-issue / same-name sub-folders joined too. `skip`:
/// sub-folders handled elsewhere (customer folders parked under 기타).
/// Returns how many sub-folders merge into another.
fn merge_group(
    ctx: &Ctx<'_>,
    keep: &Path,
    members: &[PathBuf],
    why: &str,
    skip: &[PathBuf],
    out: &mut Vec<Merge>,
) -> usize {
    struct Child {
        path: PathBuf,
        name: String,
        in_keep: bool,
    }
    let mut dirs: Vec<Child> = Vec::new();
    // The kept folder first.
    let mut ordered: Vec<&PathBuf> = members.iter().filter(|m| m.as_path() == keep).collect();
    ordered.extend(members.iter().filter(|m| m.as_path() != keep));
    for m in ordered {
        let in_keep = m.as_path() == keep;
        let Ok(rd) = std::fs::read_dir(m) else {
            continue;
        };
        let mut entries: Vec<_> = rd.filter_map(Result::ok).collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let p = e.path();
            if ctx.left_alone(&p) || skip.contains(&p) {
                continue;
            }
            let Ok(t) = e.file_type() else { continue };
            if t.is_dir() {
                dirs.push(Child {
                    path: p,
                    name: e.file_name().to_string_lossy().into_owned(),
                    in_keep,
                });
            } else if t.is_file() && !in_keep {
                out.push(Merge {
                    to: keep.join(e.file_name()),
                    from: p,
                    is_dir: false,
                    reason: format!("{why} → '{}'", rel(&ctx.root, keep)),
                });
            }
        }
    }
    // Cluster the sub-folders: same issue within merge_days, or the same
    // name apart from spacing / case. Development projects never merge
    // with another folder.
    let mut joined = 0;
    let mut done = vec![false; dirs.len()];
    for i in 0..dirs.len() {
        if done[i] {
            continue;
        }
        done[i] = true;
        let dev = super::relocate::is_dev_project(&dirs[i].path);
        let mut cluster = vec![i];
        if !dev {
            let key = norm(&dirs[i].name);
            let dated = split_dated(&dirs[i].name).map(|(d, s)| (d, norm(s)));
            for j in (i + 1)..dirs.len() {
                if done[j] || super::relocate::is_dev_project(&dirs[j].path) {
                    continue;
                }
                let same_name = norm(&dirs[j].name) == key;
                let same_issue = match (&dated, split_dated(&dirs[j].name)) {
                    (Some((d0, s0)), Some((d, s))) => {
                        norm(s) == *s0 && (d - *d0).num_days().abs() <= i64::from(ctx.o.merge_days)
                    }
                    _ => false,
                };
                if same_name || same_issue {
                    cluster.push(j);
                    done[j] = true;
                }
            }
        }
        joined += cluster.len() - 1;
        // The folder to keep: the earliest dated one, one already in the
        // kept folder first.
        let target = cluster
            .iter()
            .copied()
            .min_by_key(|&k| {
                (
                    split_dated(&dirs[k].name).map(|(d, _)| d),
                    !dirs[k].in_keep,
                    k,
                )
            })
            .unwrap_or(i);
        let to = keep.join(&dirs[target].name);
        for &k in &cluster {
            let c = &dirs[k];
            if c.path == to || (dev && to.exists()) {
                continue; // never pour one code tree into another
            }
            let reason = if k == target {
                format!("{why} → '{}'", rel(&ctx.root, keep))
            } else if split_dated(&c.name).is_some() && norm(&c.name) != norm(&dirs[target].name) {
                format!(
                    "같은 이슈 폴더 합치기({}일 안) → '{}'",
                    ctx.o.merge_days,
                    rel(&ctx.root, &to)
                )
            } else {
                format!("이름만 다른 같은 폴더 합치기 → '{}'", rel(&ctx.root, &to))
            };
            out.push(Merge {
                from: c.path.clone(),
                to: to.clone(),
                is_dir: true,
                reason,
            });
        }
    }
    joined
}

pub(super) fn name_of(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Steps 1–2: every folder right below the root, grouped with the folders
/// that stand for the same thing. Returns the moves and how many folders
/// merge into another.
pub(super) fn merges(ctx: &Ctx<'_>) -> (Vec<Merge>, usize) {
    let root = &ctx.root;
    let other = root.join(&ctx.o.other_dir);
    let tops = ctx.dirs(root);
    // (normalized name, members, name for a new folder).
    let mut groups: Vec<(String, Vec<PathBuf>, Option<String>)> = Vec::new();
    let mut add = |key: String, p: PathBuf, name: Option<String>| match groups
        .iter_mut()
        .find(|g| g.0 == key)
    {
        Some(g) => {
            g.1.push(p);
            if g.2.is_none() {
                g.2 = name;
            }
        }
        None => groups.push((key, vec![p], name)),
    };
    for t in &tops {
        add(norm(&name_of(t)), t.clone(), None);
    }
    // Customer folders parked under 기타.
    let mut parked: Vec<PathBuf> = Vec::new();
    if other.is_dir() {
        for c in ctx.dirs(&other) {
            let n = name_of(&c);
            let key = norm(&n);
            if key == norm(&ctx.o.other_dir) {
                continue;
            }
            let top = tops.iter().find(|t| norm(&name_of(t)) == key);
            let known = ctx.customer(&n);
            if known.is_some() || top.is_some_and(|t| ctx.is_customer_dir(t)) {
                add(key, c.clone(), Some(known.unwrap_or(n)));
                parked.push(c);
            }
        }
    }
    let mut out: Vec<Merge> = Vec::new();
    let mut merged = 0;
    for (_, members, new_name) in &groups {
        let at_top: Vec<&PathBuf> = members
            .iter()
            .filter(|m| m.parent() == Some(root.as_path()))
            .collect();
        // The bigger folder stays (counting only when there is a choice).
        let pick = if at_top.len() > 1 {
            at_top
                .into_iter()
                .max_by_key(|m| (count_files(m).0, std::cmp::Reverse(m.as_os_str().len())))
        } else {
            at_top.into_iter().next()
        };
        let keep = match pick {
            Some(k) => k.clone(),
            None => match new_name {
                Some(n) => root.join(crate::curate::clean(n)),
                None => continue,
            },
        };
        let losers: Vec<&PathBuf> = members.iter().filter(|m| **m != keep).collect();
        merged += losers.len();
        if !keep.exists() && losers.len() == 1 {
            // One customer folder under 기타 and none at the top: it moves
            // up whole.
            out.push(Merge {
                from: losers[0].clone(),
                to: keep.clone(),
                is_dir: true,
                reason: format!(
                    "'{}' 아래에 있던 고객사 폴더 → 맨 위 '{}'",
                    ctx.o.other_dir,
                    rel(root, &keep)
                ),
            });
            continue;
        }
        let why = if losers.iter().all(|l| l.parent() == Some(other.as_path())) {
            format!("'{}' 아래에 있던 고객사 폴더", ctx.o.other_dir)
        } else {
            "이름만 다른 같은 폴더".to_string()
        };
        merged += merge_group(ctx, &keep, members, &why, &parked, &mut out);
    }
    (out, merged)
}

/// `p` after the folder moves in `dirs` (a file that lands in a folder
/// that later merges away ends up in the kept folder).
pub(super) fn remap(p: &Path, dirs: &[(PathBuf, PathBuf)]) -> PathBuf {
    let mut cur = p.to_path_buf();
    for _ in 0..4 {
        let Some((f, t)) = dirs.iter().find(|(f, _)| cur.starts_with(f) && cur != *f) else {
            break;
        };
        let rest = cur
            .strip_prefix(f)
            .map(Path::to_path_buf)
            .unwrap_or_default();
        cur = t.join(rest);
    }
    cur
}

/// Step 5 candidates: files whose name shows a customer but that sit in a
/// folder that is not a customer's (a kind folder, `기타/…`), put there by
/// Tidy one by one (not inside a folder that came whole).
fn misplaced(ctx: &Ctx<'_>, claimed: &[PathBuf]) -> Vec<PathBuf> {
    let Some(cur) = ctx.o.curator else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for top in ctx.dirs(&ctx.root) {
        if ctx.is_customer_dir(&top) {
            continue;
        }
        let walker = walkdir::WalkDir::new(&top)
            .min_depth(1)
            .max_depth(3)
            .sort_by_file_name()
            .into_iter()
            .filter_entry(|e| {
                !(e.file_type().is_dir()
                    && (ctx.left_alone(e.path())
                        || super::relocate::is_dev_project(e.path())
                        || ctx.is_customer_dir(e.path())))
            });
        for e in walker.filter_map(Result::ok) {
            if !e.file_type().is_file() || out.len() >= MAX_AI_FILES {
                continue;
            }
            let p = e.path();
            if ctx.left_alone(p) || claimed.iter().any(|c| p.starts_with(c)) {
                continue;
            }
            let name = e.file_name().to_string_lossy().into_owned();
            if cur.customer_hint(&name, Some(ctx.store)).is_none() {
                continue;
            }
            let Ok(Some(m)) = ctx.store.active_move_to(p) else {
                continue; // the user put it there
            };
            if m.src_path.parent().and_then(Path::file_name) == p.parent().and_then(Path::file_name)
            {
                continue; // part of a folder that moved whole
            }
            out.push(p.to_path_buf());
        }
    }
    out
}

/// Look inside `root` (an organize root) and propose re-sorting it.
/// Nothing moves; save the plan with [`super::restructure::save`] and apply
/// it with [`super::restructure::apply`].
pub fn review(root: &Path, store: &Store, o: &ReviewOptions<'_>) -> Result<Review, PlanError> {
    let root =
        dunce::canonicalize(root).map_err(|_| ScanError::NotADirectory(root.to_path_buf()))?;
    let customers = o
        .curator
        .map(|c| customer_names(c.dict(), Some(store)))
        .unwrap_or_default();
    let ctx = Ctx {
        root: root.clone(),
        o,
        store,
        customers,
    };

    // 1–2. Merges.
    let (merges, merged) = merges(&ctx);
    let dir_moves: Vec<(PathBuf, PathBuf)> = merges
        .iter()
        .filter(|m| m.is_dir)
        .map(|m| (m.from.clone(), m.to.clone()))
        .collect();
    let claimed: Vec<PathBuf> = merges.iter().map(|m| m.from.clone()).collect();
    let targets: Vec<PathBuf> = merges.iter().map(|m| m.to.clone()).collect();

    // 3. Small folders (not those that are about to change).
    let mut folded_dirs: Vec<PathBuf> = Vec::new();
    let compact: Vec<PlannedMove> = super::compact::plan_compact(&root, o.min_items, store)
        .map(|p| p.moves)
        .unwrap_or_default()
        .into_iter()
        .filter(|m| {
            let leaf = m.from.parent().unwrap_or(&m.from);
            !claimed
                .iter()
                .any(|c| m.from.starts_with(c) || c.starts_with(leaf))
                && !targets
                    .iter()
                    .any(|t| m.from.starts_with(t) || t.starts_with(leaf))
                && !ctx.left_alone(&m.from)
                && !o.skip.iter().any(|s| m.from.starts_with(s))
        })
        .inspect(|m| {
            if let Some(l) = m.from.parent() {
                if !folded_dirs.iter().any(|d| d == l) {
                    folded_dirs.push(l.to_path_buf());
                }
            }
        })
        .collect();

    // 4. Loose files at the top, by the rules.
    let mut by_rules: Vec<PlannedMove> = Vec::new();
    if let (Some(rules), Some(filter)) = (o.rules, o.filter) {
        if let Ok(p) = plan_from_rules_in(&root, rules, false, filter, &o.protected, Some(&root)) {
            by_rules = p
                .moves
                .into_iter()
                .filter(|m| m.from != m.to && m.from.parent() == Some(root.as_path()))
                .collect();
        }
    }
    let loose: Vec<PathBuf> = std::fs::read_dir(&root)
        .map(|rd| {
            let mut v: Vec<PathBuf> = rd
                .filter_map(Result::ok)
                .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
                .map(|e| e.path())
                .filter(|p| !ctx.left_alone(p) && !by_rules.iter().any(|m| m.from == *p))
                .collect();
            v.sort();
            v
        })
        .unwrap_or_default();

    // 5. Files the AI could place.
    let mut cands = misplaced(&ctx, &claimed);
    let hinted: Vec<PathBuf> = cands.clone();
    cands.extend(
        loose
            .into_iter()
            .take(MAX_AI_FILES.saturating_sub(cands.len())),
    );
    let (mut by_ai, mut ai_error) = (Vec::new(), None);
    if let (Some(ai), Some(cur)) = (o.ai, o.curator) {
        if !cands.is_empty() {
            match cur.place_files(ai, store, &cands, &root) {
                Ok(m) => by_ai = m,
                Err(e) => ai_error = Some(e.to_string()),
            }
        }
    }
    // (0.16.0) A customer's file the AI did not place (no AI, AI down or
    // unsure) still goes to that customer's folder, by kind — offline.
    let mut by_customer: Vec<PlannedMove> = Vec::new();
    if let Some(cur) = o.curator {
        for f in hinted
            .iter()
            .filter(|f| !by_ai.iter().any(|m| m.from == **f))
        {
            let name = name_of(f);
            let Some(c) = cur.customer_hint(&name, Some(store)) else {
                continue;
            };
            let kind = crate::curate::kinds::kind_of(Some(cur.dict()), f);
            let (dir, _) = crate::curate::kinds::customer_kind_dir(&root, &c, &kind);
            by_customer.push(PlannedMove {
                id: 0,
                to: dir.join(&name),
                from: f.clone(),
                is_dir: false,
                files: 1,
                bytes: std::fs::metadata(f).map(|m| m.len()).unwrap_or(0),
                reason: format!("이름에 고객사 '{c}'가 보임 → '{}'", rel(&root, &dir)),
                warnings: Vec::new(),
                context: None,
                encrypt: false,
            });
        }
    }
    let asked = o.ai.is_some() && o.curator.is_some() && ai_error.is_none();
    let ai_waiting = if asked {
        0
    } else {
        cands
            .iter()
            .filter(|f| {
                !by_ai.iter().any(|m| m.from == **f) && !by_customer.iter().any(|m| m.from == **f)
            })
            .count()
    };

    // One plan: files first (their folders remapped through the merges),
    // then the merges, then the folding.
    let mut moves: Vec<PlannedMove> = Vec::new();
    let mut push = |mut m: PlannedMove, label: &str| {
        m.id = moves.len() as u32 + 1;
        m.to = remap(&m.to, &dir_moves);
        if !label.is_empty() {
            m.reason = if m.reason.is_empty() {
                label.to_string()
            } else {
                format!("{label}: {}", m.reason)
            };
        }
        if !m.is_dir && m.to.exists() && m.to != m.from {
            m.warnings.push(
                "같은 이름 파일이 있어 '(1)'처럼 번호가 붙습니다(내용이 같으면 그대로 둠)".into(),
            );
        }
        moves.push(m);
    };
    let n_ai = by_ai.len();
    for m in by_ai {
        push(m, "제자리 아닌 파일(AI)");
    }
    let n_cust = by_customer.len();
    for m in by_customer {
        push(m, "고객사 폴더 밖 고객사 파일");
    }
    let n_rules = by_rules.len();
    for m in by_rules {
        push(m, "맨 위 파일(규칙)");
    }
    for g in merges {
        let (files, bytes) = if g.is_dir {
            count_files(&g.from)
        } else {
            (1, std::fs::metadata(&g.from).map(|m| m.len()).unwrap_or(0))
        };
        let mut warnings = Vec::new();
        if g.is_dir && g.to.exists() {
            warnings.push(format!(
                "'{}' 폴더와 합칩니다 — 같은 이름 파일은 번호가 붙습니다",
                rel(&root, &g.to)
            ));
        }
        push(
            PlannedMove {
                id: 0,
                from: g.from,
                to: g.to,
                is_dir: g.is_dir,
                files,
                bytes,
                reason: g.reason,
                warnings,
                context: None,
                encrypt: false,
            },
            "",
        );
    }
    let n_fold = folded_dirs.len();
    for m in compact {
        push(m, "작은 폴더 접기");
    }

    let mut parts: Vec<String> = Vec::new();
    if merged > 0 {
        parts.push(format!(
            "같은 고객사·이슈인데 나뉜 폴더 {merged}곳을 합칩니다"
        ));
    }
    if n_fold > 0 {
        parts.push(format!(
            "파일이 {}개보다 적은 작은 폴더 {n_fold}곳을 한 단계 위로 접습니다",
            o.min_items
        ));
    }
    if n_rules > 0 {
        parts.push(format!(
            "맨 위에 흩어진 파일 {n_rules}개를 규칙대로 넣습니다"
        ));
    }
    if n_cust > 0 {
        parts.push(format!(
            "고객사 폴더 밖에 있던 고객사 파일 {n_cust}개를 그 고객사 폴더의 종류별 폴더로 옮깁니다"
        ));
    }
    if n_ai > 0 {
        parts.push(format!(
            "고객사 폴더 밖에 있던 파일 {n_ai}개를 AI가 고른 고객사·이슈 폴더로 옮깁니다"
        ));
    }
    let mut summary = if parts.is_empty() {
        "정리 위치를 살펴봤습니다. 합칠 폴더·접을 폴더·옮길 파일이 없습니다.".to_string()
    } else {
        format!(
            "정리 위치 점검: {}. 폴더는 통째로 옮기고(안은 그대로), 비게 된 폴더는 지웁니다. 되돌리기 탭에서 한 번에 되돌릴 수 있습니다.",
            parts.join(", ")
        )
    };
    if ai_waiting > 0 {
        summary.push_str(&format!(
            " 맨 위에 흩어진 파일 {ai_waiting}개는 {}",
            if ai_error.is_some() {
                "AI가 답하지 못해 그대로 둡니다."
            } else {
                "AI로 옮길 곳을 정할 수 있습니다(\"AI로 이것도 보기\")."
            }
        ));
    }
    let plan = (!moves.is_empty()).then(|| Plan {
        id: 0,
        root: root.clone(),
        source: PlanSource::Review,
        created_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0),
        summary: summary.clone(),
        folders: Vec::new(),
        moves,
        rejected: Vec::new(),
        applied_batch: None,
        anchor: Some(root.clone()),
        sources: Vec::new(),
    });
    Ok(Review {
        plan,
        summary,
        merged,
        folded: n_fold,
        by_rules: n_rules,
        by_ai: n_ai,
        to_customer: n_cust,
        ai_waiting,
        ai_error,
    })
}

/// The organize roots ("지정 위치로 모으기" targets) — what a review looks
/// at. Watch folders organized in place are not: their folders are the
/// user's own.
pub fn review_roots(cfg: &crate::config::Config) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for w in &cfg.watches {
        let a = w.anchor();
        if a != w.path.as_path() && !out.iter().any(|x| x == a) {
            out.push(a.to_path_buf());
        }
    }
    out
}

/// Options for `root` from the configuration: rules, ignore filter and
/// dictionary loaded here (each optional — a broken one only turns its
/// step off). The AI is set by the caller, when asked.
pub fn run(
    cfg: &crate::config::Config,
    root: &Path,
    store: &Store,
    protected: Vec<PathBuf>,
    ai: Option<&AiClassifier>,
) -> Result<Review, PlanError> {
    let rules = RuleSet::load(&cfg.classify.rules).ok();
    let filter = cfg.ignore_filter().ok();
    let curator = Curator::from_config(cfg).ok();
    let mut o = ReviewOptions::base(cfg, root, protected);
    o.rules = rules.as_ref();
    o.filter = filter.as_ref();
    o.curator = curator.as_ref();
    o.ai = ai;
    review(root, store, &o)
}
