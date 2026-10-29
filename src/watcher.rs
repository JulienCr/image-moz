use std::collections::HashMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use crossbeam_channel::Sender;
use notify::event::{CreateKind, RemoveKind, RenameMode};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, Debouncer, NoCache, new_debouncer_opt};

use crate::formats::is_supported;
use crate::library::FsChange;

const DEBOUNCE: Duration = Duration::from_millis(200);
const TICK: Duration = Duration::from_millis(100);
const STABLE_FOR: Duration = Duration::from_millis(300);
const ROOT_CHECK: Duration = Duration::from_secs(1);

type Signature = (u64, Option<SystemTime>);

struct Pending {
    signature: Option<Signature>,
    since: Instant,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Hint {
    File,
    Folder,
    Unknown,
}

impl From<CreateKind> for Hint {
    fn from(kind: CreateKind) -> Self {
        match kind {
            CreateKind::File => Hint::File,
            CreateKind::Folder => Hint::Folder,
            _ => Hint::Unknown,
        }
    }
}

impl From<RemoveKind> for Hint {
    fn from(kind: RemoveKind) -> Self {
        match kind {
            RemoveKind::File => Hint::File,
            RemoveKind::Folder => Hint::Folder,
            _ => Hint::Unknown,
        }
    }
}

struct Sink {
    tx: Sender<FsChange>,
    wake: Arc<dyn Fn() + Send + Sync>,
    pending: Mutex<HashMap<PathBuf, Pending>>,
}

impl Sink {
    fn send(&self, change: FsChange) {
        let _ = self.tx.send(change);
        (self.wake)();
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<PathBuf, Pending>> {
        self.pending.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Watches a folder and forwards stabilized changes; dropping it stops watching.
pub struct FolderWatcher {
    _debouncer: Debouncer<RecommendedWatcher, NoCache>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl FolderWatcher {
    /// Paths are emitted as `root` joined with the relative name, never canonicalized.
    pub fn start(
        root: &Path,
        recursive: bool,
        tx: Sender<FsChange>,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> notify::Result<FolderWatcher> {
        let sink = Arc::new(Sink { tx, wake, pending: Mutex::new(HashMap::new()) });
        let handler_sink = Arc::clone(&sink);
        // NoCache: the default file-id cache walks and opens every file, blocking for seconds.
        let mut debouncer = new_debouncer_opt::<_, RecommendedWatcher, NoCache>(
            DEBOUNCE,
            None,
            move |result: DebounceEventResult| handle_batch(&handler_sink, recursive, result),
            NoCache,
            notify::Config::default(),
        )?;
        let mode = if recursive { RecursiveMode::Recursive } else { RecursiveMode::NonRecursive };
        debouncer.watch(root, mode)?;

        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let root = root.to_path_buf();
        let mut root_present = root.is_dir();
        let thread = std::thread::Builder::new()
            .name("watcher-stabilizer".into())
            .spawn(move || {
                let mut last_check = Instant::now();
                while !thread_stop.load(Ordering::Relaxed) {
                    std::thread::sleep(TICK);
                    stabilize(&sink);
                    if last_check.elapsed() >= ROOT_CHECK {
                        last_check = Instant::now();
                        if root_toggled(&root, &mut root_present) {
                            sink.send(FsChange::Rescan);
                        }
                    }
                }
            })
            .map_err(notify::Error::io)?;

        Ok(FolderWatcher { _debouncer: debouncer, stop, thread: Some(thread) })
    }
}

impl Drop for FolderWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The OS watcher dies silently with its root, so existence is polled; true on a change.
fn root_toggled(root: &Path, present: &mut bool) -> bool {
    let now = root.is_dir();
    std::mem::replace(present, now) != now
}

fn handle_batch(sink: &Sink, recursive: bool, result: DebounceEventResult) {
    let mut pending = sink.lock();
    let mut rescan = false;
    match result {
        Ok(events) => {
            for event in &events {
                rescan |= apply_event(sink, &mut pending, recursive, event);
            }
        }
        Err(errors) => {
            log::warn!("watcher errors, rescanning: {errors:?}");
            rescan = true;
        }
    }
    if rescan {
        sink.send(FsChange::Rescan);
    }
}

/// Returns true when the event requires a full rescan.
fn apply_event(
    sink: &Sink,
    pending: &mut HashMap<PathBuf, Pending>,
    recursive: bool,
    event: &Event,
) -> bool {
    if event.need_rescan() {
        return true;
    }
    let mut rescan = false;
    match event.kind {
        EventKind::Create(kind) => {
            for path in &event.paths {
                rescan |= added(pending, path, kind.into(), recursive);
            }
        }
        EventKind::Remove(kind) => {
            for path in &event.paths {
                rescan |= removed(sink, pending, path, kind.into(), recursive);
            }
        }
        EventKind::Modify(notify::event::ModifyKind::Name(mode)) => match (mode, event.paths.as_slice()) {
            (RenameMode::Both, [from, to]) => {
                if recursive && to.is_dir() {
                    rescan = true;
                } else {
                    rescan |= removed(sink, pending, from, Hint::File, recursive);
                    rescan |= added(pending, to, Hint::File, recursive);
                }
            }
            (RenameMode::From, paths) => {
                for path in paths {
                    rescan |= removed(sink, pending, path, Hint::Unknown, recursive);
                }
            }
            (RenameMode::To, paths) => {
                for path in paths {
                    rescan |= added(pending, path, Hint::Unknown, recursive);
                }
            }
            (_, paths) => {
                for path in paths {
                    rescan |= if path.exists() {
                        added(pending, path, Hint::Unknown, recursive)
                    } else {
                        removed(sink, pending, path, Hint::Unknown, recursive)
                    };
                }
            }
        },
        EventKind::Modify(notify::event::ModifyKind::Metadata(_)) => {}
        EventKind::Modify(_) => {
            for path in &event.paths {
                if is_supported(path) {
                    touch(pending, path);
                }
            }
        }
        _ => {}
    }
    rescan
}

fn touch(pending: &mut HashMap<PathBuf, Pending>, path: &Path) {
    pending.insert(path.to_path_buf(), Pending { signature: None, since: Instant::now() });
}

/// Returns true when a directory appeared that needs a rescan.
fn added(pending: &mut HashMap<PathBuf, Pending>, path: &Path, hint: Hint, recursive: bool) -> bool {
    if is_supported(path) {
        touch(pending, path);
        return false;
    }
    recursive && (hint == Hint::Folder || (hint == Hint::Unknown && path.is_dir()))
}

/// Returns true when a directory vanished: its children get no individual events.
fn removed(
    sink: &Sink,
    pending: &mut HashMap<PathBuf, Pending>,
    path: &Path,
    hint: Hint,
    recursive: bool,
) -> bool {
    pending.remove(path);
    if is_supported(path) {
        sink.send(FsChange::Remove(path.to_path_buf()));
        return false;
    }
    recursive && hint != Hint::File
}

fn stabilize(sink: &Sink) {
    let mut pending = sink.lock();
    if pending.is_empty() {
        return;
    }
    let now = Instant::now();
    let mut ready = Vec::new();
    pending.retain(|path, entry| {
        let Ok(meta) = fs::metadata(path) else { return false };
        if !meta.is_file() {
            return false;
        }
        let signature = (meta.len(), meta.modified().ok());
        if entry.signature != Some(signature) {
            entry.signature = Some(signature);
            entry.since = now;
            return true;
        }
        if now.duration_since(entry.since) < STABLE_FOR || File::open(path).is_err() {
            return true;
        }
        ready.push(path.clone());
        false
    });
    for path in ready {
        sink.send(FsChange::Upsert(path));
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use crossbeam_channel::{Receiver, RecvTimeoutError};

    use super::*;

    const WAIT: Duration = Duration::from_secs(5);

    fn png_bytes(seed: u8) -> Vec<u8> {
        let img = image::RgbaImage::from_fn(64, 64, |x, y| image::Rgba([seed, x as u8, y as u8, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    fn write_png(path: &Path, seed: u8) {
        fs::write(path, png_bytes(seed)).unwrap();
    }

    fn start(dir: &Path, recursive: bool) -> (FolderWatcher, Receiver<FsChange>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let watcher = FolderWatcher::start(dir, recursive, tx, Arc::new(|| {})).unwrap();
        (watcher, rx)
    }

    fn next(rx: &Receiver<FsChange>) -> FsChange {
        rx.recv_timeout(WAIT).expect("expected a change within 5s")
    }

    #[test]
    fn new_file_emits_upsert() {
        let dir = tempfile::tempdir().unwrap();
        let (_w, rx) = start(dir.path(), false);
        let p = dir.path().join("a.png");
        write_png(&p, 1);
        assert_eq!(next(&rx), FsChange::Upsert(p));
    }

    #[test]
    fn delete_emits_remove() {
        let dir = tempfile::tempdir().unwrap();
        let (_w, rx) = start(dir.path(), false);
        let p = dir.path().join("a.png");
        write_png(&p, 1);
        assert_eq!(next(&rx), FsChange::Upsert(p.clone()));
        fs::remove_file(&p).unwrap();
        assert_eq!(next(&rx), FsChange::Remove(p));
    }

    #[test]
    fn rename_emits_remove_then_upsert() {
        let dir = tempfile::tempdir().unwrap();
        let (_w, rx) = start(dir.path(), false);
        let a = dir.path().join("a.png");
        let b = dir.path().join("b.png");
        write_png(&a, 1);
        assert_eq!(next(&rx), FsChange::Upsert(a.clone()));
        fs::rename(&a, &b).unwrap();
        assert_eq!(next(&rx), FsChange::Remove(a));
        assert_eq!(next(&rx), FsChange::Upsert(b));
    }

    #[test]
    fn non_image_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let (_w, rx) = start(dir.path(), false);
        fs::write(dir.path().join("a.txt"), b"hello").unwrap();
        let b = dir.path().join("b.png");
        write_png(&b, 2);
        assert_eq!(next(&rx), FsChange::Upsert(b));
    }

    #[test]
    fn slow_writer_emits_once_after_stable() {
        let dir = tempfile::tempdir().unwrap();
        let (_w, rx) = start(dir.path(), false);
        let p = dir.path().join("slow.png");
        let bytes = png_bytes(3);
        let mut file = File::create(&p).unwrap();
        for chunk in bytes.chunks(bytes.len().div_ceil(4)) {
            file.write_all(chunk).unwrap();
            file.flush().unwrap();
            std::thread::sleep(Duration::from_millis(150));
        }
        let last_chunk_at = Instant::now() - Duration::from_millis(150);
        assert_eq!(next(&rx), FsChange::Upsert(p.clone()));
        assert!(last_chunk_at.elapsed() >= STABLE_FOR, "emitted before the file was stable");
        assert!(crate::formats::decode(&p).is_ok());
        assert_eq!(rx.recv_timeout(Duration::from_secs(1)), Err(RecvTimeoutError::Timeout));
    }

    #[test]
    fn directory_create_and_remove_rescan_when_recursive() {
        let dir = tempfile::tempdir().unwrap();
        let (_w, rx) = start(dir.path(), true);
        let sub = dir.path().join("sub");
        fs::create_dir(&sub).unwrap();
        assert_eq!(next(&rx), FsChange::Rescan);
        fs::remove_dir(&sub).unwrap();
        assert_eq!(next(&rx), FsChange::Rescan);
    }

    #[test]
    fn root_vanishing_and_reappearing_rescans() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("watched");
        fs::create_dir(&root).unwrap();
        let (_w, rx) = start(&root, false);
        fs::remove_dir(&root).unwrap();
        assert_eq!(next(&rx), FsChange::Rescan);
        fs::create_dir(&root).unwrap();
        assert_eq!(next(&rx), FsChange::Rescan);
    }

    #[test]
    fn root_toggled_reports_each_transition_once() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("r");
        let mut present = false;
        assert!(!root_toggled(&root, &mut present));
        fs::create_dir(&root).unwrap();
        assert!(root_toggled(&root, &mut present));
        assert!(!root_toggled(&root, &mut present));
        fs::remove_dir(&root).unwrap();
        assert!(root_toggled(&root, &mut present));
    }

    #[test]
    fn burst_of_200_files_all_reported_once() {
        let dir = tempfile::tempdir().unwrap();
        let (_w, rx) = start(dir.path(), false);
        let expected: std::collections::HashSet<PathBuf> =
            (0..200).map(|i| dir.path().join(format!("img{i:03}.png"))).collect();
        for (i, p) in expected.iter().enumerate() {
            write_png(p, i as u8);
        }
        let mut seen = std::collections::HashSet::new();
        let deadline = Instant::now() + Duration::from_secs(10);
        while seen.len() < expected.len() {
            let left = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(left) {
                Ok(FsChange::Upsert(p)) => assert!(seen.insert(p.clone()), "duplicate {p:?}"),
                Ok(other) => panic!("unexpected {other:?}"),
                Err(_) => break,
            }
        }
        assert_eq!(seen, expected);
        assert_eq!(rx.recv_timeout(Duration::from_secs(1)), Err(RecvTimeoutError::Timeout));
    }
}
