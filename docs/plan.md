# image-moz Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A native Windows image viewer that shows a folder as a live-updating, virtualized thumbnail gallery with three layouts, keyboard selection, dual-format clipboard copy and a full-frame preview.

**Architecture:** One crate, `lib` + thin `bin`. Pure, unit-tested modules (formats, scan/library model, layouts, selection, config, preview math) sit under an egui front-end (`app` + `ui/*`). Background work is on plain threads talking to the UI over `crossbeam-channel`: a filesystem watcher thread and a thumbnail worker pool backed by a disk cache. The UI thread never decodes an image.

**Tech Stack:** Rust stable msvc (cargo 1.92), `eframe`/`egui`, `image` (png, jpeg, webp, bmp, gif, tiff, exr, hdr, qoi), `notify` + `notify-debouncer-full`, `rfd`, `windows` (Win32 clipboard, shell), `serde` + `serde_json`, `directories`, `crossbeam-channel`, `log` + `env_logger`, `tempfile` (dev). Versions: latest via `cargo add`.

**Spec:** `docs/spec.md` — the contract. Do not re-open its decisions.

## Global Constraints

- Build and run with Windows `cargo.exe`, toolchain `stable-x86_64-pc-windows-msvc`. Never from WSL.
- Code, identifiers, comments, commit messages in English. **UI strings in French** (« Ouvrir », « Récursif », « Afficher dans l'Explorateur », …).
- Comments: ≤ 3 lines per comment, ≤ 10 per docstring; only a "why" the code can't show.
- Read-only app: no delete, rename or move of user files, ever.
- Supported formats: PNG, JPEG, WebP, BMP, GIF (first frame), TIFF, EXR, HDR. Extension match is case-insensitive.
- EXR/HDR: fixed ACES tone-mapping (Narkowicz fit `x(2.51x+0.03)/(x(2.43x+0.59)+0.14)`), exposure 0 (multiplier 1.0), then linear→sRGB encode. No UI setting.
- Thumbnails: 512 px on the long side, decoded off the UI thread.
- Thumbnail cache: `%LOCALAPPDATA%\image-moz\thumbs`, key = absolute path + mtime + file size, ceiling 1 GiB (1 073 741 824 bytes), LRU purge at startup.
- Config: `%APPDATA%\image-moz\config.json`.
- Windows paths are case-insensitive: any path used as a map key (config folder key, pins) is normalized with `to_lowercase()` on its string form.
- Logging: `log` macros; `env_logger` initialized in `main`, enabled with `RUST_LOG=image_moz=debug`. Release build uses `#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]`.
- Commit after each task, only that task's files (`git add <paths>`, never `git add -A`). Commit message ends with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- `cargo test` and `cargo clippy --all-targets` (no new warnings) pass before each commit.

## Review Focus

1. **Partially written / zero-byte / locked file when the event fires** (Blender writes PNGs progressively) — no panic, tile shows an error state, retried on the next modify event. Tests: Task 1 (`decode_truncated_png_is_err`, `decode_zero_byte_is_err`), Task 2 (`modify_clears_error`), Task 3 (`slow_writer_emits_once_after_stable`).
2. **Burst of hundreds of files at once** (render sequence) — every file appears exactly once, UI stays responsive. Test: Task 3 (`burst_of_200_files_all_reported_once`).
3. **Non-ASCII and spaces in paths** (`D:\Téléchargements\mes rendus\é.png`) — scan, cache key, CF_HDROP, explorer `/select,` all work. Tests: Task 4 (`cache_roundtrip_unicode_path`), Task 8 (`copy_files_roundtrip_unicode`).
4. **Last folder gone / inaccessible at startup, or the open folder is deleted while open** — app starts on an empty state with « Dossier introuvable », no panic. Tests: Task 2 (`scan_missing_dir_is_err`), Task 6 (`load_corrupt_config_falls_back_to_default`); manual check in Task 12.
5. **Case-only differences in paths** (`Render.PNG` vs `render.png`, folder opened as `d:\Dev` vs `D:\dev`) — pins and collapsed sections still match. Test: Task 6 (`folder_key_is_case_insensitive`).

---

## File Structure

```
Cargo.toml
src/main.rs            bin: CLI arg, env_logger, eframe::run_native
src/lib.rs             module declarations
src/formats.rs         supported extensions, full decode (incl. ACES), header-only dimensions
src/library.rs         Entry, scan(), Library model: apply changes, sort, sections
src/watcher.rs         notify-debouncer-full + write-stabilization → FsChange channel
src/thumbs/mod.rs      re-exports
src/thumbs/cache.rs    disk cache: key, read/write, LRU purge
src/thumbs/pipeline.rs worker pool, wanted-list priority, results channel
src/layout.rs          grid / justified / bento placement, geometric navigation
src/selection.rs       click / ctrl / shift / arrow selection model
src/config.rs          Config, FolderState, load/save
src/platform.rs        Win32 clipboard (CF_HDROP + CF_DIB + "PNG"), paths text, open default app, explorer /select
src/preview.rs         ViewTransform math (fit, zoom at cursor, pan)
src/app.rs             eframe::App: state, wiring, keyboard
src/ui/toolbar.rs      toolbar widgets
src/ui/gallery.rs      virtualized sections + tiles drawing, hover, highlight, scroll anchoring
src/ui/status.rs       status bar
src/ui/preview_view.rs full-frame preview drawing + input
```

---

### Task 1: Scaffold + formats

**Files:**
- Create: `Cargo.toml`, `.gitignore` (`/target`), `src/main.rs`, `src/lib.rs`, `src/formats.rs`, and **empty stub files** for every other module in the File Structure (each `lib.rs` `mod` must compile, so later tasks only edit their own file).

**Interfaces:**
- Produces:
  - `pub fn is_supported(path: &Path) -> bool`
  - `pub fn decode(path: &Path) -> Result<image::RgbaImage, DecodeError>` — full-resolution, 8-bit sRGB; EXR/HDR tone-mapped; GIF first frame.
  - `pub fn dimensions(path: &Path) -> Result<(u32, u32), DecodeError>` — header only (`image::image_dimensions`).
  - `pub fn aces_srgb(linear: f32) -> u8` (exposed for tests).
  - `#[derive(Debug, thiserror::Error)] pub enum DecodeError { Io(std::io::Error), Image(image::ImageError), Unsupported }` (add `thiserror`).
- `Cargo.toml` declares **all** dependencies of the Tech Stack now, so no later task edits it (exception: a task that truly needs a new feature flag says so in its report).

- [ ] **Step 1: Write failing tests in `src/formats.rs`** (`#[cfg(test)]`, images generated into `tempfile::tempdir()`):
  - `supported_extensions`: `a.png a.JPG a.jpeg a.webp a.bmp a.gif a.tif a.tiff a.exr a.hdr` → true; `a.txt a.psd noext` → false.
  - `decode_png_roundtrip`: write 3×2 RGBA png, `decode` returns 3×2 with same pixels.
  - `decode_truncated_png_is_err`: first half of a valid png's bytes → `Err`.
  - `decode_zero_byte_is_err`.
  - `aces_values`: `aces_srgb(0.0) == 0`; `aces_srgb(0.18)` within 1 of `141`; `aces_srgb(100.0) == 255`.
  - `decode_exr_tonemaps`: write a 1×1 Rgba32F EXR of value 0.18 → decoded R within 1 of 141, alpha 255.
  - `dimensions_reads_header`: 640×480 png → `(640, 480)`.
- [ ] **Step 2:** `cargo test formats` → FAIL (not implemented).
- [ ] **Step 3: Implement.** HDR/EXR detected by extension, decoded as `Rgba32F`, each RGB channel through `aces_srgb`, alpha clamped. Everything else `image::open(..)?.to_rgba8()`. `main.rs`: opens an empty eframe window titled `image-moz`.
- [ ] **Step 4:** `cargo test` PASS; `cargo run` opens a window (close it).
- [ ] **Step 5: Commit** `feat: scaffold crate and image format decoding`.

---

### Task 2: Scan + library model

**Files:** Modify `src/library.rs`.

**Interfaces:**
- Consumes: `formats::is_supported`.
- Produces:
  - `pub struct Entry { pub path: PathBuf, pub rel_dir: String /* "" = root, '/'-separated */, pub name: String, pub mtime: SystemTime, pub size: u64, pub dims: Option<(u32,u32)>, pub error: bool, pub arrived_at: Option<Instant> }`
  - `pub fn scan(root: &Path, recursive: bool) -> io::Result<Vec<Entry>>` (missing root → `Err`; unreadable subdir skipped with `log::warn!`).
  - `#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)] pub enum SortOrder { #[default] DateDesc, DateAsc, NameAsc, NameDesc }`
  - `pub enum FsChange { Upsert(PathBuf), Remove(PathBuf), Rescan }` (defined here, used by the watcher).
  - `pub struct Library` with `new(root, recursive)`, `replace_all(Vec<Entry>)`, `apply(&mut self, change: &FsChange, now: Instant) -> bool` (true = a new entry was added), `get(&Path) -> Option<&Entry>`, `get_mut`, `len()`, `sections(&self, sort: SortOrder) -> Vec<Section>`.
  - `pub struct Section { pub rel_dir: String, pub items: Vec<PathBuf> }`
- Rules: `Upsert` of unknown path stats it, sets `arrived_at = Some(now)`, returns true; `Upsert` of known path refreshes mtime/size, resets `dims = None`, `error = false`, returns false. `Remove` drops it. Paths outside root, or in subdirs when not recursive, are ignored. `Rescan` is handled by the caller (returns false). Name sort is case-insensitive. Sections: only non-empty ones; root first, then subdirs ordered by sort: date → section's max item mtime, name → `rel_dir`.

- [ ] **Step 1: Failing tests:** `scan_flat_ignores_subdirs_and_non_images`, `scan_recursive_sets_rel_dir` (`sub/deep/x.png` → `"sub/deep"`), `scan_missing_dir_is_err`, `sort_date_desc_default`, `sort_name_case_insensitive`, `sections_root_first_then_by_latest_mtime` (in DateDesc, subdir receiving the newest image comes right after root), `upsert_new_returns_true_and_marks_arrival`, `modify_clears_error` (set `error = true`, Upsert same path → `error == false`, returns false), `remove_drops_entry`, `upsert_in_subdir_ignored_when_not_recursive`. Set mtimes with `File::set_modified`.
- [ ] **Step 2:** `cargo test library` → FAIL.
- [ ] **Step 3:** Implement (`HashMap<PathBuf, Entry>`).
- [ ] **Step 4:** `cargo test` PASS.
- [ ] **Step 5: Commit** `feat: folder scan and library model`.

---

### Task 3: Folder watcher with write stabilization

**Files:** Modify `src/watcher.rs`.

**Interfaces:**
- Consumes: `library::FsChange`, `formats::is_supported`.
- Produces: `pub struct FolderWatcher` (drop = stop) with `pub fn start(root: &Path, recursive: bool, tx: crossbeam_channel::Sender<FsChange>, wake: Arc<dyn Fn() + Send + Sync>) -> notify::Result<FolderWatcher>`.
- Behavior: `notify-debouncer-full` at 200 ms. Create/modify of a supported file goes into a pending map; a stabilization thread ticking every 100 ms emits `Upsert` once `(len, mtime)` is unchanged for **300 ms** and the file opens for reading. Remove → `Remove` immediately (and drop from pending). Rename → `Remove(old)` + pending `new`. Directory created/removed/renamed in recursive mode, or a debouncer error / overflow → `Rescan`. Every send calls `wake()`.

- [ ] **Step 1: Failing tests** (real temp dirs, generous timeouts via `recv_timeout(Duration::from_secs(5))`):
  - `new_file_emits_upsert`, `delete_emits_remove`, `rename_emits_remove_then_upsert`, `non_image_ignored` (write `a.txt` then `b.png`; first message is `b.png`).
  - `slow_writer_emits_once_after_stable`: write a png in 4 chunks 150 ms apart; exactly one `Upsert`, received after the last chunk, and that file decodes OK.
  - `burst_of_200_files_all_reported_once`: write 200 pngs as fast as possible; collect for up to 10 s; the set of `Upsert` paths equals the 200 paths, no duplicates.
- [ ] **Step 2:** `cargo test watcher` → FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** `cargo test watcher` PASS three runs in a row (flakiness check).
- [ ] **Step 5: Commit** `feat: debounced folder watcher`.

---

### Task 4: Thumbnail disk cache + pipeline

**Files:** Modify `src/thumbs/mod.rs`, `src/thumbs/cache.rs`, `src/thumbs/pipeline.rs`.

**Interfaces:**
- Consumes: `formats::decode`, `formats::dimensions`.
- Produces:
  - `pub const THUMB_SIZE: u32 = 512; pub const CACHE_CEILING: u64 = 1 << 30;`
  - `#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)] pub struct ThumbKey(pub u64)`; `pub fn key_for(path: &Path, mtime: SystemTime, size: u64) -> ThumbKey` — FNV-1a 64 over lowercased path string bytes + mtime nanos since epoch (LE) + size (LE). Implement FNV inline (don't use `DefaultHasher`: unstable across Rust versions).
  - `pub struct DiskCache { dir: PathBuf }`: `open(dir) -> io::Result<Self>`, `default_dir() -> PathBuf` (`directories` → `%LOCALAPPDATA%\image-moz\thumbs`), `get(key) -> Option<Thumb>` (touches file mtime for LRU), `put(key, &Thumb) -> io::Result<()>` (write tmp + rename), `purge_to(ceiling: u64) -> io::Result<u64 /*bytes removed*/>` (oldest mtime first).
  - `pub struct Thumb { pub image: image::RgbaImage /* long side ≤ 512 */, pub orig_dims: (u32, u32) }`. File format: `b"IMZ1"`, `u32 LE` w, `u32 LE` h, then QOI bytes. File name `{key:016x}.imz`.
  - `pub struct ThumbJob { pub path: PathBuf, pub key: ThumbKey }`
  - `pub enum ThumbOutcome { Ready(Thumb), Dims((u32,u32)), Failed }` ; `pub struct ThumbResult { pub path: PathBuf, pub key: ThumbKey, pub outcome: ThumbOutcome }`
  - `pub struct ThumbPipeline`: `new(cache: DiskCache, wake: Arc<dyn Fn()+Send+Sync>) -> Self` (threads = `available_parallelism() - 1`, min 2; starts a background `purge_to(CACHE_CEILING)`), `set_wanted(&self, jobs: Vec<ThumbJob>)` (replaces the high-priority queue; first = most urgent; jobs already done or in flight skipped), `set_probe(&self, jobs: Vec<ThumbJob>)` (low-priority dims-only queue → `Dims`), `try_iter(&self) -> Vec<ThumbResult>`.
  - Thumbnail = `DynamicImage::thumbnail(512, 512)` of the decoded image (never upscale).

- [ ] **Step 1: Failing tests:** `key_changes_with_mtime_and_size`, `key_is_case_insensitive_on_path`, `cache_roundtrip_unicode_path` (source in `tempdir/é dossier/ü.png`, put/get equal pixels & dims), `purge_removes_oldest_first` (3 entries, set mtimes, ceiling fits 2 → oldest gone), `pipeline_produces_thumb_with_long_side_512` (2000×1000 source → 512×256, `orig_dims (2000,1000)`), `pipeline_small_image_not_upscaled` (100×50 stays 100×50), `pipeline_failed_on_corrupt` (`Failed`, no panic), `pipeline_second_request_hits_cache` (delete source after first result; second pipeline on same cache dir still returns `Ready`), `probe_returns_dims`.
- [ ] **Step 2:** `cargo test thumbs` → FAIL.
- [ ] **Step 3:** Implement (`Mutex<VecDeque>` + `Condvar` or crossbeam; results over a crossbeam channel).
- [ ] **Step 4:** `cargo test` PASS.
- [ ] **Step 5: Commit** `feat: thumbnail pipeline with disk cache`.

---

### Task 5: Layouts + geometric navigation

**Files:** Modify `src/layout.rs`.

**Interfaces:**
- Produces (uses `egui::{Rect, Pos2, Vec2}`):
  - `#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)] pub enum LayoutKind { #[default] Grid, Justified, Bento }`
  - `#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)] pub enum BentoRule { #[default] Recent, Pinned, Resolution }`
  - `pub struct LayoutItem { pub id: usize, pub aspect: f32 /* w/h, 1.0 if unknown */, pub mtime: SystemTime, pub pixels: u64, pub pinned: bool }`
  - `pub struct LayoutSection { pub header: Option<String> /* None = no header row */, pub collapsed: bool, pub items: Vec<LayoutItem> /* in sort order */ }`
  - `pub struct Placed { pub id: usize, pub rect: Rect }` ; `pub struct HeaderRect { pub section: usize, pub rect: Rect }`
  - `pub struct Layout { pub tiles: Vec<Placed>, pub headers: Vec<HeaderRect>, pub height: f32 }`
  - `pub fn compute(kind: LayoutKind, rule: BentoRule, sections: &[LayoutSection], width: f32, target: f32) -> Layout`
  - `pub enum Dir { Left, Right, Up, Down }` ; `pub fn neighbor(layout: &Layout, from: usize /* id */, dir: Dir) -> Option<usize>`
- Constants: `GAP = 4.0`, `HEADER_H = 28.0`. Collapsed section → header only, no tiles. Coordinates relative to content top-left, sections stacked vertically.
- **Grid:** columns = `max(1, floor((width+GAP)/(target+GAP)))`, square side stretched so columns fill `width`.
- **Justified:** row height `target`; add items while `Σ aspect·target + gaps < width`; when a row overflows, scale it so it fills `width` exactly; last row keeps height `target`, left aligned. A single item wider than `width` alone fills the row.
- **Bento:** columns = `max(2, floor((width+GAP)/(target+GAP)))`, cell = stretched like Grid. Slot `k` (0-based within the section) is big (2×2) when `k % 7 == 0`; other slots are `2×1` if aspect ≥ 1 else `1×2`. Big slots take the next item from the featured queue — `Recent`: by mtime desc; `Resolution`: by pixels desc; `Pinned`: pinned items in sort order, then (when exhausted) the next item in sort order. Other slots take the next not-yet-placed item in sort order. Placement is dense first-fit on an occupancy grid (scan rows top→bottom, columns left→right); a shape that fits nowhere in the current row band is shrunk to 1×1 rather than leaving the section ragged forever. The pattern restarts per section.
- **neighbor:** candidates whose center lies strictly in `dir` from `from`'s center; score = primary-axis distance + 2 × cross-axis distance; lowest wins. Works across sections since coordinates are global.

- [ ] **Step 1: Failing tests:** `grid_columns_and_fill` (width 1000, target 200 → 4 columns, side `(1000-3*4)/4`), `grid_collapsed_section_header_only`, `justified_rows_fill_width` (every non-last row's right edge = width ±0.5), `justified_last_row_keeps_target_height`, `bento_first_tile_is_2x2_per_section`, `bento_landscape_2x1_portrait_1x2`, `bento_no_overlap_and_within_width` (random aspects, 200 items), `bento_rule_resolution_puts_largest_in_big_slot`, `bento_rule_pinned_puts_pinned_in_big_slot`, `neighbor_grid_arrows` (4-col grid: right of 0 is 1, down of 1 is 5, left of 0 is None), `neighbor_crosses_section_boundary`.
- [ ] **Step 2:** `cargo test layout` → FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** `cargo test` PASS.
- [ ] **Step 5: Commit** `feat: grid, justified and bento layouts`.

---

### Task 6: Config persistence

**Files:** Modify `src/config.rs`.

**Interfaces:**
- Consumes: `library::SortOrder`, `layout::{LayoutKind, BentoRule}` (if those tasks aren't merged yet, the types are defined exactly as in their Interfaces blocks).
- Produces:
  - `#[derive(Serialize, Deserialize, Default)] #[serde(default)] pub struct FolderState { pub pins: BTreeSet<String> /* paths relative to folder, lowercased, '/'-separated */, pub collapsed: BTreeSet<String> /* rel_dir */ }`
  - `#[derive(Serialize, Deserialize)] #[serde(default)] pub struct Config { pub last_folder: Option<PathBuf>, pub layout: LayoutKind, pub target_size: f32 /* default 192, clamp 64..=512 */, pub sort: SortOrder, pub recursive: bool /* false */, pub bento_rule: BentoRule, pub folders: BTreeMap<String, FolderState> }`
  - `pub fn folder_key(folder: &Path) -> String` (absolute, lowercased, trailing separator trimmed)
  - `pub fn rel_key(folder: &Path, file: &Path) -> Option<String>`
  - `impl Config { pub fn default_path() -> PathBuf; pub fn load(path: &Path) -> Config /* missing or corrupt → default, log::warn */; pub fn save(&self, path: &Path) -> io::Result<()> /* create dir, write tmp, rename */; pub fn folder(&self, folder: &Path) -> Option<&FolderState>; pub fn folder_mut(&mut self, folder: &Path) -> &mut FolderState; pub fn toggle_pin(&mut self, folder: &Path, file: &Path) -> bool /* new state */; pub fn is_pinned(&self, folder: &Path, file: &Path) -> bool }`
  - Constants `MIN_SIZE = 64.0`, `MAX_SIZE = 512.0`, `DEFAULT_SIZE = 192.0`.

- [ ] **Step 1: Failing tests:** `roundtrip_save_load`, `load_missing_is_default`, `load_corrupt_config_falls_back_to_default`, `unknown_and_missing_fields_tolerated` (`{"recursive":true,"future":1}` loads), `folder_key_is_case_insensitive` (`D:\Dev\X` and `d:\dev\x\` same key; pin toggled via one is seen via the other, including file `Render.PNG` vs `render.png`), `target_size_clamped_on_load`.
- [ ] **Step 2:** `cargo test config` → FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** `cargo test` PASS.
- [ ] **Step 5: Commit** `feat: persisted configuration`.

---

### Task 7: Selection model

**Files:** Modify `src/selection.rs`.

**Interfaces:**
- Produces: `pub struct Selection { selected: BTreeSet<usize>, anchor: Option<usize>, focus: Option<usize> }` working on **positions in display order** (`usize` index into the flat list of visible tiles, layout order):
  - `click(&mut self, idx)`: selection = {idx}, anchor = focus = idx.
  - `ctrl_click(&mut self, idx)`: toggle idx, anchor = focus = idx.
  - `shift_click(&mut self, idx)`: selection = range anchor..=idx (anchor unchanged; no anchor → like click), focus = idx.
  - `move_to(&mut self, idx, extend: bool)`: arrow-key result; `extend` (Shift+arrow) behaves like shift_click, else like click.
  - `focus() -> Option<usize>`, `selected() -> impl Iterator<Item=usize>`, `len()`, `contains(idx)`, `clear()`.
  - `remap(&mut self, map: impl Fn(usize) -> Option<usize>)`: after the list changes (insert/delete/sort), translate indices; dropped ones vanish; focus falls back to the nearest surviving selected index or None.
- The app translates paths ↔ positions; storing positions keeps shift-ranges in visual order.

- [ ] **Step 1: Failing tests:** `click_selects_one`, `ctrl_click_toggles`, `shift_click_range_from_anchor`, `shift_click_backwards`, `move_extend_keeps_anchor`, `remap_drops_deleted_and_shifts`.
- [ ] **Step 2:** `cargo test selection` → FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** `cargo test` PASS.
- [ ] **Step 5: Commit** `feat: selection model`.

---

### Task 8: Platform: clipboard + shell

**Files:** Modify `src/platform.rs`.

**Interfaces:**
- Produces:
  - `pub fn copy_files_and_bitmap(files: &[PathBuf], bitmap: Option<&image::RgbaImage>) -> Result<(), PlatformError>` — **one** `OpenClipboard` / `EmptyClipboard`, then `SetClipboardData` for `CF_HDROP` (`DROPFILES` with `fWide = 1`, double-NUL-terminated UTF-16 list), and if `bitmap` is Some: `CF_DIBV5` (32-bit BGRA, bottom-up, with alpha mask) **and** registered format `"PNG"` (PNG bytes; Chromium/Discord read it), then `CloseClipboard`. Retry `OpenClipboard` up to 10× 20 ms (another app may hold it).
  - `pub fn copy_text(text: &str) -> Result<(), PlatformError>` (`CF_UNICODETEXT`). Paths joined with `"\r\n"`.
  - `pub fn read_file_list() -> Result<Vec<PathBuf>, PlatformError>` (test helper, `DragQueryFileW`).
  - `pub fn has_format(name_or_id) -> bool` style helper for tests: `pub fn clipboard_has_png() -> bool`, `pub fn clipboard_has_dib() -> bool`.
  - `pub fn open_default(path: &Path)` (`ShellExecuteW` "open"), `pub fn reveal_in_explorer(path: &Path)` (`explorer.exe /select,"<path>"` via `std::process::Command` with `raw_arg` so the quoting survives).
  - `PlatformError` via `thiserror`.
- Use the `windows` crate (features `Win32_System_DataExchange`, `Win32_System_Memory`, `Win32_UI_Shell`, `Win32_Graphics_Gdi`, `Win32_Foundation`, `Win32_System_Ole`). This is the one task allowed to add those features to `Cargo.toml`.

- [ ] **Step 1: Failing tests** (single `#[test]` function `clipboard_roundtrip` run sequentially, since the clipboard is global): copy two files under `tempdir/é dossier/` with a 4×4 bitmap → `read_file_list()` equals the two paths, `clipboard_has_png()` and `clipboard_has_dib()` true; then `copy_text("a\r\nb")` and read `CF_UNICODETEXT` back equal; then `copy_files_and_bitmap(&[one], None)` → file list ok and `clipboard_has_png()` false.
- [ ] **Step 2:** `cargo test platform -- --test-threads=1` → FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** PASS. Manual: paste in Explorer and in Paint once (note result in report).
- [ ] **Step 5: Commit** `feat: Windows clipboard and shell helpers`.

---

### Task 9: App shell + gallery

**Files:** Modify `src/main.rs`, `src/app.rs`, `src/ui/toolbar.rs`, `src/ui/gallery.rs`, `src/ui/status.rs`.

**Interfaces:**
- Consumes: everything from Tasks 1–6.
- Produces: `pub struct ImageMozApp` (`eframe::App`), `ImageMozApp::new(cc: &eframe::CreationContext, cli_folder: Option<PathBuf>) -> Self`; `ImageMozApp::visible_order(&self) -> &[PathBuf]` (flat display order, layout order, used by Tasks 10–11); `ImageMozApp::layout(&self) -> &layout::Layout`; fields Tasks 10–11 extend: `selection: Selection`, `preview: Option<PreviewState>` (Task 11 defines the type; this task leaves the field out).
- Behavior to deliver:
  - **Folder source**: CLI arg > `config.last_folder`. Toolbar « Ouvrir » (`rfd::FileDialog::pick_folder`), and dropping a folder on the window (`ctx.input(|i| i.raw.dropped_files)`; a dropped file opens its parent). Missing folder → centered « Dossier introuvable : <chemin> » + « Ouvrir ». Opening a folder: `scan` on a background thread, restart `FolderWatcher`, save `last_folder`. Window title `image-moz — <folder>`.
  - **Toolbar** (French labels): « Ouvrir », « Récursif » checkbox, layout selector « Grille / Justifié / Bento » (+ keys `1` `2` `3`), sort combo « Date ↓ / Date ↑ / Nom A→Z / Nom Z→A », bento rule combo « Récentes / Épinglées / Plus grandes » (visible in Bento only), size slider `64..=512`. `Ctrl+wheel`, `Ctrl+=`, `Ctrl++`, `Ctrl+-`: ±16 px clamped. Every change saved to config (save at most once per second + on exit).
  - **Gallery**: `ScrollArea::vertical`; layout recomputed when width, sections, dims, size, layout kind or rule change (cache by a cheap signature). Only tiles intersecting the viewport ± one screen are drawn and sent to `set_wanted` (viewport first, then the margin); `set_probe` gets every entry with `dims == None`. Textures: `HashMap<PathBuf, TextureHandle>` LRU capped at **400**. Tile: thumb fitted inside its rect (contain, not crop), placeholder gray while loading, a « ⚠ » error state on `Failed` (entry.error = true). Section headers in recursive mode: rel path (root shown as « . »), click toggles collapse (persisted in `FolderState.collapsed`). Non-recursive: single section, no header.
  - **Live updates**: drain watcher channel each frame → `Library::apply`; `Rescan` / F5 → full rescan. A modified file drops its texture and its thumb (key changed). New entries: highlight border fading over **1.5 s** from `arrived_at`. Scroll: if the offset was 0 it stays 0; otherwise keep the first visible tile's on-screen y unchanged across a relayout (adjust offset by its delta).
  - **Hover**: tooltip with name, `W × H`, size (Ko/Mo, French decimal comma), and rel path in recursive mode. **Status bar**: same info for hovered else focused image, plus « N images » and « M sélectionnée(s) ».
  - `wake` closures call `ctx.request_repaint()`. Repaint while a highlight is fading.

- [ ] **Step 1:** Implement; `cargo build` clean, `cargo clippy` clean, `cargo test` still green.
- [ ] **Step 2: Verify by running** `cargo run -- "<scratchpad path from Task 12>"`: thumbnails appear, the three layouts switch with 1/2/3, size slider works, toggle Récursif shows sections. Copy a png into the folder while open → it appears highlighted without the view jumping. Take a screenshot (PowerShell `System.Windows.Forms` + `System.Drawing` `CopyFromScreen` into the scratchpad) and inspect it.
- [ ] **Step 3: Commit** `feat: gallery app with live folder updates`.

---

### Task 10: Interactions: selection, keyboard, clipboard, pins, shell

**Files:** Modify `src/app.rs`, `src/ui/gallery.rs`.

**Interfaces:**
- Consumes: `Selection` (Task 7), `layout::neighbor` (Task 5), `platform::*` (Task 8), `Config::toggle_pin` (Task 6), `ImageMozApp::visible_order` (Task 9).
- Behavior: click / Ctrl+click / Shift+click on tiles; arrows move the focus with `layout::neighbor` (Shift extends) and scroll the focused tile into view; double-click → `platform::open_default`; right-click → context menu « Afficher dans l'Explorateur » → `reveal_in_explorer`; `Ctrl+C` → `copy_files_and_bitmap(selected, focused full-res decode)` (decode on a background thread; then copy on the UI thread when ready — the clipboard needs no window but must not block a frame on decode); `Ctrl+Alt+C` → `copy_text` of absolute paths, one per line; `P` toggles pin on the focused image (pins of missing files ignored, Bento re-layouts); selection survives inserts/removals via `Selection::remap`. Selected tiles drawn with an accent border, focus with a thicker one. A short status-bar message « Copié : N fichier(s) » / « Chemins copiés » for 2 s; clipboard error shown there too.
- [ ] **Step 1:** Implement; build, clippy, test green.
- [ ] **Step 2: Verify by running:** select 2 tiles, Ctrl+C, paste in an Explorer folder (files copied) and in Paint (bitmap). Ctrl+Alt+C, paste in Notepad. Arrows move in all three layouts. P in Bento moves the image into a 2×2 slot with rule « Épinglées ».
- [ ] **Step 3: Commit** `feat: selection, keyboard, clipboard and pins`.

---

### Task 11: Full-frame preview

**Files:** Modify `src/preview.rs`, `src/ui/preview_view.rs`, `src/app.rs`.

**Interfaces:**
- Produces in `preview.rs`: `#[derive(Clone, Copy, Debug, PartialEq)] pub struct ViewTransform { pub scale: f32, pub offset: egui::Vec2 /* image origin in viewport coords */ }` with `fit(image: Vec2, viewport: Rect) -> Self` (contain, centered; small images are upscaled to fit too), `zoom_at(&mut self, factor: f32, cursor: Pos2)` (the image point under the cursor stays under the cursor; scale clamped to `0.02..=64`), `pan(&mut self, delta: Vec2)`, `image_rect(&self, image: Vec2) -> Rect`.
- `PreviewState { index: usize /* into visible_order */, image: Option<TextureHandle>, loading path, transform: Option<ViewTransform>, fitted: bool }`.
- Behavior: `Space` opens the focused image; decode original (`formats::decode`) on a background thread, show the thumbnail meanwhile; `←`/`→` previous/next in `visible_order` (moves the selection focus too); `Esc`/`Space` close; wheel zoom ×1.1 per notch at cursor; drag pans; `0` refits; window resize while fitted refits. `Ctrl+C` / `Ctrl+Alt+C` act on the previewed image. Dark background, file name + dims in the status bar.
- [ ] **Step 1: Failing tests in `preview.rs`:** `fit_centers_and_contains`, `zoom_keeps_point_under_cursor` (point under cursor in image space equal before/after ±1e-3), `zoom_clamped`, `pan_moves_offset`.
- [ ] **Step 2:** `cargo test preview` → FAIL.
- [ ] **Step 3:** Implement math, then view + wiring.
- [ ] **Step 4:** `cargo test` PASS; run the app, open preview on a large png, zoom, pan, 0, arrows, Esc.
- [ ] **Step 5: Commit** `feat: full-frame preview`.

---

### Task 12: Verification on the real folder

**Folder:** `C:\Users\julie\AppData\Local\Temp\claude\D--dev-blender-tools\4914bc68-3a54-4641-89ca-d3137793d89d\scratchpad` (contains pngs plus `.json`/`.py` that must be ignored).

- [ ] `cargo build --release`; launch `target\release\image-moz.exe "<folder>"` with `RUST_LOG=image_moz=debug` (debug build for logs).
- [ ] Screenshot: the 7 pngs, no json/py tiles, in each layout.
- [ ] While open, copy a new png into the folder (e.g. a copy of `plan_0.png` named `zz_new.png`): it appears within ~1 s, highlighted, log shows one `Upsert`. Overwrite it with another image: thumbnail changes. Delete it: tile disappears. **Clean up** the added file at the end — the folder is not ours.
- [ ] Restart without argument: same folder, layout and size restored.
- [ ] Launch with a nonexistent folder: « Dossier introuvable », no panic.
- [ ] Report results; fix-forward anything broken as its own commit.
