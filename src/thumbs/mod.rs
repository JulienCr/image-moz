pub mod cache;
pub mod pipeline;

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub use cache::DiskCache;
pub use pipeline::ThumbPipeline;

pub const THUMB_SIZE: u32 = 512;
pub const CACHE_CEILING: u64 = 1 << 30;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ThumbKey(pub u64);

/// Stable cache key: FNV-1a 64 over the lowercased path, mtime nanos and size.
/// Implemented inline because `DefaultHasher` is not stable across Rust versions.
pub fn key_for(path: &Path, mtime: SystemTime, size: u64) -> ThumbKey {
    const OFFSET: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;
    let nanos = mtime
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let lowered = path.to_string_lossy().to_lowercase();
    let mut hash = OFFSET;
    for byte in lowered
        .as_bytes()
        .iter()
        .chain(nanos.to_le_bytes().iter())
        .chain(size.to_le_bytes().iter())
    {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    ThumbKey(hash)
}

pub struct Thumb {
    /// Long side at most `THUMB_SIZE`.
    pub image: image::RgbaImage,
    pub orig_dims: (u32, u32),
}

#[derive(Clone)]
pub struct ThumbJob {
    pub path: PathBuf,
    pub key: ThumbKey,
}

pub enum ThumbOutcome {
    Ready(Thumb),
    Dims((u32, u32)),
    Failed,
}

pub struct ThumbResult {
    pub path: PathBuf,
    pub key: ThumbKey,
    pub outcome: ThumbOutcome,
}
