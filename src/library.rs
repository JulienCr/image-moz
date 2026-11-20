use std::cmp::Reverse;
use std::collections::{BTreeSet, HashMap};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::{Instant, SystemTime};

use serde::{Deserialize, Serialize};

use crate::formats::{is_supported, type_label};

#[derive(Debug, Clone)]
pub struct Entry {
    pub path: PathBuf,
    /// Directory relative to the root, '/'-separated; empty for the root itself.
    pub rel_dir: String,
    pub name: String,
    pub mtime: SystemTime,
    /// Not every filesystem records a creation time.
    pub created: Option<SystemTime>,
    pub size: u64,
    pub dims: Option<(u32, u32)>,
    /// Triangle count of a 3D model, once its thumbnail has been made.
    pub triangles: Option<u32>,
    /// The file is valid but has no preview to draw (a `.blend` without thumbnail).
    pub no_preview: bool,
    pub error: bool,
    pub arrived_at: Option<Instant>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SortOrder {
    #[default]
    DateDesc,
    DateAsc,
    NameAsc,
    NameDesc,
    SizeDesc,
    SizeAsc,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FsChange {
    Upsert(PathBuf),
    Remove(PathBuf),
    Rescan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub rel_dir: String,
    pub items: Vec<PathBuf>,
}

/// What the gallery shows; every active criterion must match.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    /// Case-insensitive part of the file name; blank matches everything.
    pub text: String,
    /// Keeps entries modified or created at or after this instant (the start of today).
    pub since: Option<SystemTime>,
    /// Selected type labels (see `type_label`); empty means every type.
    pub types: BTreeSet<String>,
}

impl Filter {
    pub fn named(text: &str) -> Self {
        Self { text: text.to_owned(), ..Self::default() }
    }

    /// Any criterion besides the name.
    pub fn has_quick(&self) -> bool {
        self.since.is_some() || !self.types.is_empty()
    }

    pub fn is_active(&self) -> bool {
        self.has_quick() || !self.text.trim().is_empty()
    }

    fn matches(&self, e: &Entry, needle: &str) -> bool {
        (needle.is_empty() || e.name.to_lowercase().contains(needle))
            && self.since.is_none_or(|t| e.mtime >= t || e.created.is_some_and(|c| c >= t))
            && (self.types.is_empty() || type_label(&e.path).is_some_and(|l| self.types.contains(&l)))
    }
}

/// What the quick-filter chips show, over the whole library.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QuickCounts {
    pub today: usize,
    /// Type label and file count, most files first, then by name.
    pub types: Vec<(String, usize)>,
}

fn comp_key(c: Component) -> String {
    c.as_os_str().to_string_lossy().to_lowercase()
}

/// Lowercased map key; Windows paths are case-insensitive.
fn map_key(path: &Path) -> PathBuf {
    path.components().map(comp_key).collect()
}

/// `/`-joined directory of `path` below `root`, or `None` if outside it.
fn rel_dir_of(root: &Path, path: &Path) -> Option<String> {
    let mut rc = root.components().map(comp_key);
    let mut pc = path.parent()?.components();
    for r in rc.by_ref() {
        if comp_key(pc.next()?) != r {
            return None;
        }
    }
    let parts: Vec<_> = pc.map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    Some(parts.join("/"))
}

fn make_entry(path: &Path, rel_dir: String) -> Option<Entry> {
    let meta = path.metadata().ok()?;
    if !meta.is_file() {
        return None;
    }
    Some(Entry {
        path: path.to_path_buf(),
        rel_dir,
        name: path.file_name()?.to_string_lossy().into_owned(),
        mtime: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
        created: meta.created().ok(),
        size: meta.len(),
        dims: None,
        triangles: None,
        no_preview: false,
        error: false,
        arrived_at: None,
    })
}

fn walk(root: &Path, dir: &Path, recursive: bool, out: &mut Vec<Entry>) -> io::Result<()> {
    for item in std::fs::read_dir(dir)? {
        let item = item?;
        let path = item.path();
        let ft = item.file_type()?;
        if ft.is_dir() {
            if recursive && let Err(e) = walk(root, &path, true, out) {
                log::warn!("skipping unreadable dir {}: {e}", path.display());
            }
        } else if is_supported(&path)
            && let Some(rel) = rel_dir_of(root, &path)
        {
            out.extend(make_entry(&path, rel));
        }
    }
    Ok(())
}

pub fn scan(root: &Path, recursive: bool) -> io::Result<Vec<Entry>> {
    let mut out = Vec::new();
    walk(root, root, recursive, &mut out)?;
    Ok(out)
}

pub struct Library {
    root: PathBuf,
    recursive: bool,
    entries: HashMap<PathBuf, Entry>,
}

impl Library {
    pub fn new(root: PathBuf, recursive: bool) -> Self {
        Self { root, recursive, entries: HashMap::new() }
    }

    pub fn replace_all(&mut self, entries: Vec<Entry>) {
        self.entries = entries.into_iter().map(|e| (map_key(&e.path), e)).collect();
    }

    /// Returns true when a new entry was added. `Rescan` is the caller's job.
    pub fn apply(&mut self, change: &FsChange, now: Instant) -> bool {
        match change {
            FsChange::Rescan => false,
            FsChange::Remove(p) => {
                self.entries.remove(&map_key(p));
                false
            }
            FsChange::Upsert(p) => {
                if !is_supported(p) {
                    return false;
                }
                let Some(rel) = rel_dir_of(&self.root, p) else { return false };
                if !self.recursive && !rel.is_empty() {
                    return false;
                }
                let key = map_key(p);
                if let Some(e) = self.entries.get_mut(&key) {
                    if let Ok(m) = p.metadata() {
                        e.mtime = m.modified().unwrap_or(e.mtime);
                        e.created = m.created().ok().or(e.created);
                        e.size = m.len();
                    }
                    e.dims = None;
                    e.triangles = None;
                    e.no_preview = false;
                    e.error = false;
                    return false;
                }
                let Some(mut e) = make_entry(p, rel) else { return false };
                e.arrived_at = Some(now);
                self.entries.insert(key, e);
                true
            }
        }
    }

    pub fn get(&self, path: &Path) -> Option<&Entry> {
        self.entries.get(&map_key(path))
    }

    pub fn get_mut(&mut self, path: &Path) -> Option<&mut Entry> {
        self.entries.get_mut(&map_key(path))
    }

    pub fn iter(&self) -> impl Iterator<Item = &Entry> {
        self.entries.values()
    }

    /// Lets every failed entry be retried.
    pub fn clear_errors(&mut self) {
        for entry in self.entries.values_mut() {
            entry.error = false;
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Counts for the quick-filter chips; `since` is the start of today.
    pub fn quick_counts(&self, since: SystemTime) -> QuickCounts {
        let mut today = 0;
        let mut by_type: HashMap<String, usize> = HashMap::new();
        for e in self.entries.values() {
            today += usize::from(e.mtime >= since || e.created.is_some_and(|c| c >= since));
            if let Some(label) = type_label(&e.path) {
                *by_type.entry(label).or_default() += 1;
            }
        }
        let mut types: Vec<_> = by_type.into_iter().collect();
        types.sort_by(|(na, ca), (nb, cb)| cb.cmp(ca).then_with(|| na.cmp(nb)));
        QuickCounts { today, types }
    }

    /// Entries grouped by folder, keeping those that match `filter`.
    pub fn sections(&self, sort: SortOrder, filter: &Filter, flat: bool) -> Vec<Section> {
        let needle = filter.text.trim().to_lowercase();
        let mut groups: HashMap<&str, Vec<&Entry>> = HashMap::new();
        for e in self.entries.values().filter(|e| filter.matches(e, &needle)) {
            groups.entry(if flat { "" } else { e.rel_dir.as_str() }).or_default().push(e);
        }
        let mut sections: Vec<(&str, Vec<&Entry>)> = groups.into_iter().collect();
        for (_, items) in &mut sections {
            match sort {
                SortOrder::DateDesc => items.sort_by_key(|e| (Reverse(e.mtime), e.name.to_lowercase())),
                SortOrder::DateAsc => items.sort_by_key(|e| (e.mtime, e.name.to_lowercase())),
                SortOrder::NameAsc => items.sort_by_key(|e| e.name.to_lowercase()),
                SortOrder::NameDesc => items.sort_by_key(|e| Reverse(e.name.to_lowercase())),
                SortOrder::SizeDesc => items.sort_by_key(|e| (Reverse(e.size), e.name.to_lowercase())),
                SortOrder::SizeAsc => items.sort_by_key(|e| (e.size, e.name.to_lowercase())),
            }
        }
        let newest = |items: &[&Entry]| items.iter().map(|e| e.mtime).max();
        let total = |items: &[&Entry]| items.iter().map(|e| e.size).sum::<u64>();
        sections.sort_by(|(da, a), (db, b)| {
            (!da.is_empty()).cmp(&!db.is_empty()).then_with(|| match sort {
                SortOrder::DateDesc => newest(b).cmp(&newest(a)),
                SortOrder::DateAsc => newest(a).cmp(&newest(b)),
                SortOrder::NameAsc => da.to_lowercase().cmp(&db.to_lowercase()),
                SortOrder::NameDesc => db.to_lowercase().cmp(&da.to_lowercase()),
                SortOrder::SizeDesc => total(b).cmp(&total(a)),
                SortOrder::SizeAsc => total(a).cmp(&total(b)),
            })
            .then_with(|| da.cmp(db))
        });
        sections
            .into_iter()
            .map(|(d, items)| Section {
                rel_dir: d.to_string(),
                items: items.into_iter().map(|e| e.path.clone()).collect(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use std::time::Duration;

    /// A fixed "start of today" for the tests that build entries by hand.
    fn noon() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
    }

    fn touch(path: &Path, age_secs: u64) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let f = File::create(path).unwrap();
        let t = SystemTime::now() - Duration::from_secs(age_secs);
        f.set_modified(t).unwrap();
    }

    fn names(lib: &Library, sort: SortOrder) -> Vec<String> {
        lib.sections(sort, &Filter::default(), false)
            .iter()
            .flat_map(|s| s.items.iter().map(|p| lib.get(p).unwrap().name.clone()))
            .collect()
    }

    fn lib_of(dir: &Path, recursive: bool) -> Library {
        let mut lib = Library::new(dir.to_path_buf(), recursive);
        lib.replace_all(scan(dir, recursive).unwrap());
        lib
    }

    #[test]
    fn scan_flat_ignores_subdirs_and_non_images() {
        let d = tempfile::tempdir().unwrap();
        touch(&d.path().join("a.png"), 10);
        touch(&d.path().join("b.txt"), 10);
        touch(&d.path().join("sub/c.png"), 10);
        let es = scan(d.path(), false).unwrap();
        assert_eq!(es.len(), 1);
        assert_eq!(es[0].name, "a.png");
        assert_eq!(es[0].rel_dir, "");
    }

    #[test]
    fn scan_recursive_sets_rel_dir() {
        let d = tempfile::tempdir().unwrap();
        touch(&d.path().join("sub/deep/x.png"), 10);
        let es = scan(d.path(), true).unwrap();
        assert_eq!(es.len(), 1);
        assert_eq!(es[0].rel_dir, "sub/deep");
    }

    #[test]
    fn scan_missing_dir_is_err() {
        let d = tempfile::tempdir().unwrap();
        assert!(scan(&d.path().join("nope"), false).is_err());
    }

    #[test]
    fn sort_date_desc_default() {
        let d = tempfile::tempdir().unwrap();
        touch(&d.path().join("old.png"), 100);
        touch(&d.path().join("new.png"), 1);
        let lib = lib_of(d.path(), false);
        assert_eq!(SortOrder::default(), SortOrder::DateDesc);
        assert_eq!(names(&lib, SortOrder::DateDesc), ["new.png", "old.png"]);
        assert_eq!(names(&lib, SortOrder::DateAsc), ["old.png", "new.png"]);
    }

    #[test]
    fn sort_name_case_insensitive() {
        let d = tempfile::tempdir().unwrap();
        for n in ["b.png", "A.png", "c.png"] {
            touch(&d.path().join(n), 10);
        }
        let lib = lib_of(d.path(), false);
        assert_eq!(names(&lib, SortOrder::NameAsc), ["A.png", "b.png", "c.png"]);
        assert_eq!(names(&lib, SortOrder::NameDesc), ["c.png", "b.png", "A.png"]);
    }

    #[test]
    fn sort_size_orders_items_and_breaks_ties_by_name() {
        let d = tempfile::tempdir().unwrap();
        for (n, bytes) in [("b.png", 10), ("a.png", 10), ("big.png", 500), ("small.png", 1)] {
            touch(&d.path().join(n), 10);
            fs::write(d.path().join(n), vec![0u8; bytes]).unwrap();
        }
        let lib = lib_of(d.path(), false);
        assert_eq!(names(&lib, SortOrder::SizeDesc), ["big.png", "a.png", "b.png", "small.png"]);
        assert_eq!(names(&lib, SortOrder::SizeAsc), ["small.png", "a.png", "b.png", "big.png"]);
    }

    #[test]
    fn sections_by_size_use_folder_total() {
        let d = tempfile::tempdir().unwrap();
        for (n, bytes) in [("root.png", 1), ("many/a.png", 30), ("many/b.png", 30), ("one/c.png", 50)] {
            touch(&d.path().join(n), 10);
            fs::write(d.path().join(n), vec![0u8; bytes]).unwrap();
        }
        let lib = lib_of(d.path(), true);
        let dirs = |s| lib.sections(s, &Filter::default(), false).into_iter().map(|s| s.rel_dir).collect::<Vec<_>>();
        assert_eq!(dirs(SortOrder::SizeDesc), ["", "many", "one"]);
        assert_eq!(dirs(SortOrder::SizeAsc), ["", "one", "many"]);
    }

    #[test]
    fn sections_root_first_then_by_latest_mtime() {
        let d = tempfile::tempdir().unwrap();
        touch(&d.path().join("root.png"), 500);
        touch(&d.path().join("older/o.png"), 200);
        touch(&d.path().join("newer/n.png"), 5);
        let lib = lib_of(d.path(), true);
        let dirs: Vec<_> = lib
            .sections(SortOrder::DateDesc, &Filter::default(), false)
            .into_iter()
            .map(|s| s.rel_dir)
            .collect();
        assert_eq!(dirs, ["", "newer", "older"]);
        let dirs: Vec<_> = lib
            .sections(SortOrder::NameAsc, &Filter::default(), false)
            .into_iter()
            .map(|s| s.rel_dir)
            .collect();
        assert_eq!(dirs, ["", "newer", "older"]);
    }

    #[test]
    fn sections_with_equal_sort_keys_are_ordered_by_rel_dir() {
        let d = tempfile::tempdir().unwrap();
        let t = SystemTime::now() - Duration::from_secs(50);
        for name in ["Beta", "alpha", "ALPHA2"] {
            let p = d.path().join(name).join("x.png");
            touch(&p, 0);
            File::options().write(true).open(&p).unwrap().set_modified(t).unwrap();
        }
        for _ in 0..20 {
            let lib = lib_of(d.path(), true);
            let dirs: Vec<_> = lib.sections(SortOrder::DateDesc, &Filter::default(), false).into_iter().map(|s| s.rel_dir).collect();
            assert_eq!(dirs, ["ALPHA2", "Beta", "alpha"]);
        }
    }

    #[test]
    fn filter_matches_name_only_ignoring_case_and_empty_sections() {
        let d = tempfile::tempdir().unwrap();
        touch(&d.path().join("Cat.png"), 10);
        touch(&d.path().join("dog.png"), 10);
        touch(&d.path().join("cats/dog2.png"), 10);
        touch(&d.path().join("birds/owl.png"), 10);
        let lib = lib_of(d.path(), true);
        let found = |q: &str| -> Vec<String> {
            let mut v: Vec<_> = lib
                .sections(SortOrder::NameAsc, &Filter::named(q), false)
                .iter()
                .flat_map(|s| s.items.iter().map(|p| lib.get(p).unwrap().name.clone()))
                .collect();
            v.sort();
            v
        };
        assert_eq!(found("CAT"), ["Cat.png"]);
        assert_eq!(found("  dog "), ["dog.png", "dog2.png"]);
        assert_eq!(found("   ").len(), 4);
        let dirs: Vec<_> = lib.sections(SortOrder::NameAsc, &Filter::named("owl"), false).into_iter().map(|s| s.rel_dir).collect();
        assert_eq!(dirs, ["birds"]);
        assert!(lib.sections(SortOrder::NameAsc, &Filter::named("zzz"), false).is_empty());
    }

    /// A library of in-memory entries: (name, modified, created), as seconds relative to `noon()`.
    fn lib_with(files: &[(&str, i64, Option<i64>)]) -> Library {
        let time = |secs: i64| {
            let d = Duration::from_secs(secs.unsigned_abs());
            if secs >= 0 { noon() + d } else { noon() - d }
        };
        let entries = files.iter().map(|(name, m, c)| {
            Entry {
                path: PathBuf::from(name),
                rel_dir: String::new(),
                name: (*name).to_owned(),
                mtime: time(*m),
                created: c.map(time),
                size: 1,
                dims: None,
                triangles: None,
                no_preview: false,
                error: false,
                arrived_at: None,
            }
        });
        let mut lib = Library::new(PathBuf::new(), false);
        lib.replace_all(entries.collect());
        lib
    }

    fn matching(lib: &Library, filter: &Filter) -> Vec<String> {
        let mut v: Vec<_> = lib
            .sections(SortOrder::NameAsc, filter, false)
            .iter()
            .flat_map(|s| s.items.iter().map(|p| p.to_string_lossy().into_owned()))
            .collect();
        v.sort();
        v
    }

    #[test]
    fn today_filter_keeps_files_modified_or_created_since_midnight() {
        let lib = lib_with(&[
            ("modified.png", 10, Some(-500)),
            ("created.png", -500, Some(10)),
            ("neither.png", -500, Some(-400)),
            ("no_birth.png", -500, None),
            ("no_birth_today.png", 10, None),
        ]);
        let today = Filter { since: Some(noon()), ..Filter::default() };
        assert_eq!(matching(&lib, &today), ["created.png", "modified.png", "no_birth_today.png"]);
        assert_eq!(matching(&lib, &Filter::default()).len(), 5);
    }

    #[test]
    fn type_filter_ors_selected_types_and_merges_aliases() {
        let lib = lib_with(&[("a.png", 0, None), ("b.jpeg", 0, None), ("c.JPG", 0, None), ("d.tiff", 0, None), ("e.glb", 0, None)]);
        let types = |labels: &[&str]| Filter { types: labels.iter().map(|l| (*l).to_owned()).collect(), ..Filter::default() };
        assert_eq!(matching(&lib, &types(&["JPG"])), ["b.jpeg", "c.JPG"]);
        assert_eq!(matching(&lib, &types(&["PNG", "TIF"])), ["a.png", "d.tiff"]);
        assert_eq!(matching(&lib, &types(&["GLB", "JPG", "PNG"])).len(), 4);
        assert!(matching(&lib, &types(&["BLEND"])).is_empty());
    }

    #[test]
    fn name_today_and_types_combine_with_and() {
        let lib = lib_with(&[("cat.png", 10, None), ("cat.jpg", 10, None), ("dog.png", 10, None), ("cat_old.png", -10, None)]);
        let filter = Filter {
            text: " CAT ".to_owned(),
            since: Some(noon()),
            types: ["PNG".to_owned()].into(),
        };
        assert_eq!(matching(&lib, &filter), ["cat.png"]);
        assert!(filter.is_active() && filter.has_quick());
        assert!(Filter::named("cat").is_active() && !Filter::named("cat").has_quick());
        assert!(!Filter::named("  ").is_active());
    }

    #[test]
    fn quick_counts_cover_the_whole_library_sorted_by_count_then_name() {
        let lib = lib_with(&[
            ("a.png", 10, None),
            ("b.png", -10, None),
            ("c.jpeg", 10, None),
            ("d.jpg", -10, None),
            ("e.glb", -10, Some(10)),
            ("f.blend", -10, None),
        ]);
        let counts = lib.quick_counts(noon());
        assert_eq!(counts.today, 3);
        let types: Vec<_> = counts.types.iter().map(|(n, c)| (n.as_str(), *c)).collect();
        assert_eq!(types, [("JPG", 2), ("PNG", 2), ("BLEND", 1), ("GLB", 1)]);
    }

    #[test]
    fn flat_sections_merge_folders_in_one_sorted_list() {
        let d = tempfile::tempdir().unwrap();
        touch(&d.path().join("a/old.png"), 300);
        touch(&d.path().join("b/mid.png"), 200);
        touch(&d.path().join("a/new.png"), 100);
        touch(&d.path().join("b/other.png"), 50);
        let lib = lib_of(d.path(), true);
        let flat = |q: &str| -> Vec<String> {
            let sections = lib.sections(SortOrder::DateDesc, &Filter::named(q), true);
            assert!(sections.len() <= 1 && sections.iter().all(|s| s.rel_dir.is_empty()));
            sections.iter().flat_map(|s| s.items.iter().map(|p| lib.get(p).unwrap().name.clone())).collect()
        };
        assert_eq!(flat(""), ["other.png", "new.png", "mid.png", "old.png"]);
        assert_eq!(flat("o"), ["other.png", "old.png"]);
        assert!(flat("zzz").is_empty());
    }

    #[test]
    fn upsert_new_returns_true_and_marks_arrival() {
        let d = tempfile::tempdir().unwrap();
        let mut lib = lib_of(d.path(), false);
        let p = d.path().join("n.png");
        touch(&p, 1);
        let now = Instant::now();
        assert!(lib.apply(&FsChange::Upsert(p.clone()), now));
        assert_eq!(lib.get(&p).unwrap().arrived_at, Some(now));
        assert_eq!(lib.len(), 1);
    }

    #[test]
    fn modify_clears_error() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.png");
        touch(&p, 10);
        let mut lib = lib_of(d.path(), false);
        lib.get_mut(&p).unwrap().error = true;
        lib.get_mut(&p).unwrap().dims = Some((1, 1));
        assert!(!lib.apply(&FsChange::Upsert(p.clone()), Instant::now()));
        let e = lib.get(&p).unwrap();
        assert!(!e.error);
        assert_eq!(e.dims, None);
    }

    #[test]
    fn remove_drops_entry() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.png");
        touch(&p, 10);
        let mut lib = lib_of(d.path(), false);
        assert!(!lib.apply(&FsChange::Remove(p.clone()), Instant::now()));
        assert!(lib.get(&p).is_none());
        assert_eq!(lib.len(), 0);
    }

    #[test]
    fn upsert_in_subdir_ignored_when_not_recursive() {
        let d = tempfile::tempdir().unwrap();
        let mut lib = lib_of(d.path(), false);
        let p = d.path().join("sub/x.png");
        touch(&p, 1);
        assert!(!lib.apply(&FsChange::Upsert(p), Instant::now()));
        assert_eq!(lib.len(), 0);
    }
}
