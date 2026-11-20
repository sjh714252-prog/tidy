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
use crate::curate::group;
use crate::curate::kinds::{is_kind_name, kind_of};
use crate::curate::sense::{self, Clues, Shape};
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
    /// (0.18.0) Files placed by what their names / contents say (log
    /// bundles, manuals, runtimes).
    pub by_name: usize,
    /// (0.18.0) Logs no customer could be found for: left where they are,
    /// to be asked about in 확인 대기 ([`run`] queues them).
    pub ask_logs: Vec<(PathBuf, PathBuf)>,
    /// (0.20.0, §14.30) Archives grouped by company / topic (name, the
    /// folder inside, the documents inside).
    pub by_group: usize,
    /// (0.20.0) The groups made: (folder under the root, archives).
    pub groups: Vec<(String, usize)>,
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
    /// (0.18.0) For [`sense::shape`] (None: `name_sense` off).
    clues: Option<Clues>,
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
        self.only_in(&text)
    }

    /// The customer `text` names — only when exactly one.
    fn only_in(&self, text: &str) -> Option<usize> {
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

    /// (0.18.0) What `p` is, by name / contents; `cust` the customer found.
    fn shape(&self, p: &Path, cust: Option<usize>, entries: &[String]) -> Option<Shape> {
        let clues = self.clues.as_ref()?;
        let name = cust.map(|i| self.customers[i].name.clone());
        sense::shape(p, clues, name.as_deref(), Some(entries))
    }

    /// (0.18.0) The customer a zip's file list names (exactly one).
    fn in_zip(&self, entries: &[String]) -> Option<usize> {
        let mut found: Option<usize> = None;
        for e in entries {
            if let Some(i) = self.in_text(e) {
                if found.is_some_and(|f| f != i) {
                    return None;
                }
                found = Some(i);
            }
        }
        found
    }

    /// Where a customer's file goes: a log bundle to its day folder, a
    /// manual to `매뉴얼`, the rest to its kind folder.
    fn customer_dest(&self, cust: &Path, file: &Path, shape: Option<&Shape>) -> PathBuf {
        let root = &self.ctx.root;
        match shape {
            Some(Shape::Log { date, label, .. }) => {
                reuse_existing(&cust.join(sense::LOG_KIND), root, 0)
                    .join(Shape::log_folder(*date, label))
            }
            Some(Shape::Manual { .. }) => reuse_existing(&cust.join(sense::MANUAL_KIND), root, 0),
            _ => self.kind_dir(cust, file),
        }
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
        // (0.20.0) Also a folder holding `로그` / `매뉴얼` / `압축파일`
        // (what log bundles and archive groups make, §14.24·§14.30).
        let by_layout = t.is_dir()
            && (ctx.is_customer_dir(t)
                || (o.sense && !sense::layout_top(&n) && sense::customer_like(t)));
        let parked = !t.exists(); // a customer folder coming up from 기타
        if known.is_some() || by_layout || parked {
            add(known.unwrap_or_else(|| n.clone()), Some(t.clone()), vec![n]);
            cust_tops.push(t.clone());
        }
    }
    let clues = o.sense.then(|| {
        let mut c = Clues::from_dict(o.curator.map(|c| c.dict()), &[]);
        for cu in &customers {
            c.add_customer(&cu.name, cu.keys.clone());
        }
        c.add_service_dirs(&root);
        c
    });
    let b = Builder {
        ctx,
        customers,
        clues,
    };
    let mut by_name = 0usize;
    let mut ask_logs: Vec<(PathBuf, PathBuf)> = Vec::new();

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
        let entries: Vec<_> = rd.filter_map(Result::ok).collect();
        let mut files: Vec<PathBuf> = entries
            .iter()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
            .map(|e| e.path())
            .filter(|p| !b.ctx.left_alone(p))
            .collect();
        files.sort();
        let ci = b.customers.iter().position(|x| x.dir == c.dir);
        for f in files {
            let shape = b.shape(&f, ci, &sense::zip_entries(&f, 2000));
            let to = b
                .customer_dest(&c.dir, &f, shape.as_ref())
                .join(name_of(&f));
            let why = match &shape {
                Some(s @ (Shape::Log { .. } | Shape::Manual { .. })) => format!(
                    "고객사 폴더 바로 아래 {} → '{}'",
                    s.describe(),
                    rel(&root, to.parent().unwrap_or(&to))
                ),
                _ => format!(
                    "고객사 폴더 바로 아래 파일 → 종류별로 '{}'",
                    rel(&root, to.parent().unwrap_or(&to))
                ),
            };
            if push(&mut moves, f, to, false, why) {
                to_customer += 1;
            }
        }
        // (0.18.0) Log bundles and manuals lying loose in the customer's
        // kind folders (`압축파일/`, `로그/`, `텍스트/`) → their own folders.
        if b.clues.is_some() {
            let dict = b.ctx.o.curator.map(|c| c.dict());
            let mut kinds: Vec<PathBuf> = entries
                .iter()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .map(|e| e.path())
                .filter(|d| is_kind_name(dict, &name_of(d)) && !b.ctx.left_alone(d))
                .collect();
            kinds.sort();
            for k in kinds {
                let Ok(rd) = std::fs::read_dir(&k) else {
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
                    let Some(shape) = b.shape(&f, ci, &sense::zip_entries(&f, 2000)) else {
                        continue;
                    };
                    if !matches!(shape, Shape::Log { .. } | Shape::Manual { .. }) {
                        continue;
                    }
                    let dir = b.customer_dest(&c.dir, &f, Some(&shape));
                    if dir == k {
                        continue;
                    }
                    let why = format!("{} → '{}'", shape.describe(), rel(&root, &dir));
                    if push(&mut moves, f.clone(), dir.join(name_of(&f)), false, why) {
                        by_name += 1;
                    }
                }
            }
        }
    }

    // 4. Everything outside the customer folders.
    let rules = o.rules;
    // Where the rules put a file found in `dir` (`None`: no rule, or it is
    // already in that rule's tree).
    let rule_dest = |p: &Path, dir: &Path| -> Option<(PathBuf, String)> {
        let rules = rules?;
        let facts = FileFacts::from_path(p).ok()?;
        let cl = rules.classify_in(&facts, Some(&root))?;
        if !cl.dest_dir.starts_with(&root) || cl.dest_dir == dir {
            return None;
        }
        // (0.16.1) Already in the rule's own tree (`텍스트/2026` for a
        // `텍스트/{yyyy}/{mm}` rule): left where it is — the rules only
        // take files from elsewhere, so a file never churns between period
        // folders (or to `1980/01` for a bad modified date).
        let top = |p: &Path| {
            p.strip_prefix(&root)
                .ok()
                .and_then(|r| r.components().next())
                .map(|c| norm(&c.as_os_str().to_string_lossy()))
        };
        if top(dir).is_some() && top(dir) == top(&cl.dest_dir) {
            return None;
        }
        let why = format!("규칙 '{}' → '{}'", cl.rule_name, rel(&root, &cl.dest_dir));
        Some((cl.dest_dir.join(name_of(p)), why))
    };
    struct Loose {
        item: group::Item,
        path: PathBuf,
        dir: PathBuf,
        entries: Vec<String>,
    }
    let mut loose: Vec<Loose> = Vec::new();
    let mut by_group = 0usize;
    let mut groups: Vec<(String, usize)> = Vec::new();
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
            let entries = if b.clues.is_some() {
                sense::zip_entries(&p, 2000)
            } else {
                Vec::new()
            };
            let found = b
                .in_text(&name)
                .map(|i| (i, "이름"))
                .or_else(|| b.in_body(&p).map(|i| (i, "내용")))
                .or_else(|| b.in_zip(&entries).map(|i| (i, "압축 파일 안 이름")))
                .or_else(|| {
                    // (0.20.0, §14.30) The documents inside the zip.
                    (b.clues.is_some() && group::is_archive(&p))
                        .then(|| b.only_in(&sense::zip_text(&p)))
                        .flatten()
                        .map(|i| (i, "압축 파일 안 문서"))
                });
            let shape = b.shape(&p, found.map(|(i, _)| i), &entries);
            if let Some((i, by)) = found {
                let c = &b.customers[i];
                note_new(c);
                let to = b.customer_dest(&c.dir, &p, shape.as_ref()).join(&name);
                let what = match &shape {
                    Some(s @ (Shape::Log { .. } | Shape::Manual { .. })) => {
                        format!(" · {}", s.describe())
                    }
                    _ => String::new(),
                };
                let why = format!(
                    "{by}에 고객사 '{}'가 보임{what} → '{}'",
                    c.name,
                    rel(&root, to.parent().unwrap_or(&to))
                );
                if push(&mut moves, p, to, false, why) {
                    to_customer += 1;
                }
                continue;
            }
            // (0.18.0) No customer: manuals → 서비스, runtimes → 설치파일,
            // logs that say so → asked about (not left to the type rules).
            // (0.18.3) Runtimes also from the AI's topic folders under
            // `기타` (`기타/개발/amazon-corretto-8-x64-linux-jdk.zip`).
            let in_other = dir
                .strip_prefix(&root)
                .ok()
                .and_then(|r| r.components().next())
                .is_some_and(|c| norm(&c.as_os_str().to_string_lossy()) == norm(&o.other_dir));
            if !zone && in_other && matches!(shape, Some(Shape::Runtime { .. })) {
                if let Some(s) = &shape {
                    if let Some((d, _)) = sense::dest(&root, s, None) {
                        if d != dir {
                            let why =
                                format!("이름으로 보면 {} → '{}'", s.describe(), rel(&root, &d));
                            if push(&mut moves, p, d.join(&name), false, why) {
                                by_name += 1;
                            }
                        }
                    }
                }
                continue;
            }
            if zone {
                match &shape {
                    Some(s @ (Shape::Manual { .. } | Shape::Runtime { .. })) => {
                        if let Some((d, _)) = sense::dest(&root, s, None) {
                            if d != dir {
                                let why = format!(
                                    "이름으로 보면 {} → '{}'",
                                    s.describe(),
                                    rel(&root, &d)
                                );
                                if push(&mut moves, p, d.join(&name), false, why) {
                                    by_name += 1;
                                }
                            }
                        }
                        continue;
                    }
                    Some(s @ Shape::Log { .. }) if sense::ask_customer(s, &p) => {
                        if let Some(sub) = sense::customer_sub(s) {
                            ask_logs.push((p, root.join(sub)));
                        }
                        continue;
                    }
                    _ => {}
                }
            }
            // (0.20.0, §14.30) An archive no customer claims: into the
            // topic folder made for its subject before, else grouped with
            // the archives that share its subject (after the walk).
            if zone && shape.is_none() && b.clues.is_some() && group::is_archive(&p) {
                if let Some(d) = group::topic_dir(&root, &o.other_dir, &name) {
                    if d != dir {
                        let why = format!(
                            "이름에 주제 '{}'가 보임 → '{}'",
                            name_of(&d),
                            rel(&root, &d)
                        );
                        if push(&mut moves, p, d.join(&name), false, why) {
                            by_group += 1;
                        }
                    }
                    continue;
                }
                loose.push(Loose {
                    item: group::Item {
                        name: name.clone(),
                        inner: group::inner_subject(&entries),
                    },
                    path: p,
                    dir: dir.clone(),
                    entries,
                });
                continue;
            }
            if !zone {
                continue;
            }
            if let Some((to, why)) = rule_dest(&p, &dir) {
                if push(&mut moves, p, to, false, why) {
                    by_rules += 1;
                }
            }
        }
    }

    // 4b. (0.20.0, §14.30) Archives sharing a subject → one folder: a
    // company's → `{업체}/압축파일` (log bundles to its day folders), a
    // topic's → `기타/{주제}`. A lone archive named for a company → that
    // company. The rest → the rules, as before.
    let items: Vec<group::Item> = loose.iter().map(|l| l.item.clone()).collect();
    let mut placed = vec![false; loose.len()];
    let mut targets: Vec<(Vec<usize>, String, bool)> = group::groups(&items, o.min_items)
        .into_iter()
        .map(|g| (g.members, g.label, g.company))
        .collect();
    for (i, l) in loose.iter().enumerate() {
        if targets.iter().any(|(m, _, _)| m.contains(&i)) {
            continue;
        }
        if let Some(c) = group::company_in(&l.item.name) {
            match targets
                .iter_mut()
                .find(|(_, n, co)| *co && norm(n) == norm(&c))
            {
                Some(t) => t.0.push(i),
                None => targets.push((vec![i], c, true)),
            }
        }
    }
    for (members, label, company) in targets {
        let mut count = 0usize;
        if company {
            // An existing customer of that name, or a new one.
            let cust = match b.in_text(&label) {
                Some(ci) => Customer {
                    name: b.customers[ci].name.clone(),
                    keys: b.customers[ci].keys.clone(),
                    dir: b.customers[ci].dir.clone(),
                },
                None => Customer {
                    dir: reuse_existing(&root.join(clean(&label)), &root, 0),
                    keys: vec![norm(&label)],
                    name: label.clone(),
                },
            };
            let clues = b.clues.clone().map(|mut c| {
                c.add_customer(&cust.name, cust.keys.clone());
                c
            });
            for &i in &members {
                let l = &loose[i];
                let shape = clues
                    .as_ref()
                    .and_then(|c| sense::shape(&l.path, c, Some(&cust.name), Some(&l.entries)));
                let shape = shape.filter(|s| matches!(s, Shape::Log { .. } | Shape::Manual { .. }));
                let d = b.customer_dest(&cust.dir, &l.path, shape.as_ref());
                if d == l.dir {
                    placed[i] = true;
                    continue;
                }
                note_new(&cust);
                let what = shape
                    .as_ref()
                    .map(|s| format!(" · {}", s.describe()))
                    .unwrap_or_default();
                let why = if members.len() > 1 {
                    format!(
                        "이름이 비슷한 압축 파일 {}개 = 업체 '{}'{what} → '{}'",
                        members.len(),
                        cust.name,
                        rel(&root, &d)
                    )
                } else {
                    format!("이름이 업체 '{}'{what} → '{}'", cust.name, rel(&root, &d))
                };
                if push(
                    &mut moves,
                    l.path.clone(),
                    d.join(name_of(&l.path)),
                    false,
                    why,
                ) {
                    count += 1;
                }
                placed[i] = true;
            }
        } else {
            let d = reuse_existing(&root.join(&o.other_dir).join(clean(&label)), &root, 0);
            for &i in &members {
                let l = &loose[i];
                placed[i] = true;
                if d == l.dir {
                    continue;
                }
                let why = format!(
                    "이름이 비슷한 압축 파일 {}개 = 주제 '{label}' → '{}'",
                    members.len(),
                    rel(&root, &d)
                );
                if push(
                    &mut moves,
                    l.path.clone(),
                    d.join(name_of(&l.path)),
                    false,
                    why,
                ) {
                    count += 1;
                }
            }
        }
        if count > 0 {
            by_group += count;
            groups.push((
                rel(
                    &root,
                    &if company {
                        reuse_existing(&root.join(clean(&label)), &root, 0)
                    } else {
                        root.join(&o.other_dir).join(clean(&label))
                    },
                ),
                count,
            ));
        }
    }
    for (i, l) in loose.iter().enumerate() {
        if placed[i] {
            continue;
        }
        if let Some((to, why)) = rule_dest(&l.path, &l.dir) {
            if push(&mut moves, l.path.clone(), to, false, why) {
                by_rules += 1;
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
    if by_name > 0 {
        parts.push(format!(
            "로그 묶음·매뉴얼·설치 파일 {by_name}개를 이름·내용대로 날짜·서비스·OS 폴더에 넣습니다"
        ));
    }
    if by_group > 0 {
        let names: Vec<String> = groups
            .iter()
            .take(8)
            .map(|(n, c)| format!("{n}({c})"))
            .collect();
        parts.push(format!(
            "압축 파일 {by_group}개를 이름·안의 파일로 업체·주제별로 묶습니다({}{})",
            names.join(", "),
            if groups.len() > 8 { " …" } else { "" }
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
    if !ask_logs.is_empty() {
        summary.push_str(&format!(
            " 고객사를 알 수 없는 로그 {}개는 그대로 두고 확인 대기에서 어느 고객사인지 묻습니다.",
            ask_logs.len()
        ));
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
        by_name,
        ask_logs,
        by_group,
        groups,
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
    let r = rebuild(root, store, &o)?;
    // (0.18.0) Logs without a customer: "어느 고객사?" in 확인 대기.
    for (p, sug) in &r.ask_logs {
        if store.get_pending_by_path(p).ok().flatten().is_none() {
            if let Err(e) = store.upsert_pending(p, Some(sug), Some(sense::ASK_CUSTOMER_RULE)) {
                tracing::warn!(path = %p.display(), "not queued: {e}");
            }
        }
    }
    Ok(r)
}
