//! Extracts the preview thumbnail Blender embeds in `.blend` files.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Read};
use std::path::Path;

use image::RgbaImage;

const MAX_SIDE: i32 = 2048;

#[derive(Debug, thiserror::Error)]
pub enum BlendError {
    #[error("i/o error: {0}")]
    Io(#[from] io::Error),
    #[error("invalid .blend file: {0}")]
    Format(String),
}

/// Returns the embedded thumbnail, or `Ok(None)` when the file has none.
///
/// Only the leading blocks are read: the thumbnail precedes the scene data,
/// so a multi-hundred-MB file costs a few KB of decoding.
pub fn embedded_thumbnail(path: &Path) -> Result<Option<RgbaImage>, BlendError> {
    let mut file = BufReader::new(File::open(path)?);
    let head = file.fill_buf()?;
    let mut reader: Box<dyn Read> = if head.starts_with(&[0x28, 0xB5, 0x2F, 0xFD]) {
        let decoder = ruzstd::decoding::StreamingDecoder::new(file)
            .map_err(|e| BlendError::Format(format!("zstd: {e}")))?;
        Box::new(decoder)
    } else if head.starts_with(&[0x1F, 0x8B]) {
        Box::new(flate2::read::GzDecoder::new(file))
    } else {
        Box::new(file)
    };
    read_thumbnail(&mut reader)
}

fn fmt_err(msg: &str) -> BlendError {
    BlendError::Format(msg.to_string())
}

fn read_exact(r: &mut dyn Read, buf: &mut [u8]) -> Result<(), BlendError> {
    r.read_exact(buf).map_err(|e| match e.kind() {
        io::ErrorKind::UnexpectedEof => fmt_err("unexpected end of file"),
        _ => BlendError::Io(e),
    })
}

struct BlockHeader {
    code: [u8; 4],
    len: u64,
}

enum Layout {
    Legacy { pointer_size: usize, big_endian: bool },
    V5,
}

fn read_file_header(r: &mut dyn Read) -> Result<Layout, BlendError> {
    let mut head = [0u8; 12];
    read_exact(r, &mut head)?;
    if &head[..7] != b"BLENDER" {
        return Err(fmt_err("missing BLENDER magic"));
    }
    match head[7] {
        b'_' | b'-' => {
            let pointer_size = if head[7] == b'_' { 4 } else { 8 };
            match head[8] {
                b'v' | b'V' => Ok(Layout::Legacy { pointer_size, big_endian: head[8] == b'V' }),
                _ => Err(fmt_err("unknown endianness")),
            }
        }
        b'0'..=b'9' => {
            let size: usize = std::str::from_utf8(&head[7..9])
                .ok()
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| fmt_err("bad header size"))?;
            if size < 12 {
                return Err(fmt_err("bad header size"));
            }
            let mut rest = vec![0u8; size - 12];
            read_exact(r, &mut rest)?;
            Ok(Layout::V5)
        }
        _ => Err(fmt_err("unknown header")),
    }
}

fn read_block_header(r: &mut dyn Read, layout: &Layout) -> Result<Option<BlockHeader>, BlendError> {
    let mut code = [0u8; 4];
    if r.read(&mut code[..1])? == 0 {
        return Ok(None);
    }
    read_exact(r, &mut code[1..])?;
    let len = match layout {
        Layout::V5 => {
            let mut rest = [0u8; 28];
            read_exact(r, &mut rest)?;
            i64::from_le_bytes(rest[12..20].try_into().unwrap())
        }
        Layout::Legacy { pointer_size, big_endian } => {
            let mut rest = vec![0u8; 4 + pointer_size + 8];
            read_exact(r, &mut rest)?;
            let raw: [u8; 4] = rest[..4].try_into().unwrap();
            i64::from(if *big_endian { i32::from_be_bytes(raw) } else { i32::from_le_bytes(raw) })
        }
    };
    let len = u64::try_from(len).map_err(|_| fmt_err("negative block length"))?;
    Ok(Some(BlockHeader { code, len }))
}

fn read_thumbnail(r: &mut dyn Read) -> Result<Option<RgbaImage>, BlendError> {
    let layout = read_file_header(r)?;
    let big_endian = matches!(layout, Layout::Legacy { big_endian: true, .. });
    let i32_at = |b: &[u8]| {
        let raw: [u8; 4] = b.try_into().unwrap();
        if big_endian { i32::from_be_bytes(raw) } else { i32::from_le_bytes(raw) }
    };
    while let Some(block) = read_block_header(r, &layout)? {
        match &block.code {
            b"REND" => {
                let skipped = io::copy(&mut (&mut *r).take(block.len), &mut io::sink())?;
                if skipped != block.len {
                    return Err(fmt_err("truncated REND block"));
                }
            }
            b"TEST" => {
                let max = 8 + 4 * (MAX_SIDE as u64) * (MAX_SIDE as u64);
                if block.len < 8 || block.len > max {
                    return Err(fmt_err("bad thumbnail block length"));
                }
                let mut dims = [0u8; 8];
                read_exact(r, &mut dims)?;
                let (w, h) = (i32_at(&dims[..4]), i32_at(&dims[4..]));
                if !(1..=MAX_SIDE).contains(&w) || !(1..=MAX_SIDE).contains(&h) {
                    return Err(fmt_err("thumbnail dimensions out of range"));
                }
                let (w, h) = (w as usize, h as usize);
                if block.len != 8 + (w * h * 4) as u64 {
                    return Err(fmt_err("thumbnail length does not match its dimensions"));
                }
                let mut pixels = vec![0u8; w * h * 4];
                read_exact(r, &mut pixels)?;
                // Blender stores rows bottom first.
                let flipped: Vec<u8> = pixels.chunks_exact(w * 4).rev().flatten().copied().collect();
                return Ok(RgbaImage::from_raw(w as u32, h as u32, flipped));
            }
            _ => return Ok(None),
        }
    }
    Ok(None)
}

#[cfg(test)]
#[derive(Clone, Copy)]
pub(crate) enum Codec {
    Zstd,
    Gzip,
}

/// Builds a `.blend` image from a header and `(code, payload)` blocks, using
/// the block layout the header implies (17-byte v5 header, else legacy).
#[cfg(test)]
pub(crate) fn synth(header: &[u8], blocks: &[([u8; 4], Vec<u8>)], compress: Option<Codec>) -> Vec<u8> {
    use std::io::Write;
    let v5 = header.len() == 17;
    let pointer_size = if header[7] == b'_' { 4 } else { 8 };
    let mut out = header.to_vec();
    for (code, data) in blocks {
        out.extend_from_slice(code);
        if v5 {
            out.extend_from_slice(&0i32.to_le_bytes());
            out.extend_from_slice(&0u64.to_le_bytes());
            out.extend_from_slice(&(data.len() as i64).to_le_bytes());
            out.extend_from_slice(&1i64.to_le_bytes());
        } else {
            let len = data.len() as i32;
            out.extend_from_slice(&if header[8] == b'V' { len.to_be_bytes() } else { len.to_le_bytes() });
            out.extend(std::iter::repeat_n(0u8, pointer_size + 8));
        }
        out.extend_from_slice(data);
    }
    match compress {
        None => out,
        Some(Codec::Zstd) => {
            ruzstd::encoding::compress_to_vec(&out[..], ruzstd::encoding::CompressionLevel::Fastest)
        }
        Some(Codec::Gzip) => {
            let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
            enc.write_all(&out).unwrap();
            enc.finish().unwrap()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEGACY: &[u8] = b"BLENDER-v405";
    const V5: &[u8] = b"BLENDER17-01v0501";

    /// 2x2 thumbnail stored bottom row first: red row, then blue row.
    fn test_block() -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(&2i32.to_le_bytes());
        d.extend_from_slice(&2i32.to_le_bytes());
        for _ in 0..2 {
            d.extend_from_slice(&[255, 0, 0, 255]);
        }
        for _ in 0..2 {
            d.extend_from_slice(&[0, 0, 255, 255]);
        }
        d
    }

    fn write(bytes: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.blend");
        std::fs::write(&path, bytes).unwrap();
        (dir, path)
    }

    fn standard_blocks() -> Vec<([u8; 4], Vec<u8>)> {
        vec![(*b"REND", vec![0; 16]), (*b"TEST", test_block()), (*b"ENDB", vec![])]
    }

    fn assert_upright(img: &RgbaImage) {
        assert_eq!(img.dimensions(), (2, 2));
        assert_eq!(img.get_pixel(0, 0).0, [0, 0, 255, 255]);
        assert_eq!(img.get_pixel(0, 1).0, [255, 0, 0, 255]);
    }

    #[test]
    fn legacy_raw_thumbnail() {
        let (_d, p) = write(&synth(LEGACY, &standard_blocks(), None));
        assert_upright(&embedded_thumbnail(&p).unwrap().unwrap());
    }

    #[test]
    fn v5_header_thumbnail() {
        let (_d, p) = write(&synth(V5, &standard_blocks(), None));
        assert_upright(&embedded_thumbnail(&p).unwrap().unwrap());
    }

    #[test]
    fn zstd_and_gzip_payloads() {
        for codec in [Codec::Zstd, Codec::Gzip] {
            let (_d, p) = write(&synth(V5, &standard_blocks(), Some(codec)));
            assert_upright(&embedded_thumbnail(&p).unwrap().unwrap());
        }
    }

    #[test]
    fn no_test_block_is_none() {
        let blocks = vec![(*b"REND", vec![0; 16]), (*b"GLOB", vec![0; 8])];
        let (_d, p) = write(&synth(V5, &blocks, None));
        assert!(embedded_thumbnail(&p).unwrap().is_none());
    }

    #[test]
    fn stops_before_reading_the_rest() {
        let mut bytes = synth(V5, &standard_blocks()[..2], None);
        bytes.extend_from_slice(b"GLOB");
        bytes.extend_from_slice(&0i32.to_le_bytes());
        bytes.extend_from_slice(&0u64.to_le_bytes());
        bytes.extend_from_slice(&(1i64 << 40).to_le_bytes());
        bytes.extend_from_slice(&1i64.to_le_bytes());
        let (_d, p) = write(&bytes);
        assert!(embedded_thumbnail(&p).unwrap().is_some());
    }

    #[test]
    fn garbage_is_format_error() {
        let (_d, p) = write(b"NOTBLEND");
        assert!(matches!(embedded_thumbnail(&p), Err(BlendError::Format(_))));
        let mut truncated = test_block();
        truncated.truncate(12);
        let (_d, p) = write(&synth(V5, &[(*b"TEST", truncated)], None));
        assert!(matches!(embedded_thumbnail(&p), Err(BlendError::Format(_))));
    }

    #[test]
    #[ignore = "needs .blend files listed in IMAGE_MOZ_REAL_BLENDS"]
    fn real_files() {
        let Some(list) = std::env::var_os("IMAGE_MOZ_REAL_BLENDS") else {
            println!("IMAGE_MOZ_REAL_BLENDS is unset (';'-separated .blend paths); nothing to check");
            return;
        };
        for (i, blend) in std::env::split_paths(&list).enumerate() {
            let t = std::time::Instant::now();
            let found = embedded_thumbnail(&blend).unwrap();
            let Some(img) = found else {
                println!("{}: no thumbnail, in {:?}", blend.display(), t.elapsed());
                continue;
            };
            println!("{}: {}x{} in {:?}", blend.display(), img.width(), img.height(), t.elapsed());
            img.save(std::env::temp_dir().join(format!("blend-real-{i}.png"))).unwrap();
        }
    }
}
