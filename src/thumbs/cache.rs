use std::fs;
use std::io::{self, Cursor};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use super::{Thumb, ThumbKey};

const MAGIC: &[u8; 4] = b"IMZ1";
const HEADER_LEN: usize = 12;
static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone)]
pub struct DiskCache {
    dir: PathBuf,
}

impl DiskCache {
    pub fn open(dir: PathBuf) -> io::Result<Self> {
        fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    /// `IMAGE_MOZ_CACHE_DIR` replaces the default location; used by the end-to-end suite.
    pub fn default_dir() -> PathBuf {
        if let Some(dir) = std::env::var_os("IMAGE_MOZ_CACHE_DIR").filter(|d| !d.is_empty()) {
            return PathBuf::from(dir);
        }
        directories::BaseDirs::new()
            .map(|d| d.data_local_dir().join("image-moz").join("thumbs"))
            .unwrap_or_else(|| std::env::temp_dir().join("image-moz").join("thumbs"))
    }

    fn path_for(&self, key: ThumbKey) -> PathBuf {
        self.dir.join(format!("{:016x}.imz", key.0))
    }

    /// Returns the cached thumbnail and refreshes its mtime so purging is LRU.
    pub fn get(&self, key: ThumbKey) -> Option<Thumb> {
        let path = self.path_for(key);
        let bytes = fs::read(&path).ok()?;
        let thumb = decode(&bytes)?;
        if let Ok(file) = fs::OpenOptions::new().write(true).open(&path) {
            let _ = file.set_modified(SystemTime::now());
        }
        Some(thumb)
    }

    pub fn put(&self, key: ThumbKey, thumb: &Thumb) -> io::Result<()> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&thumb.orig_dims.0.to_le_bytes());
        bytes.extend_from_slice(&thumb.orig_dims.1.to_le_bytes());
        let mut qoi = Cursor::new(Vec::new());
        thumb
            .image
            .write_to(&mut qoi, image::ImageFormat::Qoi)
            .map_err(io::Error::other)?;
        bytes.extend_from_slice(&qoi.into_inner());
        let tmp = self.dir.join(format!(
            "{:016x}.{}.tmp",
            key.0,
            TMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(&tmp, &bytes)?;
        fs::rename(&tmp, self.path_for(key)).inspect_err(|_| {
            let _ = fs::remove_file(&tmp);
        })
    }

    /// Deletes the least recently used entries until the total is at most
    /// `ceiling`; returns the number of bytes removed.
    pub fn purge_to(&self, ceiling: u64) -> io::Result<u64> {
        let mut entries = Vec::new();
        for entry in fs::read_dir(&self.dir)?.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "imz") {
                continue;
            }
            if let Ok(meta) = entry.metadata() {
                let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                entries.push((mtime, meta.len(), path));
            }
        }
        let mut total: u64 = entries.iter().map(|e| e.1).sum();
        entries.sort_by_key(|e| e.0);
        let mut removed = 0;
        for (_, len, path) in entries {
            if total <= ceiling {
                break;
            }
            if fs::remove_file(&path).is_ok() {
                total -= len;
                removed += len;
            }
        }
        Ok(removed)
    }
}

fn decode(bytes: &[u8]) -> Option<Thumb> {
    if bytes.len() < HEADER_LEN || &bytes[..4] != MAGIC {
        return None;
    }
    let w = u32::from_le_bytes(bytes[4..8].try_into().ok()?);
    let h = u32::from_le_bytes(bytes[8..12].try_into().ok()?);
    let image = image::load_from_memory_with_format(&bytes[HEADER_LEN..], image::ImageFormat::Qoi)
        .ok()?
        .to_rgba8();
    Some(Thumb {
        image,
        orig_dims: (w, h),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thumbs::key_for;
    use std::path::Path;
    use std::time::Duration;

    fn thumb(w: u32, h: u32) -> Thumb {
        let image = image::RgbaImage::from_fn(w, h, |x, y| image::Rgba([x as u8, y as u8, 7, 255]));
        Thumb {
            image,
            orig_dims: (w * 4, h * 4),
        }
    }

    #[test]
    fn key_changes_with_mtime_and_size() {
        let p = Path::new("C:/a/b.png");
        let t = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
        let base = key_for(p, t, 10);
        assert_ne!(base, key_for(p, t + Duration::from_micros(1), 10));
        assert_ne!(base, key_for(p, t, 11));
        assert_eq!(base, key_for(p, t, 10));
    }

    #[test]
    fn default_dir_has_no_cache_component() {
        let dir = DiskCache::default_dir();
        assert!(dir.ends_with(Path::new("image-moz").join("thumbs")));
        assert!(dir.components().all(|c| c.as_os_str() != "cache"));
    }

    #[test]
    fn key_is_case_insensitive_on_path() {
        let t = SystemTime::UNIX_EPOCH + Duration::from_secs(5);
        assert_eq!(
            key_for(Path::new("C:/Photos/A.PNG"), t, 1),
            key_for(Path::new("c:/photos/a.png"), t, 1)
        );
    }

    #[test]
    fn cache_roundtrip_unicode_path() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("\u{e9} dossier").join("\u{fc}.png");
        let cache = DiskCache::open(dir.path().join("cache")).unwrap();
        let key = key_for(&src, SystemTime::UNIX_EPOCH, 3);
        let t = thumb(20, 10);
        cache.put(key, &t).unwrap();
        let got = cache.get(key).unwrap();
        assert_eq!(got.image, t.image);
        assert_eq!(got.orig_dims, t.orig_dims);
    }

    #[test]
    fn purge_removes_oldest_first() {
        let dir = tempfile::tempdir().unwrap();
        let cache = DiskCache::open(dir.path().to_path_buf()).unwrap();
        let keys = [ThumbKey(1), ThumbKey(2), ThumbKey(3)];
        let base = SystemTime::now() - Duration::from_secs(1000);
        for (i, k) in keys.iter().enumerate() {
            cache.put(*k, &thumb(16, 16)).unwrap();
            let f = fs::OpenOptions::new()
                .write(true)
                .open(cache.path_for(*k))
                .unwrap();
            f.set_modified(base + Duration::from_secs(i as u64 * 10))
                .unwrap();
        }
        let one = fs::metadata(cache.path_for(keys[0])).unwrap().len();
        let removed = cache.purge_to(one * 2).unwrap();
        assert_eq!(removed, one);
        assert!(!cache.path_for(keys[0]).exists());
        assert!(cache.path_for(keys[1]).exists());
        assert!(cache.path_for(keys[2]).exists());
    }
}
