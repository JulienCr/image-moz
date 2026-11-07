use crate::layout::{BentoRule, LayoutKind};
use crate::library::SortOrder;
use directories::BaseDirs;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};

pub const MIN_SIZE: f32 = 64.0;
pub const MAX_SIZE: f32 = 512.0;
pub const DEFAULT_SIZE: f32 = 192.0;

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct FolderState {
    /// Paths relative to the folder, lowercased, '/'-separated.
    pub pins: BTreeSet<String>,
    pub collapsed: BTreeSet<String>,
    pub recursive: bool,
    pub sort: SortOrder,
}

/// The per-folder view settings a tab edits from the toolbar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FolderSettings {
    pub recursive: bool,
    pub sort: SortOrder,
}

/// One window's tabs; a new field here (e.g. bounds) loads from older files through `default`.
#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct WindowSession {
    pub tabs: Vec<PathBuf>,
    pub active: usize,
}

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Session {
    pub windows: Vec<WindowSession>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Config {
    pub session: Session,
    pub layout: LayoutKind,
    pub target_size: f32,
    pub bento_rule: BentoRule,
    pub folders: BTreeMap<String, FolderState>,
    /// Pre-session fields: read once by `migrate_legacy`, never written back.
    #[serde(skip_serializing)]
    last_folder: Option<PathBuf>,
    #[serde(skip_serializing)]
    recursive: Option<bool>,
    #[serde(skip_serializing)]
    sort: Option<SortOrder>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            session: Session::default(),
            layout: LayoutKind::default(),
            target_size: DEFAULT_SIZE,
            bento_rule: BentoRule::default(),
            folders: BTreeMap::new(),
            last_folder: None,
            recursive: None,
            sort: None,
        }
    }
}

// No canonicalize(): it yields `\?\` verbatim paths on Windows.
fn absolute_lower(path: &Path) -> PathBuf {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    PathBuf::from(abs.to_string_lossy().to_lowercase())
}

pub fn folder_key(folder: &Path) -> String {
    let s = absolute_lower(folder).to_string_lossy().into_owned();
    let trimmed = s.trim_end_matches(['\\', '/']);
    if trimmed.is_empty() { s } else { trimmed.to_string() }
}

pub fn rel_key(folder: &Path, file: &Path) -> Option<String> {
    let rel = absolute_lower(file)
        .strip_prefix(absolute_lower(folder))
        .ok()?
        .to_path_buf();
    let parts: Vec<_> = rel.components().map(|c| c.as_os_str().to_string_lossy()).collect();
    if parts.is_empty() {
        return None;
    }
    Some(parts.join("/"))
}

impl Config {
    pub fn default_path() -> PathBuf {
        let base = BaseDirs::new()
            .map(|d| d.config_dir().to_path_buf())
            .unwrap_or_default();
        base.join("image-moz").join("config.json")
    }

    pub fn load(path: &Path) -> Config {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) => {
                if e.kind() != io::ErrorKind::NotFound {
                    log::warn!("cannot read config {}: {e}", path.display());
                }
                return Config::default();
            }
        };
        match serde_json::from_str::<Config>(&text) {
            Ok(mut cfg) => {
                cfg.target_size = if cfg.target_size.is_finite() {
                    cfg.target_size.clamp(MIN_SIZE, MAX_SIZE)
                } else {
                    DEFAULT_SIZE
                };
                cfg.migrate_legacy();
                cfg
            }
            Err(e) => {
                log::warn!("corrupt config {}: {e}", path.display());
                Config::default()
            }
        }
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path)
    }

    /// `last_folder` becomes the single tab of a session-less file; its global view settings follow it.
    fn migrate_legacy(&mut self) {
        let (folder, recursive, sort) = (self.last_folder.take(), self.recursive.take(), self.sort.take());
        let Some(folder) = folder.filter(|_| self.session.windows.is_empty()) else { return };
        let state = self.folder_mut(&folder);
        state.recursive = recursive.unwrap_or(state.recursive);
        state.sort = sort.unwrap_or(state.sort);
        self.session.windows.push(WindowSession { tabs: vec![folder], active: 0 });
    }

    pub fn folder_settings(&self, folder: &Path) -> FolderSettings {
        self.folder(folder)
            .map_or_else(FolderSettings::default, |s| FolderSettings { recursive: s.recursive, sort: s.sort })
    }

    /// Returns true when the stored settings changed.
    pub fn set_folder_settings(&mut self, folder: &Path, settings: FolderSettings) -> bool {
        if self.folder_settings(folder) == settings {
            return false;
        }
        let state = self.folder_mut(folder);
        state.recursive = settings.recursive;
        state.sort = settings.sort;
        true
    }

    pub fn folder(&self, folder: &Path) -> Option<&FolderState> {
        self.folders.get(&folder_key(folder))
    }

    pub fn folder_mut(&mut self, folder: &Path) -> &mut FolderState {
        self.folders.entry(folder_key(folder)).or_default()
    }

    /// Returns the new pinned state; false when the file is outside the folder.
    pub fn toggle_pin(&mut self, folder: &Path, file: &Path) -> bool {
        let Some(rel) = rel_key(folder, file) else {
            return false;
        };
        let pins = &mut self.folder_mut(folder).pins;
        if pins.remove(&rel) {
            false
        } else {
            pins.insert(rel);
            true
        }
    }

    pub fn is_pinned(&self, folder: &Path, file: &Path) -> bool {
        match (self.folder(folder), rel_key(folder, file)) {
            (Some(state), Some(rel)) => state.pins.contains(&rel),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_save_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("config.json");
        let mut cfg = Config { target_size: 300.0, ..Config::default() };
        cfg.session.windows.push(WindowSession { tabs: vec![PathBuf::from(r"D:\Pics")], active: 0 });
        cfg.set_folder_settings(
            Path::new(r"D:\Pics"),
            FolderSettings { recursive: true, sort: SortOrder::NameDesc },
        );
        cfg.toggle_pin(Path::new(r"D:\Pics"), Path::new(r"D:\Pics\a.png"));
        cfg.save(&path).unwrap();
        assert!(!path.with_file_name("config.json.tmp").exists());
        assert_eq!(Config::load(&path), cfg);
    }

    #[test]
    fn load_missing_is_default() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Config::load(&dir.path().join("nope.json")), Config::default());
    }

    #[test]
    fn load_corrupt_config_falls_back_to_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(Config::load(&path), Config::default());
    }

    #[test]
    fn unknown_and_missing_fields_tolerated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, r#"{"layout":"Bento","future":1}"#).unwrap();
        let cfg = Config::load(&path);
        assert_eq!(cfg.layout, LayoutKind::Bento);
        assert_eq!(cfg.target_size, DEFAULT_SIZE);
        assert!(cfg.session.windows.is_empty());
    }

    #[test]
    fn pre_session_config_becomes_a_single_tab() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let old = r#"{
            "last_folder": "D:\\Pics",
            "layout": "Justified",
            "target_size": 240.0,
            "sort": "NameAsc",
            "recursive": true,
            "bento_rule": "Pinned",
            "folders": { "d:\\pics": { "pins": ["a.png"], "collapsed": ["sub"] } }
        }"#;
        std::fs::write(&path, old).unwrap();
        let cfg = Config::load(&path);
        assert_eq!(cfg.session.windows, [WindowSession { tabs: vec![PathBuf::from(r"D:\Pics")], active: 0 }]);
        assert_eq!(
            cfg.folder_settings(Path::new(r"D:\Pics")),
            FolderSettings { recursive: true, sort: SortOrder::NameAsc }
        );
        assert_eq!(cfg.layout, LayoutKind::Justified);
        assert!(cfg.is_pinned(Path::new(r"D:\Pics"), Path::new(r"D:\Pics\a.png")));

        cfg.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("last_folder"));
        assert_eq!(Config::load(&path), cfg);
    }

    #[test]
    fn legacy_folder_does_not_override_a_stored_session() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let json = r#"{"last_folder":"D:\\Old","session":{"windows":[{"tabs":[],"active":0}]}}"#;
        std::fs::write(&path, json).unwrap();
        let cfg = Config::load(&path);
        assert_eq!(cfg.session.windows, [WindowSession::default()]);
    }

    #[test]
    fn unseen_folder_gets_default_settings() {
        let mut cfg = Config::default();
        let folder = Path::new(r"D:\New");
        assert_eq!(cfg.folder_settings(folder), FolderSettings { recursive: false, sort: SortOrder::DateDesc });
        assert!(cfg.set_folder_settings(folder, FolderSettings { recursive: true, ..Default::default() }));
        assert!(!cfg.set_folder_settings(folder, FolderSettings { recursive: true, ..Default::default() }));
        assert!(cfg.folder_settings(Path::new(r"d:\new\")).recursive);
        assert!(!cfg.folder_settings(Path::new(r"D:\Other")).recursive);
    }

    #[test]
    fn folder_key_is_case_insensitive() {
        assert_eq!(folder_key(Path::new(r"D:\Dev\X")), folder_key(Path::new(r"d:\dev\x\")));
        let mut cfg = Config::default();
        assert!(cfg.toggle_pin(Path::new(r"D:\Dev\X"), Path::new(r"D:\Dev\X\Render.PNG")));
        assert!(cfg.is_pinned(Path::new(r"d:\dev\x\"), Path::new(r"d:\dev\x\render.png")));
        assert!(!cfg.toggle_pin(Path::new(r"d:\dev\x\"), Path::new(r"d:\dev\x\render.png")));
        assert!(!cfg.is_pinned(Path::new(r"D:\Dev\X"), Path::new(r"D:\Dev\X\Render.PNG")));
    }

    #[test]
    fn rel_key_is_relative_slash_separated() {
        assert_eq!(
            rel_key(Path::new(r"D:\Dev\X"), Path::new(r"d:\dev\x\Sub\Render.PNG")).as_deref(),
            Some("sub/render.png")
        );
        assert_eq!(rel_key(Path::new(r"D:\Dev\X"), Path::new(r"D:\Other\a.png")), None);
    }

    #[test]
    fn target_size_clamped_on_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, r#"{"target_size":9999}"#).unwrap();
        assert_eq!(Config::load(&path).target_size, MAX_SIZE);
        std::fs::write(&path, r#"{"target_size":1}"#).unwrap();
        assert_eq!(Config::load(&path).target_size, MIN_SIZE);
    }

    #[test]
    fn default_path_ends_with_app_dir() {
        assert!(Config::default_path().ends_with(Path::new(r"image-moz\config.json")));
    }
}
