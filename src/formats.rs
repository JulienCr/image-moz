use std::path::Path;

use image::{Rgba32FImage, RgbaImage};

use crate::blend::{self, BlendError};
use crate::model::offscreen::{MODEL_RENDER_SIZE, Offscreen};
use crate::model::{self, ModelError};

const EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "webp", "bmp", "gif", "tif", "tiff", "exr", "hdr",
];
const MODEL_EXTENSIONS: &[&str] = &["glb", "gltf", "obj", "stl", "3mf"];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MediaKind {
    Image,
    Model,
    Blend,
}

/// A decoded file; `triangles` is set for 3D models only.
pub struct Decoded {
    pub image: RgbaImage,
    pub triangles: Option<u32>,
}

#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("image error: {0}")]
    Image(#[from] image::ImageError),
    #[error("unsupported format")]
    Unsupported,
    #[error("model error: {0}")]
    Model(#[from] ModelError),
    #[error("gpu error: {0}")]
    Gpu(String),
    #[error("blend error: {0}")]
    Blend(#[from] BlendError),
    #[error("no embedded preview")]
    NoPreview,
}

fn extension(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
}

pub fn kind(path: &Path) -> Option<MediaKind> {
    let ext = extension(path)?;
    if EXTENSIONS.contains(&ext.as_str()) {
        Some(MediaKind::Image)
    } else if MODEL_EXTENSIONS.contains(&ext.as_str()) {
        Some(MediaKind::Model)
    } else if ext == "blend" {
        Some(MediaKind::Blend)
    } else {
        None
    }
}

pub fn is_supported(path: &Path) -> bool {
    kind(path).is_some()
}

/// Upper-case extension shown on tiles of non-image files.
pub fn badge(path: &Path) -> Option<&'static str> {
    match extension(path)?.as_str() {
        "glb" => Some("GLB"),
        "gltf" => Some("GLTF"),
        "obj" => Some("OBJ"),
        "stl" => Some("STL"),
        "3mf" => Some("3MF"),
        "blend" => Some("BLEND"),
        _ => None,
    }
}

/// Header-only size; models are reported at their render size without being read.
pub fn dimensions(path: &Path) -> Result<(u32, u32), DecodeError> {
    match kind(path) {
        Some(MediaKind::Model) => Ok((MODEL_RENDER_SIZE, MODEL_RENDER_SIZE)),
        Some(MediaKind::Blend) => Ok(blend_thumbnail(path)?.dimensions()),
        _ => Ok(image::image_dimensions(path)?),
    }
}

pub fn decode(path: &Path) -> Result<Decoded, DecodeError> {
    let still = |image| Decoded { image, triangles: None };
    match kind(path) {
        None => Err(DecodeError::Unsupported),
        Some(MediaKind::Model) => {
            let scene = model::load(path)?;
            let gpu = Offscreen::shared().map_err(DecodeError::Gpu)?;
            let image = gpu.render(&scene, MODEL_RENDER_SIZE).map_err(DecodeError::Gpu)?;
            Ok(Decoded { image, triangles: Some(scene.triangle_count()) })
        }
        Some(MediaKind::Blend) => Ok(still(blend_thumbnail(path)?)),
        Some(MediaKind::Image) => match extension(path).as_deref() {
            Some("exr" | "hdr") => Ok(still(tonemap(&image::open(path)?.to_rgba32f()))),
            _ => Ok(still(image::open(path)?.to_rgba8())),
        },
    }
}

fn blend_thumbnail(path: &Path) -> Result<RgbaImage, DecodeError> {
    blend::embedded_thumbnail(path)?.ok_or(DecodeError::NoPreview)
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
            "a.hdr", "a.glb", "a.GLTF", "a.obj", "a.stl", "a.3mf", "a.blend",
        ] {
            assert!(is_supported(Path::new(ok)), "{ok}");
        }
        for no in ["a.txt", "a.psd", "noext", "a.blend1"] {
            assert!(!is_supported(Path::new(no)), "{no}");
        }
    }

    #[test]
    fn decode_png_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.png");
        let src = RgbaImage::from_fn(3, 2, |x, y| Rgba([x as u8 * 40, y as u8 * 90, 7, 200]));
        src.save(&path).unwrap();
        assert_eq!(decode(&path).unwrap().image, src);
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
        let px = decode(&path).unwrap().image.get_pixel(0, 0).0;
        assert!((px[0] as i32 - 141).abs() <= 1, "{px:?}");
        assert_eq!(px[3], 255);
    }

    #[test]
    fn badges() {
        assert_eq!(badge(Path::new("a.Glb")), Some("GLB"));
        assert_eq!(badge(Path::new("a.blend")), Some("BLEND"));
        assert_eq!(badge(Path::new("a.3mf")), Some("3MF"));
        assert_eq!(badge(Path::new("a.png")), None);
    }

    #[test]
    fn decode_model_reports_triangles() {
        let d = decode(&crate::model::fixture("cube.glb")).unwrap();
        assert_eq!(d.triangles, Some(12));
        assert_eq!(d.image.dimensions(), (MODEL_RENDER_SIZE, MODEL_RENDER_SIZE));
    }

    #[test]
    fn model_dimensions_skip_reading() {
        assert_eq!(
            dimensions(Path::new("missing.glb")).unwrap(),
            (MODEL_RENDER_SIZE, MODEL_RENDER_SIZE)
        );
    }

    #[test]
    fn blend_without_thumbnail_is_no_preview() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.blend");
        let blocks = vec![(*b"REND", vec![0; 16]), (*b"GLOB", vec![0; 8])];
        std::fs::write(&path, blend::synth(b"BLENDER17-01v0501", &blocks, None)).unwrap();
        assert!(matches!(decode(&path), Err(DecodeError::NoPreview)));
        assert!(matches!(dimensions(&path), Err(DecodeError::NoPreview)));
    }

    #[test]
    fn dimensions_reads_header() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.png");
        RgbaImage::new(640, 480).save(&path).unwrap();
        assert_eq!(dimensions(&path).unwrap(), (640, 480));
    }
}
