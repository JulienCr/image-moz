use std::cmp::Reverse;
use std::collections::HashMap;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::{Instant, SystemTime};

use serde::{Deserialize, Serialize};

use crate::formats::is_supported;

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

    /// Entries grouped by folder; a non-blank `filter` keeps names containing it, ignoring case.
    pub fn sections(&self, sort: SortOrder, filter: &str, flat: bool) -> Vec<Section> {
        let needle = filter.trim().to_lowercase();
        let mut groups: HashMap<&str, Vec<&Entry>> = HashMap::new();
        for e in self.entries.values().filter(|e| needle.is_empty() || e.name.to_lowercase().contains(&needle)) {
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

    fn touch(path: &Path, age_secs: u64) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let f = File::create(path).unwrap();
        let t = SystemTime::now() - Duration::from_secs(age_secs);
        f.set_modified(t).unwrap();
    }

    fn names(lib: &Library, sort: SortOrder) -> Vec<String> {
        lib.sections(sort, "", false)
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
        let dirs = |s| lib.sections(s, "", false).into_iter().map(|s| s.rel_dir).collect::<Vec<_>>();
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
            .sections(SortOrder::DateDesc, "", false)
            .into_iter()
            .map(|s| s.rel_dir)
            .collect();
        assert_eq!(dirs, ["", "newer", "older"]);
        let dirs: Vec<_> = lib
            .sections(SortOrder::NameAsc, "", false)
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
            let dirs: Vec<_> = lib.sections(SortOrder::DateDesc, "", false).into_iter().map(|s| s.rel_dir).collect();
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
                .sections(SortOrder::NameAsc, q, false)
                .iter()
                .flat_map(|s| s.items.iter().map(|p| lib.get(p).unwrap().name.clone()))
                .collect();
            v.sort();
            v
        };
        assert_eq!(found("CAT"), ["Cat.png"]);
        assert_eq!(found("  dog "), ["dog.png", "dog2.png"]);
        assert_eq!(found("   ").len(), 4);
        let dirs: Vec<_> = lib.sections(SortOrder::NameAsc, "owl", false).into_iter().map(|s| s.rel_dir).collect();
        assert_eq!(dirs, ["birds"]);
        assert!(lib.sections(SortOrder::NameAsc, "zzz", false).is_empty());
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
            let sections = lib.sections(SortOrder::DateDesc, q, true);
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
