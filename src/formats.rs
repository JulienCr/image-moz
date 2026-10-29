use std::path::Path;

use image::{Rgba32FImage, RgbaImage};

const EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "webp", "bmp", "gif", "tif", "tiff", "exr", "hdr",
];

#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("image error: {0}")]
    Image(#[from] image::ImageError),
    #[error("unsupported format")]
    Unsupported,
}

fn extension(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
}

pub fn is_supported(path: &Path) -> bool {
    extension(path).is_some_and(|e| EXTENSIONS.contains(&e.as_str()))
}

pub fn dimensions(path: &Path) -> Result<(u32, u32), DecodeError> {
    Ok(image::image_dimensions(path)?)
}

pub fn decode(path: &Path) -> Result<RgbaImage, DecodeError> {
    if !is_supported(path) {
        return Err(DecodeError::Unsupported);
    }
    match extension(path).as_deref() {
        Some("exr" | "hdr") => Ok(tonemap(&image::open(path)?.to_rgba32f())),
        _ => Ok(image::open(path)?.to_rgba8()),
    }
}

fn tonemap(src: &Rgba32FImage) -> RgbaImage {
    RgbaImage::from_fn(src.width(), src.height(), |x, y| {
        let p = src.get_pixel(x, y).0;
        image::Rgba([
            aces_srgb(p[0]),
            aces_srgb(p[1]),
            aces_srgb(p[2]),
            (p[3].clamp(0.0, 1.0) * 255.0).round() as u8,
        ])
    })
}

/// ACES filmic tone curve (Narkowicz fit) followed by the sRGB transfer function.
pub fn aces_srgb(linear: f32) -> u8 {
    let x = if linear.is_finite() { linear.max(0.0) } else { 0.0 };
    let aces = ((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14)).clamp(0.0, 1.0);
    let srgb = if aces <= 0.003_130_8 {
        aces * 12.92
    } else {
        1.055 * aces.powf(1.0 / 2.4) - 0.055
    };
    (srgb * 255.0).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    #[test]
    fn supported_extensions() {
        for ok in [
            "a.png", "a.JPG", "a.jpeg", "a.webp", "a.bmp", "a.gif", "a.tif", "a.tiff", "a.exr",
            "a.hdr",
        ] {
            assert!(is_supported(Path::new(ok)), "{ok}");
        }
        for no in ["a.txt", "a.psd", "noext"] {
            assert!(!is_supported(Path::new(no)), "{no}");
        }
    }

    #[test]
    fn decode_png_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.png");
        let src = RgbaImage::from_fn(3, 2, |x, y| Rgba([x as u8 * 40, y as u8 * 90, 7, 200]));
        src.save(&path).unwrap();
        assert_eq!(decode(&path).unwrap(), src);
    }

    #[test]
    fn decode_truncated_png_is_err() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.png");
        RgbaImage::from_fn(32, 32, |x, y| Rgba([x as u8, y as u8, 0, 255]))
            .save(&path)
            .unwrap();
        let bytes = std::fs::read(&path).unwrap();
        std::fs::write(&path, &bytes[..bytes.len() / 2]).unwrap();
        assert!(decode(&path).is_err());
    }

    #[test]
    fn decode_zero_byte_is_err() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.png");
        std::fs::write(&path, b"").unwrap();
        assert!(decode(&path).is_err());
    }

    #[test]
    fn aces_values() {
        assert_eq!(aces_srgb(0.0), 0);
        assert!((aces_srgb(0.18) as i32 - 141).abs() <= 1, "{}", aces_srgb(0.18));
        assert_eq!(aces_srgb(100.0), 255);
    }

    #[test]
    fn decode_exr_tonemaps() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.exr");
        let src = Rgba32FImage::from_pixel(1, 1, Rgba([0.18, 0.18, 0.18, 1.0]));
        src.save(&path).unwrap();
        let px = decode(&path).unwrap().get_pixel(0, 0).0;
        assert!((px[0] as i32 - 141).abs() <= 1, "{px:?}");
        assert_eq!(px[3], 255);
    }

    #[test]
    fn dimensions_reads_header() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.png");
        RgbaImage::new(640, 480).save(&path).unwrap();
        assert_eq!(dimensions(&path).unwrap(), (640, 480));
    }
}
