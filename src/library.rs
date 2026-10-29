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
    pub size: u64,
    pub dims: Option<(u32, u32)>,
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
        size: meta.len(),
        dims: None,
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
                        e.size = m.len();
                    }
                    e.dims = None;
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

    pub fn sections(&self, sort: SortOrder) -> Vec<Section> {
        let mut groups: HashMap<&str, Vec<&Entry>> = HashMap::new();
        for e in self.entries.values() {
            groups.entry(e.rel_dir.as_str()).or_default().push(e);
        }
        let mut sections: Vec<(&str, Vec<&Entry>)> = groups.into_iter().collect();
        for (_, items) in &mut sections {
            match sort {
                SortOrder::DateDesc => items.sort_by_key(|e| (Reverse(e.mtime), e.name.to_lowercase())),
                SortOrder::DateAsc => items.sort_by_key(|e| (e.mtime, e.name.to_lowercase())),
                SortOrder::NameAsc => items.sort_by_key(|e| e.name.to_lowercase()),
                SortOrder::NameDesc => items.sort_by_key(|e| Reverse(e.name.to_lowercase())),
            }
        }
        let newest = |items: &[&Entry]| items.iter().map(|e| e.mtime).max();
        sections.sort_by(|(da, a), (db, b)| {
            (!da.is_empty()).cmp(&!db.is_empty()).then_with(|| match sort {
                SortOrder::DateDesc => newest(b).cmp(&newest(a)),
                SortOrder::DateAsc => newest(a).cmp(&newest(b)),
                SortOrder::NameAsc => da.to_lowercase().cmp(&db.to_lowercase()),
                SortOrder::NameDesc => db.to_lowercase().cmp(&da.to_lowercase()),
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
        lib.sections(sort)
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
    fn sections_root_first_then_by_latest_mtime() {
        let d = tempfile::tempdir().unwrap();
        touch(&d.path().join("root.png"), 500);
        touch(&d.path().join("older/o.png"), 200);
        touch(&d.path().join("newer/n.png"), 5);
        let lib = lib_of(d.path(), true);
        let dirs: Vec<_> = lib
            .sections(SortOrder::DateDesc)
            .into_iter()
            .map(|s| s.rel_dir)
            .collect();
        assert_eq!(dirs, ["", "newer", "older"]);
        let dirs: Vec<_> = lib
            .sections(SortOrder::NameAsc)
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
            let dirs: Vec<_> = lib.sections(SortOrder::DateDesc).into_iter().map(|s| s.rel_dir).collect();
            assert_eq!(dirs, ["ALPHA2", "Beta", "alpha"]);
        }
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
