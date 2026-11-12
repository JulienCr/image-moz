use std::io::Read;

use flate2::read::DeflateDecoder;
use image::RgbaImage;

const SIGNATURE: [u8; 8] = *b"\x89PNG\r\n\x1a\n";
const MAX_PIXELS: u64 = 1 << 28;

#[derive(Debug, thiserror::Error)]
#[error("invalid CgBI png: {0}")]
pub struct CgbiError(pub &'static str);

struct Header {
    width: u32,
    height: u32,
    bpp: usize,
    /// Offset of the first chunk after IHDR.
    next: usize,
}

/// Returns (type, data, offset of the next chunk); the trailing CRC is skipped, not validated.
fn chunk_at(bytes: &[u8], at: usize) -> Result<(&[u8], &[u8], usize), CgbiError> {
    let head = bytes
        .get(at..at.saturating_add(8))
        .ok_or(CgbiError("truncated chunk header"))?;
    let len = u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as usize;
    let data_end = (at + 8).checked_add(len).ok_or(CgbiError("chunk too large"))?;
    let data = bytes.get(at + 8..data_end).ok_or(CgbiError("truncated chunk"))?;
    Ok((&head[4..8], data, data_end.saturating_add(4)))
}

/// True when the bytes start with the PNG signature followed by a `CgBI` chunk.
pub fn is_cgbi(bytes: &[u8]) -> bool {
    bytes.len() >= 16 && bytes[..8] == SIGNATURE && &bytes[12..16] == b"CgBI"
}

fn header(bytes: &[u8]) -> Result<Header, CgbiError> {
    if !is_cgbi(bytes) {
        return Err(CgbiError("not a CgBI file"));
    }
    let (_, _, ihdr_at) = chunk_at(bytes, 8)?;
    let (kind, ihdr, next) = chunk_at(bytes, ihdr_at)?;
    if kind != b"IHDR" || ihdr.len() < 13 {
        return Err(CgbiError("missing IHDR"));
    }
    let width = u32::from_be_bytes([ihdr[0], ihdr[1], ihdr[2], ihdr[3]]);
    let height = u32::from_be_bytes([ihdr[4], ihdr[5], ihdr[6], ihdr[7]]);
    if width == 0 || height == 0 {
        return Err(CgbiError("empty image"));
    }
    if ihdr[8] != 8 {
        return Err(CgbiError("unsupported bit depth"));
    }
    let bpp = match ihdr[9] {
        6 => 4,
        2 => 3,
        _ => return Err(CgbiError("unsupported color type")),
    };
    if ihdr[12] != 0 {
        return Err(CgbiError("interlaced images are not supported"));
    }
    Ok(Header { width, height, bpp, next })
}

/// Reads the size from IHDR; needs only the first ~64 bytes of the file.
pub fn dimensions(bytes: &[u8]) -> Result<(u32, u32), CgbiError> {
    let h = header(bytes)?;
    Ok((h.width, h.height))
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let (ia, ib, ic) = (a as i32, b as i32, c as i32);
    let p = ia + ib - ic;
    let (pa, pb, pc) = ((p - ia).abs(), (p - ib).abs(), (p - ic).abs());
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

/// `raw` must hold exactly `rows * (stride + 1)` bytes.
fn unfilter(raw: &[u8], stride: usize, rows: usize, bpp: usize) -> Result<Vec<u8>, CgbiError> {
    let mut out = vec![0u8; stride * rows];
    for y in 0..rows {
        let line = &raw[y * (stride + 1)..(y + 1) * (stride + 1)];
        let (filter, src) = (line[0], &line[1..]);
        let (done, rest) = out.split_at_mut(y * stride);
        let prev = if y == 0 { None } else { Some(&done[(y - 1) * stride..]) };
        let cur = &mut rest[..stride];
        for x in 0..stride {
            let a = if x >= bpp { cur[x - bpp] } else { 0 };
            let b = prev.map_or(0, |p| p[x]);
            let c = if x >= bpp { prev.map_or(0, |p| p[x - bpp]) } else { 0 };
            cur[x] = src[x].wrapping_add(match filter {
                0 => 0,
                1 => a,
                2 => b,
                3 => ((a as u16 + b as u16) / 2) as u8,
                4 => paeth(a, b, c),
                _ => return Err(CgbiError("bad filter type")),
            });
        }
    }
    Ok(out)
}

pub fn decode(bytes: &[u8]) -> Result<RgbaImage, CgbiError> {
    let h = header(bytes)?;
    let pixels = h.width as u64 * h.height as u64;
    if pixels > MAX_PIXELS {
        return Err(CgbiError("image too large"));
    }
    let stride = h.width as usize * h.bpp;
    let rows = h.height as usize;
    let expected = (stride + 1) * rows;

    let mut idat = Vec::new();
    let mut at = h.next;
    while let Ok((kind, data, next)) = chunk_at(bytes, at) {
        match kind {
            b"IDAT" => idat.extend_from_slice(data),
            b"IEND" => break,
            _ => {}
        }
        at = next;
    }

    // Apple streams are raw deflate (no zlib header) and may end abruptly;
    // the length check below is what validates them.
    let mut raw = Vec::new();
    let _ = DeflateDecoder::new(idat.as_slice())
        .take(expected as u64)
        .read_to_end(&mut raw);
    if raw.len() < expected {
        return Err(CgbiError("inflated data too short"));
    }

    let px = unfilter(&raw, stride, rows, h.bpp)?;
    let mut rgba = Vec::with_capacity(pixels as usize * 4);
    for p in px.chunks_exact(h.bpp) {
        let a = if h.bpp == 4 { p[3] } else { 255 };
        // Apple premultiplies color by alpha; undo it, the app expects straight alpha.
        let un = |c: u8| match a {
            0 => 0,
            255 => c,
            _ => ((c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8,
        };
        rgba.extend_from_slice(&[un(p[2]), un(p[1]), un(p[0]), a]);
    }
    RgbaImage::from_raw(h.width, h.height, rgba).ok_or(CgbiError("size mismatch"))
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use flate2::Compression;
    use flate2::write::DeflateEncoder;
    use image::Rgba;

    use super::*;
    use crate::formats;

    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        out.extend_from_slice(&[0; 4]);
    }

    /// Encodes `img` as a CgBI file; row `y` uses filter `filters[y % filters.len()]`.
    fn synth(img: &RgbaImage, filters: &[u8]) -> Vec<u8> {
        let (w, h) = img.dimensions();
        let stride = w as usize * 4;
        let mut prev = vec![0u8; stride];
        let mut raw = Vec::new();
        for y in 0..h {
            let cur: Vec<u8> = (0..w)
                .flat_map(|x| {
                    let [r, g, b, a] = img.get_pixel(x, y).0;
                    let pm = |c: u8| ((c as u32 * a as u32 + 127) / 255) as u8;
                    [pm(b), pm(g), pm(r), a]
                })
                .collect();
            let f = filters[y as usize % filters.len()];
            raw.push(f);
            for x in 0..stride {
                let a = if x >= 4 { cur[x - 4] } else { 0 };
                let b = prev[x];
                let c = if x >= 4 { prev[x - 4] } else { 0 };
                raw.push(cur[x].wrapping_sub(match f {
                    0 => 0,
                    1 => a,
                    2 => b,
                    3 => ((a as u16 + b as u16) / 2) as u8,
                    _ => paeth(a, b, c),
                }));
            }
            prev = cur;
        }
        let mut enc = DeflateEncoder::new(Vec::new(), Compression::default());
        enc.write_all(&raw).unwrap();
        let idat = enc.finish().unwrap();

        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&w.to_be_bytes());
        ihdr.extend_from_slice(&h.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        let mut out = SIGNATURE.to_vec();
        chunk(&mut out, b"CgBI", &[0x50, 0, 0x20, 6]);
        chunk(&mut out, b"IHDR", &ihdr);
        chunk(&mut out, b"IDAT", &idat);
        chunk(&mut out, b"IEND", &[]);
        out
    }

    fn sample() -> RgbaImage {
        RgbaImage::from_fn(17, 9, |x, y| match (x + y) % 4 {
            0 => Rgba([x as u8 * 13, y as u8 * 27, 200, 255]),
            1 => Rgba([0, 0, 0, 0]),
            2 => Rgba([250, 120, x as u8 * 5, 128]),
            _ => Rgba([30, 220, 90, 40]),
        })
    }

    fn write_png(bytes: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.png");
        std::fs::write(&path, bytes).unwrap();
        (dir, path)
    }

    fn assert_matches(got: &RgbaImage, want: &RgbaImage) {
        assert_eq!(got.dimensions(), want.dimensions());
        for (g, w) in got.pixels().zip(want.pixels()) {
            match w.0[3] {
                255 => assert_eq!(g, w),
                0 => assert_eq!(g.0[3], 0),
                a => {
                    assert_eq!(g.0[3], a);
                    // Premultiplying quantizes color to 1/a steps once un-premultiplied.
                    let tol = 255 / a as i32 + 1;
                    for i in 0..3 {
                        assert!((g.0[i] as i32 - w.0[i] as i32).abs() <= tol, "{g:?} vs {w:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn decode_roundtrip_through_formats() {
        let src = sample();
        let (_dir, path) = write_png(&synth(&src, &[0]));
        assert_matches(&formats::decode(&path).unwrap().image, &src);
    }

    #[test]
    fn all_filter_types_decode() {
        let src = sample();
        let (_dir, path) = write_png(&synth(&src, &[1, 2, 3, 4, 0]));
        assert_matches(&formats::decode(&path).unwrap().image, &src);
    }

    #[test]
    fn dimensions_from_ihdr() {
        let (_dir, path) = write_png(&synth(&sample(), &[0]));
        assert_eq!(formats::dimensions(&path).unwrap(), (17, 9));
    }

    #[test]
    fn truncated_file_is_err() {
        let bytes = synth(&sample(), &[0]);
        for cut in [10, 30, bytes.len() / 2, bytes.len() - 20] {
            let (_dir, path) = write_png(&bytes[..cut]);
            assert!(formats::decode(&path).is_err(), "cut at {cut}");
        }
    }
}
