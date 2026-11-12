//! (0.16.0, SPEC §14.22) "정리 위치 새로 짜기": for an organize root that
//! grew under several older layouts, look at every file and folder again
//! and propose where each belongs under today's layout (§14.19) — offline,
//! as one ordinary plan (preview, tick, apply as one undoable batch).
//!
//! What it adds to [`super::review`]'s merges:
//! - **고객사 데이터는 고객사 폴더로** — any file that belongs to a customer
//!   (its name shows the customer, or its indexed text names exactly one
//!   customer) goes to `{고객사}/{종류}/` (`가람정보/문서/견적서.pdf`),
//!   wherever it was (`문서/2026/`, `기타/…`, the top). A folder whose name
//!   shows a customer moves whole under that customer.
//! - **고객사 폴더 바로 아래 파일** → `{고객사}/{종류}/`.
//! - **나머지 파일** go where the rules say (when a rule matches).
//!
//! What stays: issue folders' inside, folders that are not Tidy's layout
//! (moved whole only when they show a customer), development projects,
//! hidden / protected / watch folders. No AI here: nothing blocks.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use super::restructure::{Plan, PlanError, PlanSource, PlannedMove};
use super::review::{count_files, merges, name_of, rel, remap, Ctx, ReviewOptions};
use super::scan::ScanError;
use crate::classify::FileFacts;
use crate::curate::existing::{norm, reuse_existing, split_dated};
use crate::curate::kinds::{is_kind_name, kind_of};
use crate::curate::{clean, customer_names};
use crate::store::Store;

/// Most moves in one plan (the rest is left for the next run).
const MAX_MOVES: usize = 20_000;
/// How deep the layout is walked below the root.
const MAX_DEPTH: usize = 6;
/// Characters of indexed text looked at for customer names.
const BODY_CHARS: usize = 20_000;

/// The result: a plan (when anything should move) and what was found.
#[derive(Debug, Clone, Serialize)]
pub struct Rebuild {
    pub plan: Option<Plan>,
    pub summary: String,
    /// Folders merged into another (same customer / issue).
    pub merged: usize,
    /// Files and folders that go into a customer's folder.
    pub to_customer: usize,
    /// Files the rules place.
    pub by_rules: usize,
    /// Customers that get a new folder.
    pub new_customers: Vec<String>,
    /// More than [`MAX_MOVES`]: run again after applying.
    pub capped: bool,
}

/// A customer and where its folder is (or will be).
struct Customer {
    name: String,
    /// Spellings that point to it (name, aliases, folder names), normalized.
    keys: Vec<String>,
    dir: PathBuf,
}

fn usable(k: &str) -> bool {
    let n = k.chars().count();
    n >= 3 || (n == 2 && !k.is_ascii())
}

/// A year / month layout folder (`2026`, `2026-11`, `202611`).
fn is_period(name: &str) -> bool {
    let digits: String = name.chars().filter(char::is_ascii_digit).collect();
    let rest = name
        .chars()
        .all(|c| c.is_ascii_digit() || c == '-' || c == '_' || c == '.');
    rest && (4..=8).contains(&digits.len())
}

struct Builder<'a> {
    ctx: Ctx<'a>,
    customers: Vec<Customer>,
}

impl Builder<'_> {
    /// The customer `text` mentions (longest spelling wins).
    fn in_text(&self, text: &str) -> Option<usize> {
        let hay = norm(text);
        let mut best: Option<(usize, usize)> = None;
        for (i, c) in self.customers.iter().enumerate() {
            for k in &c.keys {
                if usable(k) && hay.contains(k.as_str()) && best.is_none_or(|(_, l)| k.len() > l) {
                    best = Some((i, k.len()));
                }
            }
        }
        best.map(|(i, _)| i)
    }

    /// The customer a file's indexed text names — only when exactly one.
    fn in_body(&self, p: &Path) -> Option<usize> {
        let rec = self.ctx.store.get_file(p).ok()??;
        let text = self.ctx.store.file_content(rec.id).ok()??;
        let head: String = text.chars().take(BODY_CHARS).collect();
        let hay = norm(&head);
        let mut found: Option<usize> = None;
        for (i, c) in self.customers.iter().enumerate() {
            if c.keys.iter().any(|k| usable(k) && hay.contains(k.as_str())) {
                if found.is_some() {
                    return None;
                }
                found = Some(i);
            }
        }
        found
    }

    fn kind_dir(&self, cust: &Path, file: &Path) -> PathBuf {
        let dict = self.ctx.o.curator.map(|c| c.dict());
        reuse_existing(&cust.join(clean(&kind_of(dict, file))), &self.ctx.root, 0)
    }

    /// A layout folder's name: a kind, a period, `날짜_이슈`, `기타`.
    fn layout_name(&self, name: &str) -> bool {
        let dict = self.ctx.o.curator.map(|c| c.dict());
        is_period(name)
            || split_dated(name).is_some()
            || is_kind_name(dict, name)
            || norm(name) == norm(&self.ctx.o.other_dir)
    }

    /// A folder of Tidy's layout below a non-customer top folder.
    fn is_layout(&self, dir: &Path, depth: usize) -> bool {
        let name = name_of(dir);
        let dict = self.ctx.o.curator.map(|c| c.dict());
        depth == 1
            || is_period(&name)
            || split_dated(&name).is_some()
            || is_kind_name(dict, &name)
            || (depth == 2
                && dir
                    .parent()
                    .map(name_of)
                    .is_some_and(|p| norm(&p) == norm(&self.ctx.o.other_dir)))
    }
}

/// Look at everything inside `root` and propose the layout it would have
/// if it were organized today. Nothing moves.
pub fn rebuild(root: &Path, store: &Store, o: &ReviewOptions<'_>) -> Result<Rebuild, PlanError> {
    let root =
        dunce::canonicalize(root).map_err(|_| ScanError::NotADirectory(root.to_path_buf()))?;
    let names = o
        .curator
        .map(|c| customer_names(c.dict(), Some(store)))
        .unwrap_or_default();
    let ctx = Ctx {
        root: root.clone(),
        o,
        store,
        customers: names.clone(),
    };

    // 1. Same customer / issue split into several folders: merged first.
    let (merges, merged) = merges(&ctx);
    let dir_moves: Vec<(PathBuf, PathBuf)> = merges
        .iter()
        .filter(|m| m.is_dir)
        .map(|m| (m.from.clone(), m.to.clone()))
        .collect();
    let claimed: Vec<PathBuf> = merges.iter().map(|m| m.from.clone()).collect();
    // Folders whose contents the merges move out piece by piece (a folder
    // that moves whole is in `claimed` itself).
    let mut members: Vec<PathBuf> = Vec::new();
    for m in &merges {
        if m.to.parent() != Some(root.as_path()) {
            if let Some(p) = m.from.parent() {
                if !members.iter().any(|x| x == p) {
                    members.push(p.to_path_buf());
                }
            }
        }
    }

    // 2. The customers and their folders after the merges.
    let other_key = norm(&o.other_dir);
    let mut customers: Vec<Customer> = Vec::new();
    let mut add = |name: String, dir: Option<PathBuf>, extra: Vec<String>| {
        let k = norm(&name);
        if k.is_empty() || k == other_key {
            return;
        }
        let i = match customers.iter().position(|c| c.keys.contains(&k)) {
            Some(i) => i,
            None => {
                customers.push(Customer {
                    dir: root.join(clean(&name)),
                    name,
                    keys: vec![k],
                });
                customers.len() - 1
            }
        };
        let c = &mut customers[i];
        if let Some(d) = dir {
            c.dir = d;
        }
        for e in extra {
            let e = norm(&e);
            if !e.is_empty() && !c.keys.contains(&e) {
                c.keys.push(e);
            }
        }
    };
    if let Some(cur) = o.curator {
        for e in &cur.dict().entities {
            add(e.name.clone(), None, e.aliases.clone());
        }
    }
    for n in &names {
        add(n.clone(), None, Vec::new());
    }
    let mut tops: Vec<PathBuf> = ctx
        .dirs(&root)
        .into_iter()
        .filter(|t| !claimed.contains(t))
        .collect();
    for m in &merges {
        if m.is_dir && m.to.parent() == Some(root.as_path()) && !tops.contains(&m.to) {
            tops.push(m.to.clone());
        }
    }
    let mut cust_tops: Vec<PathBuf> = Vec::new();
    for t in &tops {
        let n = name_of(t);
        if norm(&n) == other_key {
            continue;
        }
        let known = ctx.customer(&n);
        let by_layout = t.is_dir() && ctx.is_customer_dir(t);
        let parked = !t.exists(); // a customer folder coming up from 기타
        if known.is_some() || by_layout || parked {
            add(known.unwrap_or_else(|| n.clone()), Some(t.clone()), vec![n]);
            cust_tops.push(t.clone());
        }
    }
    let b = Builder { ctx, customers };

    let mut moves: Vec<PlannedMove> = Vec::new();
    let mut to_customer = 0usize;
    let mut by_rules = 0usize;
    let mut capped = false;
    let mut new_customers: Vec<String> = Vec::new();
    let mut push =
        |moves: &mut Vec<PlannedMove>, from: PathBuf, to: PathBuf, is_dir: bool, reason: String| {
            if moves.len() >= MAX_MOVES {
                capped = true;
                return false;
            }
            let to = remap(&to, &dir_moves);
            if to == from || moves.iter().any(|m| m.from == from) {
                return false;
            }
            let (files, bytes) = if is_dir {
                count_files(&from)
            } else {
                (1, std::fs::metadata(&from).map(|m| m.len()).unwrap_or(0))
            };
            let mut warnings = Vec::new();
            if to.exists() {
                warnings.push(if is_dir {
                    format!(
                        "'{}' 폴더와 합칩니다 — 같은 이름 파일은 번호가 붙습니다",
                        rel(&root, &to)
                    )
                } else {
                    "같은 이름 파일이 있어 '(1)'처럼 번호가 붙습니다(내용이 같으면 그대로 둠)"
                        .into()
                });
            }
            moves.push(PlannedMove {
                id: moves.len() as u32 + 1,
                from,
                to,
                is_dir,
                files,
                bytes,
                reason,
                warnings,
                context: None,
                encrypt: false,
            });
            true
        };
    let mut note_new = |c: &Customer| {
        if !remap(&c.dir, &dir_moves).exists()
            && !dir_moves.iter().any(|(_, t)| *t == c.dir)
            && !new_customers.contains(&c.name)
        {
            new_customers.push(c.name.clone());
        }
    };

    // 3. Files right inside a customer folder → its kind folder.
    for c in &b.customers {
        if !c.dir.is_dir() {
            continue;
        }
        let Ok(rd) = std::fs::read_dir(&c.dir) else {
            continue;
        };
        let mut files: Vec<PathBuf> = rd
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
            .map(|e| e.path())
            .filter(|p| !b.ctx.left_alone(p))
            .collect();
        files.sort();
        for f in files {
            let to = b.kind_dir(&c.dir, &f).join(name_of(&f));
            let why = format!(
                "고객사 폴더 바로 아래 파일 → 종류별로 '{}'",
                rel(&root, to.parent().unwrap_or(&to))
            );
            if push(&mut moves, f, to, false, why) {
                to_customer += 1;
            }
        }
    }

    // 4. Everything outside the customer folders.
    let rules = o.rules;
    // (dir, depth, rules apply here): the root and kind / period folders —
    // never 기타's topic folders or a folder of the user's own.
    let dict = o.curator.map(|c| c.dict());
    let mut walk: Vec<(PathBuf, usize, bool)> = vec![(root.clone(), 0, true)];
    while let Some((dir, depth, zone)) = walk.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut entries: Vec<_> = rd.filter_map(Result::ok).collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let p = e.path();
            if b.ctx.left_alone(&p) || claimed.iter().any(|c| p.starts_with(c)) {
                continue;
            }
            let Ok(t) = e.file_type() else { continue };
            let name = e.file_name().to_string_lossy().into_owned();
            if t.is_dir() {
                // A customer folder, or one the merges empty (기타/고객사).
                if (depth == 0 && cust_tops.contains(&p)) || members.contains(&p) {
                    continue;
                }
                if super::relocate::is_dev_project(&p) {
                    continue;
                }
                // A folder whose name shows a customer (and is not a
                // layout name like `문서`, `2026`) goes whole to it.
                let named = b.in_text(&name).filter(|_| !b.layout_name(&name));
                if named.is_none() && b.is_layout(&p, depth + 1) && depth + 1 < MAX_DEPTH {
                    let z = if depth == 0 {
                        is_kind_name(dict, &name) || is_period(&name)
                    } else {
                        zone
                    };
                    walk.push((p, depth + 1, z));
                    continue;
                }
                // Someone's own folder: moved whole, only for a customer.
                if let Some(i) = named {
                    let c = &b.customers[i];
                    note_new(c);
                    let to = c.dir.join(&name);
                    let why = format!(
                        "이름에 고객사 '{}'가 보이는 폴더 → 통째로 '{}'",
                        c.name,
                        rel(&root, &c.dir)
                    );
                    if push(&mut moves, p, to, true, why) {
                        to_customer += 1;
                    }
                }
                continue;
            }
            if !t.is_file() {
                continue;
            }
            let found = b
                .in_text(&name)
                .map(|i| (i, "이름"))
                .or_else(|| b.in_body(&p).map(|i| (i, "내용")));
            if let Some((i, by)) = found {
                let c = &b.customers[i];
                note_new(c);
                let to = b.kind_dir(&c.dir, &p).join(&name);
                let why = format!(
                    "{by}에 고객사 '{}'가 보임 → '{}'",
                    c.name,
                    rel(&root, to.parent().unwrap_or(&to))
                );
                if push(&mut moves, p, to, false, why) {
                    to_customer += 1;
                }
                continue;
            }
            let Some(rules) = rules.filter(|_| zone) else {
                continue;
            };
            let Ok(facts) = FileFacts::from_path(&p) else {
                continue;
            };
            if let Some(cl) = rules.classify_in(&facts, Some(&root)) {
                if !cl.dest_dir.starts_with(&root) || cl.dest_dir == dir {
                    continue;
                }
                let to = cl.dest_dir.join(&name);
                let why = format!("규칙 '{}' → '{}'", cl.rule_name, rel(&root, &cl.dest_dir));
                if push(&mut moves, p, to, false, why) {
                    by_rules += 1;
                }
            }
        }
    }

    // 5. The merges last (files above may land in folders they rename).
    for g in merges {
        let to = if !g.is_dir
            && b.customers
                .iter()
                .any(|c| g.to.parent() == Some(c.dir.as_path()))
        {
            let cust = g.to.parent().unwrap_or(&root).to_path_buf();
            b.kind_dir(&cust, &g.from).join(name_of(&g.from))
        } else {
            g.to
        };
        // Through `push` but without its remap of this very move.
        if moves.len() >= MAX_MOVES {
            capped = true;
            break;
        }
        let (files, bytes) = if g.is_dir {
            count_files(&g.from)
        } else {
            (1, std::fs::metadata(&g.from).map(|m| m.len()).unwrap_or(0))
        };
        let mut warnings = Vec::new();
        if g.is_dir && to.exists() {
            warnings.push(format!(
                "'{}' 폴더와 합칩니다 — 같은 이름 파일은 번호가 붙습니다",
                rel(&root, &to)
            ));
        }
        moves.push(PlannedMove {
            id: moves.len() as u32 + 1,
            from: g.from,
            to,
            is_dir: g.is_dir,
            files,
            bytes,
            reason: g.reason,
            warnings,
            context: None,
            encrypt: false,
        });
    }

    let mut parts: Vec<String> = Vec::new();
    if to_customer > 0 {
        parts.push(format!(
            "고객사 자료 {to_customer}개를 그 고객사 폴더의 종류별 폴더로 모읍니다"
        ));
    }
    if merged > 0 {
        parts.push(format!(
            "같은 고객사·이슈인데 나뉜 폴더 {merged}곳을 합칩니다"
        ));
    }
    if by_rules > 0 {
        parts.push(format!("다른 파일 {by_rules}개를 규칙대로 넣습니다"));
    }
    let mut summary = if parts.is_empty() {
        "정리 위치를 처음부터 다시 살펴봤습니다. 지금 방식과 다른 곳에 있는 파일이 없습니다."
            .to_string()
    } else {
        format!(
            "정리 위치 새로 짜기: {}. 고객사 폴더는 '고객사/이슈' 또는 '고객사/종류', 이슈 폴더 안과 직접 만든 폴더 안은 그대로 둡니다. 비게 된 폴더는 지우고, 되돌리기 탭에서 한 번에 되돌릴 수 있습니다.",
            parts.join(", ")
        )
    };
    if !new_customers.is_empty() {
        summary.push_str(&format!(" 새 고객사 폴더: {}.", new_customers.join(", ")));
    }
    if capped {
        summary.push_str(&format!(
            " 한 번에 {MAX_MOVES}개까지만 담았습니다 — 적용한 뒤 다시 실행하면 나머지가 나옵니다."
        ));
    }
    let plan = (!moves.is_empty()).then(|| Plan {
        id: 0,
        root: root.clone(),
        source: PlanSource::Rebuild,
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
    Ok(Rebuild {
        plan,
        summary,
        merged,
        to_customer,
        by_rules,
        new_customers,
        capped,
    })
}

/// [`rebuild`] with rules, ignore filter and dictionary from the
/// configuration (each optional).
pub fn run(
    cfg: &crate::config::Config,
    root: &Path,
    store: &Store,
    protected: Vec<PathBuf>,
) -> Result<Rebuild, PlanError> {
    let rules = crate::classify::RuleSet::load(&cfg.classify.rules).ok();
    let filter = cfg.ignore_filter().ok();
    let curator = crate::curate::Curator::from_config(cfg).ok();
    let mut o = ReviewOptions::base(cfg, root, protected);
    o.rules = rules.as_ref();
    o.filter = filter.as_ref();
    o.curator = curator.as_ref();
    rebuild(root, store, &o)
}
