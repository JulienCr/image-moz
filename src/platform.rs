//! Windows clipboard and shell helpers.

use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND};
use windows::Win32::Graphics::Gdi::{BI_BITFIELDS, BITMAPV5HEADER};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{
    GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock,
};
use windows::Win32::UI::Shell::{DROPFILES, DragQueryFileW, HDROP, ShellExecuteW};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{PCWSTR, w};

const CF_UNICODETEXT: u32 = 13;
const CF_HDROP: u32 = 15;
const CF_DIBV5: u32 = 17;
const LCS_SRGB: u32 = 0x7352_4742;
const LCS_GM_IMAGES: u32 = 4;

#[derive(Debug, thiserror::Error)]
pub enum PlatformError {
    #[error("clipboard is busy: {0}")]
    ClipboardBusy(windows::core::Error),
    #[error("clipboard call failed: {0}")]
    Win32(#[from] windows::core::Error),
    #[error("PNG encoding failed: {0}")]
    Png(#[from] image::ImageError),
    #[error("clipboard format not available")]
    FormatMissing,
}

struct ClipboardGuard;

impl ClipboardGuard {
    fn open() -> Result<Self, PlatformError> {
        let mut last = None;
        for _ in 0..10 {
            match unsafe { OpenClipboard(Some(HWND::default())) } {
                Ok(()) => return Ok(Self),
                Err(e) => {
                    last = Some(e);
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        }
        Err(PlatformError::ClipboardBusy(last.expect("loop ran")))
    }
}

impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        let _ = unsafe { CloseClipboard() };
    }
}

fn utf16_nul(s: &std::ffi::OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

/// Hands `bytes` to the clipboard as `format`. On success the system owns the memory.
fn set_clipboard_bytes(format: u32, bytes: &[u8]) -> Result<(), PlatformError> {
    unsafe {
        let hmem = GlobalAlloc(GMEM_MOVEABLE, bytes.len())?;
        let ptr = GlobalLock(hmem);
        if ptr.is_null() {
            let err = windows::core::Error::from_thread();
            let _ = GlobalFree(Some(hmem));
            return Err(err.into());
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr as *mut u8, bytes.len());
        let _ = GlobalUnlock(hmem);
        if let Err(e) = SetClipboardData(format, Some(HANDLE(hmem.0))) {
            let _ = GlobalFree(Some(hmem));
            return Err(e.into());
        }
    }
    Ok(())
}

fn hdrop_bytes(files: &[PathBuf]) -> Vec<u8> {
    let header_len = std::mem::size_of::<DROPFILES>();
    let header = DROPFILES {
        pFiles: header_len as u32,
        pt: Default::default(),
        fNC: false.into(),
        fWide: true.into(),
    };
    let mut bytes = vec![0u8; header_len];
    unsafe { std::ptr::write_unaligned(bytes.as_mut_ptr() as *mut DROPFILES, header) };
    for f in files {
        for unit in utf16_nul(f.as_os_str()) {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
    }
    bytes.extend_from_slice(&0u16.to_le_bytes());
    bytes
}

fn dibv5_bytes(img: &image::RgbaImage) -> Vec<u8> {
    let (w, h) = img.dimensions();
    let header_len = std::mem::size_of::<BITMAPV5HEADER>();
    let header = BITMAPV5HEADER {
        bV5Size: header_len as u32,
        bV5Width: w as i32,
        bV5Height: h as i32,
        bV5Planes: 1,
        bV5BitCount: 32,
        bV5Compression: BI_BITFIELDS,
        bV5SizeImage: w * h * 4,
        bV5RedMask: 0x00FF_0000,
        bV5GreenMask: 0x0000_FF00,
        bV5BlueMask: 0x0000_00FF,
        bV5AlphaMask: 0xFF00_0000,
        bV5CSType: LCS_SRGB,
        bV5Intent: LCS_GM_IMAGES,
        ..Default::default()
    };
    let mut bytes = vec![0u8; header_len];
    unsafe { std::ptr::write_unaligned(bytes.as_mut_ptr() as *mut BITMAPV5HEADER, header) };
    bytes.reserve((w * h * 4) as usize);
    for y in (0..h).rev() {
        for x in 0..w {
            let [r, g, b, a] = img.get_pixel(x, y).0;
            bytes.extend_from_slice(&[b, g, r, a]);
        }
    }
    bytes
}

fn png_bytes(img: &image::RgbaImage) -> Result<Vec<u8>, PlatformError> {
    let mut out = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)?;
    Ok(out)
}

fn png_format() -> u32 {
    unsafe { RegisterClipboardFormatW(w!("PNG")) }
}

/// Replaces the clipboard with a file list plus, optionally, the bitmap as DIBV5 and PNG.
pub fn copy_files_and_bitmap(
    files: &[PathBuf],
    bitmap: Option<&image::RgbaImage>,
) -> Result<(), PlatformError> {
    let hdrop = hdrop_bytes(files);
    let bitmap_data = bitmap
        .map(|img| Ok::<_, PlatformError>((dibv5_bytes(img), png_bytes(img)?)))
        .transpose()?;
    let _guard = ClipboardGuard::open()?;
    unsafe { EmptyClipboard()? };
    set_clipboard_bytes(CF_HDROP, &hdrop)?;
    if let Some((dib, png)) = bitmap_data {
        set_clipboard_bytes(CF_DIBV5, &dib)?;
        set_clipboard_bytes(png_format(), &png)?;
    }
    Ok(())
}

pub fn copy_text(text: &str) -> Result<(), PlatformError> {
    let bytes: Vec<u8> = text
        .encode_utf16()
        .chain(std::iter::once(0))
        .flat_map(u16::to_le_bytes)
        .collect();
    let _guard = ClipboardGuard::open()?;
    unsafe { EmptyClipboard()? };
    set_clipboard_bytes(CF_UNICODETEXT, &bytes)
}

fn format_available(format: u32) -> bool {
    unsafe { IsClipboardFormatAvailable(format).is_ok() }
}

pub fn clipboard_has_png() -> bool {
    format_available(png_format())
}

pub fn clipboard_has_dib() -> bool {
    format_available(CF_DIBV5)
}

pub fn read_file_list() -> Result<Vec<PathBuf>, PlatformError> {
    let _guard = ClipboardGuard::open()?;
    let handle = unsafe { GetClipboardData(CF_HDROP) }.map_err(|_| PlatformError::FormatMissing)?;
    let hdrop = HDROP(handle.0);
    let count = unsafe { DragQueryFileW(hdrop, u32::MAX, None) };
    let mut out = Vec::with_capacity(count as usize);
    for i in 0..count {
        let len = unsafe { DragQueryFileW(hdrop, i, None) } as usize;
        let mut buf = vec![0u16; len + 1];
        let n = unsafe { DragQueryFileW(hdrop, i, Some(&mut buf)) } as usize;
        out.push(PathBuf::from(std::ffi::OsString::from_wide(&buf[..n])));
    }
    Ok(out)
}

pub fn read_text() -> Result<String, PlatformError> {
    let _guard = ClipboardGuard::open()?;
    let handle =
        unsafe { GetClipboardData(CF_UNICODETEXT) }.map_err(|_| PlatformError::FormatMissing)?;
    let hmem = HGLOBAL(handle.0);
    unsafe {
        let ptr = GlobalLock(hmem) as *const u16;
        if ptr.is_null() {
            return Err(windows::core::Error::from_thread().into());
        }
        let mut len = 0;
        while *ptr.add(len) != 0 {
            len += 1;
        }
        let text = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len));
        let _ = GlobalUnlock(hmem);
        Ok(text)
    }
}

/// Opens `path` with its default application; failures are logged, never raised.
pub fn open_default(path: &Path) {
    let wide = utf16_nul(path.as_os_str());
    let result = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(wide.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    // ShellExecuteW reports success with a pseudo-handle greater than 32.
    let code = result.0 as isize;
    if code <= 32 {
        log::warn!("ShellExecuteW failed for {}: code {code}", path.display());
    }
}

/// Shows `path` selected in Explorer. Explorer exits with 1 even on success, so the code is ignored.
pub fn reveal_in_explorer(path: &Path) {
    let arg = format!("/select,\"{}\"", path.display());
    if let Err(e) = std::process::Command::new("explorer.exe")
        .raw_arg(arg)
        .spawn()
    {
        log::warn!("explorer.exe failed for {}: {e}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "overwrites the system clipboard"]
    fn clipboard_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("é dossier");
        std::fs::create_dir(&sub).unwrap();
        let a = sub.join("a.png");
        let b = sub.join("b.png");
        std::fs::write(&a, b"x").unwrap();
        std::fs::write(&b, b"y").unwrap();
        let bmp = image::RgbaImage::from_pixel(4, 4, image::Rgba([10, 20, 30, 255]));

        copy_files_and_bitmap(&[a.clone(), b.clone()], Some(&bmp)).unwrap();
        assert_eq!(read_file_list().unwrap(), vec![a.clone(), b.clone()]);
        assert!(clipboard_has_png());
        assert!(clipboard_has_dib());

        copy_text("a\r\nb").unwrap();
        assert_eq!(read_text().unwrap(), "a\r\nb");

        copy_files_and_bitmap(std::slice::from_ref(&a), None).unwrap();
        assert_eq!(read_file_list().unwrap(), vec![a]);
        assert!(!clipboard_has_png());
    }
}
