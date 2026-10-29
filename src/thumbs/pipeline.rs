use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

use crossbeam_channel::{Receiver, Sender};

use super::{
    CACHE_CEILING, DiskCache, THUMB_SIZE, Thumb, ThumbJob, ThumbKey, ThumbOutcome, ThumbResult,
};
use crate::formats;

#[derive(Default)]
struct State {
    wanted: VecDeque<ThumbJob>,
    probe: VecDeque<ThumbJob>,
    in_flight: HashSet<ThumbKey>,
    done: HashSet<ThumbKey>,
    probe_in_flight: HashSet<ThumbKey>,
    probe_done: HashSet<ThumbKey>,
    shutdown: bool,
}

struct Shared {
    state: Mutex<State>,
    cond: Condvar,
}

enum Work {
    Thumb(ThumbJob),
    Probe(ThumbJob),
}

pub struct ThumbPipeline {
    shared: Arc<Shared>,
    results: Receiver<ThumbResult>,
}

impl ThumbPipeline {
    pub fn new(cache: DiskCache, wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            cond: Condvar::new(),
        });
        let (tx, results) = crossbeam_channel::unbounded();
        let threads = thread::available_parallelism()
            .map_or(2, |n| n.get().saturating_sub(1))
            .max(2);
        for _ in 0..threads {
            let (shared, cache, tx, wake) =
                (shared.clone(), cache.clone(), tx.clone(), wake.clone());
            thread::spawn(move || worker(shared, cache, tx, wake));
        }
        thread::spawn(move || {
            if let Err(e) = cache.purge_to(CACHE_CEILING) {
                log::warn!("thumbnail cache purge failed: {e}");
            }
        });
        Self { shared, results }
    }

    /// Replaces the high-priority queue; the first job is the most urgent.
    pub fn set_wanted(&self, jobs: Vec<ThumbJob>) {
        let mut st = self.shared.state.lock().unwrap();
        let jobs: VecDeque<ThumbJob> = jobs
            .into_iter()
            .filter(|j| !st.done.contains(&j.key) && !st.in_flight.contains(&j.key))
            .collect();
        let added = !jobs.is_empty();
        st.wanted = jobs;
        drop(st);
        if added {
            self.shared.cond.notify_all();
        }
    }

    /// Replaces the low-priority queue of header-only dimension probes.
    pub fn set_probe(&self, jobs: Vec<ThumbJob>) {
        let mut st = self.shared.state.lock().unwrap();
        let jobs: VecDeque<ThumbJob> = jobs
            .into_iter()
            .filter(|j| !st.probe_done.contains(&j.key) && !st.probe_in_flight.contains(&j.key))
            .collect();
        let added = !jobs.is_empty();
        st.probe = jobs;
        drop(st);
        if added {
            self.shared.cond.notify_all();
        }
    }

    pub fn try_iter(&self) -> Vec<ThumbResult> {
        self.results.try_iter().collect()
    }
}

impl Drop for ThumbPipeline {
    fn drop(&mut self) {
        self.shared.state.lock().unwrap().shutdown = true;
        self.shared.cond.notify_all();
    }
}

fn next_work(shared: &Shared) -> Option<Work> {
    let mut st = shared.state.lock().unwrap();
    loop {
        if st.shutdown {
            return None;
        }
        if let Some(job) = st.wanted.pop_front() {
            if st.in_flight.insert(job.key) {
                return Some(Work::Thumb(job));
            }
            continue;
        }
        if let Some(job) = st.probe.pop_front() {
            if st.probe_in_flight.insert(job.key) {
                return Some(Work::Probe(job));
            }
            continue;
        }
        st = shared.cond.wait(st).unwrap();
    }
}

fn worker(
    shared: Arc<Shared>,
    cache: DiskCache,
    tx: Sender<ThumbResult>,
    wake: Arc<dyn Fn() + Send + Sync>,
) {
    while let Some(work) = next_work(&shared) {
        let (job, is_probe, outcome) = match work {
            Work::Thumb(job) => {
                let outcome = guarded(&job, || make_thumb(&cache, &job));
                (job, false, outcome)
            }
            Work::Probe(job) => {
                let outcome = guarded(&job, || match formats::dimensions(&job.path) {
                    Ok(dims) => ThumbOutcome::Dims(dims),
                    Err(_) => ThumbOutcome::Failed,
                });
                (job, true, outcome)
            }
        };
        {
            let mut st = shared.state.lock().unwrap();
            if is_probe {
                st.probe_in_flight.remove(&job.key);
                st.probe_done.insert(job.key);
            } else {
                st.in_flight.remove(&job.key);
                st.done.insert(job.key);
            }
        }
        let result = ThumbResult {
            path: job.path,
            key: job.key,
            outcome,
        };
        if tx.send(result).is_err() {
            return;
        }
        wake();
    }
}

/// Decoders can panic on malformed input; a panic must become `Failed`, not a dead worker.
fn guarded(job: &ThumbJob, f: impl FnOnce() -> ThumbOutcome) -> ThumbOutcome {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or_else(|_| {
        log::error!("thumbnail worker panicked on {}", job.path.display());
        ThumbOutcome::Failed
    })
}

fn make_thumb(cache: &DiskCache, job: &ThumbJob) -> ThumbOutcome {
    if let Some(thumb) = cache.get(job.key) {
        return ThumbOutcome::Ready(thumb);
    }
    let decoded = match formats::decode(&job.path) {
        Ok(img) => img,
        Err(e) => {
            log::warn!("thumbnail decode failed for {}: {e}", job.path.display());
            return ThumbOutcome::Failed;
        }
    };
    let orig_dims = decoded.dimensions();
    // `thumbnail` upscales small images, so only call it when shrinking.
    let image = if orig_dims.0.max(orig_dims.1) > THUMB_SIZE {
        image::DynamicImage::ImageRgba8(decoded)
            .thumbnail(THUMB_SIZE, THUMB_SIZE)
            .to_rgba8()
    } else {
        decoded
    };
    let thumb = Thumb { image, orig_dims };
    if let Err(e) = cache.put(job.key, &thumb) {
        log::warn!("thumbnail cache write failed: {e}");
    }
    ThumbOutcome::Ready(thumb)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thumbs::key_for;
    use std::path::Path;
    use std::time::{Duration, Instant};

    fn job(path: &Path) -> ThumbJob {
        let meta = std::fs::metadata(path).unwrap();
        ThumbJob {
            path: path.to_path_buf(),
            key: key_for(path, meta.modified().unwrap(), meta.len()),
        }
    }

    fn pipeline(dir: &Path) -> ThumbPipeline {
        ThumbPipeline::new(DiskCache::open(dir.to_path_buf()).unwrap(), Arc::new(|| {}))
    }

    fn wait_one(p: &ThumbPipeline) -> ThumbResult {
        let start = Instant::now();
        loop {
            if let Some(r) = p.try_iter().into_iter().next() {
                return r;
            }
            assert!(start.elapsed() < Duration::from_secs(20), "timeout");
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn write_png(path: &Path, w: u32, h: u32) {
        image::RgbaImage::from_pixel(w, h, image::Rgba([10, 200, 30, 255]))
            .save(path)
            .unwrap();
    }

    fn ready(r: ThumbResult) -> Thumb {
        match r.outcome {
            ThumbOutcome::Ready(t) => t,
            _ => panic!("expected Ready"),
        }
    }

    #[test]
    fn pipeline_produces_thumb_with_long_side_512() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.png");
        write_png(&src, 2000, 1000);
        let p = pipeline(&dir.path().join("cache"));
        p.set_wanted(vec![job(&src)]);
        let t = ready(wait_one(&p));
        assert_eq!(t.image.dimensions(), (512, 256));
        assert_eq!(t.orig_dims, (2000, 1000));
    }

    #[test]
    fn pipeline_small_image_not_upscaled() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.png");
        write_png(&src, 100, 50);
        let p = pipeline(&dir.path().join("cache"));
        p.set_wanted(vec![job(&src)]);
        assert_eq!(ready(wait_one(&p)).image.dimensions(), (100, 50));
    }

    #[test]
    fn pipeline_failed_on_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("bad.png");
        std::fs::write(&src, b"not an image").unwrap();
        let p = pipeline(&dir.path().join("cache"));
        p.set_wanted(vec![job(&src)]);
        assert!(matches!(wait_one(&p).outcome, ThumbOutcome::Failed));
    }

    #[test]
    fn pipeline_second_request_hits_cache() {
        let dir = tempfile::tempdir().unwrap();
        let cache_dir = dir.path().join("cache");
        let src = dir.path().join("a.png");
        write_png(&src, 300, 300);
        let j = job(&src);
        let p = pipeline(&cache_dir);
        p.set_wanted(vec![ThumbJob {
            path: j.path.clone(),
            key: j.key,
        }]);
        ready(wait_one(&p));
        drop(p);
        std::fs::remove_file(&src).unwrap();
        let p2 = pipeline(&cache_dir);
        p2.set_wanted(vec![j]);
        assert_eq!(ready(wait_one(&p2)).orig_dims, (300, 300));
    }

    #[test]
    fn finished_key_is_not_produced_again() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.png");
        write_png(&src, 64, 64);
        let j = job(&src);
        let p = pipeline(&dir.path().join("cache"));
        p.set_wanted(vec![ThumbJob {
            path: j.path.clone(),
            key: j.key,
        }]);
        wait_one(&p);
        p.set_wanted(vec![j]);
        thread::sleep(Duration::from_millis(200));
        assert!(p.try_iter().is_empty());
    }

    #[test]
    fn panic_in_job_becomes_failed() {
        let job = ThumbJob {
            path: "x".into(),
            key: ThumbKey(1),
        };
        let out = guarded(&job, || panic!("boom"));
        assert!(matches!(out, ThumbOutcome::Failed));
    }

    #[test]
    fn probe_returns_dims() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.png");
        write_png(&src, 123, 45);
        let p = pipeline(&dir.path().join("cache"));
        p.set_probe(vec![job(&src)]);
        assert!(matches!(
            wait_one(&p).outcome,
            ThumbOutcome::Dims((123, 45))
        ));
    }
}
