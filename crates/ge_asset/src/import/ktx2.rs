//! KTX2 textures (plan 0005, decision 7): BC1/3/4/5/7 and RGBA8 payloads load as-is with their
//! stored mips. zstd/BasisLZ supercompression, cube maps and arrays are rejected with a clear
//! error in v1.

use crate::AssetError;
use crate::types::{PixelFormat, Texture};

pub fn import(bytes: &[u8]) -> Result<Texture, AssetError> {
    let err = |m: String| AssetError::Import(format!("KTX2: {m}"));
    let reader = ktx2::Reader::new(bytes).map_err(|e| err(format!("{e:?}")))?;
    let header = reader.header();
    if let Some(scheme) = header.supercompression_scheme {
        return Err(err(format!(
            "supercompression {scheme:?} is not supported yet; save without supercompression \
             (e.g. `toktx --encode uastc` without `--zcmp`)"
        )));
    }
    if header.face_count > 1 || header.layer_count > 1 || header.pixel_depth > 1 {
        return Err(err(
            "only plain 2D textures are supported (no cube maps, arrays or 3D)".into(),
        ));
    }
    let format = header
        .format
        .and_then(map_format)
        .ok_or_else(|| err(format!("unsupported format {:?}", header.format)))?;
    let (width, height) = (header.pixel_width.max(1), header.pixel_height.max(1));
    let mips: Vec<Vec<u8>> = reader.levels().map(|l| l.data.to_vec()).collect();
    let texture = Texture {
        format,
        width,
        height,
        mips,
    };
    if !texture.is_valid() {
        return Err(err(
            "level sizes don't match the format and dimensions".into()
        ));
    }
    Ok(texture)
}

fn map_format(f: ktx2::Format) -> Option<PixelFormat> {
    use ktx2::Format as F;
    Some(match f {
        F::R8G8B8A8_UNORM => PixelFormat::Rgba8,
        F::R8G8B8A8_SRGB => PixelFormat::Rgba8Srgb,
        F::BC1_RGB_UNORM_BLOCK | F::BC1_RGBA_UNORM_BLOCK => PixelFormat::Bc1,
        F::BC1_RGB_SRGB_BLOCK | F::BC1_RGBA_SRGB_BLOCK => PixelFormat::Bc1Srgb,
        F::BC3_UNORM_BLOCK => PixelFormat::Bc3,
        F::BC3_SRGB_BLOCK => PixelFormat::Bc3Srgb,
        F::BC4_UNORM_BLOCK => PixelFormat::Bc4,
        F::BC5_UNORM_BLOCK => PixelFormat::Bc5,
        F::BC7_UNORM_BLOCK => PixelFormat::Bc7,
        F::BC7_SRGB_BLOCK => PixelFormat::Bc7Srgb,
        _ => return None,
    })
}

/// Writes a minimal, valid KTX2 file (no DFD-dependent features, no supercompression). Used by
/// tests and `xtask gen-assets`; our own writer, so the fixtures carry no third-party license.
pub fn write(format: PixelFormat, width: u32, height: u32, mips: &[Vec<u8>]) -> Vec<u8> {
    let vk_format: u32 = match format {
        PixelFormat::Rgba8 => 37,
        PixelFormat::Rgba8Srgb => 43,
        PixelFormat::Bc1 => 133,
        PixelFormat::Bc1Srgb => 134,
        PixelFormat::Bc3 => 137,
        PixelFormat::Bc3Srgb => 138,
        PixelFormat::Bc4 => 139,
        PixelFormat::Bc5 => 141,
        PixelFormat::Bc7 => 145,
        PixelFormat::Bc7Srgb => 146,
    };
    // typeSize is 1 for block-compressed and 8-bit formats.
    let type_size: u32 = 1;
    let level_count = mips.len() as u32;
    // Header (80) + level index (24 per level) + a DFD holding only its 4-byte total size (no
    // descriptor blocks: our reader takes the format from `vkFormat`). No key/value data.
    let dfd_offset = 80 + 24 * mips.len();
    let header_len = dfd_offset + 4;
    let mut out = Vec::new();
    out.extend_from_slice(&[
        0xAB, 0x4B, 0x54, 0x58, 0x20, 0x32, 0x30, 0xBB, 0x0D, 0x0A, 0x1A, 0x0A,
    ]);
    for v in [vk_format, type_size, width, height, 0, 0, 1, level_count, 0] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    // Index: dfd offset/length, kvd offset/length (u32 each), sgd offset/length (u64 each).
    for v in [dfd_offset as u32, 4, 0, 0] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    for v in [0u64, 0] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    // Level data is stored smallest-first after the index, 16-byte aligned for safety.
    let mut offsets = vec![0u64; mips.len()];
    let mut cursor = header_len.next_multiple_of(16);
    for i in (0..mips.len()).rev() {
        offsets[i] = cursor as u64;
        cursor = (cursor + mips[i].len()).next_multiple_of(16);
    }
    for (i, m) in mips.iter().enumerate() {
        out.extend_from_slice(&offsets[i].to_le_bytes());
        out.extend_from_slice(&(m.len() as u64).to_le_bytes());
        out.extend_from_slice(&(m.len() as u64).to_le_bytes());
    }
    out.extend_from_slice(&4u32.to_le_bytes()); // DFD total size
    out.resize(cursor, 0);
    for (i, m) in mips.iter().enumerate() {
        let o = offsets[i] as usize;
        out[o..o + m.len()].copy_from_slice(m);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A BC1 block of one solid color: both endpoints equal, all indices 0.
    fn solid_bc1(rgb565: u16, blocks: usize) -> Vec<u8> {
        let mut b = Vec::with_capacity(blocks * 8);
        for _ in 0..blocks {
            b.extend_from_slice(&rgb565.to_le_bytes());
            b.extend_from_slice(&rgb565.to_le_bytes());
            b.extend_from_slice(&[0; 4]);
        }
        b
    }

    #[test]
    fn bc1_with_mips_round_trips() {
        let mips = vec![
            solid_bc1(0xF800, 4),
            solid_bc1(0x07E0, 1),
            solid_bc1(0x001F, 1),
        ];
        let file = write(PixelFormat::Bc1Srgb, 8, 8, &mips);
        let t = import(&file).unwrap();
        assert_eq!((t.format, t.width, t.height), (PixelFormat::Bc1Srgb, 8, 8));
        assert_eq!(t.mips, mips);
    }

    #[test]
    fn garbage_and_bad_sizes_are_errors() {
        assert!(import(b"definitely not ktx2").is_err());
        let bad = write(PixelFormat::Bc1, 8, 8, &[vec![0; 3]]);
        assert!(import(&bad).is_err());
    }
}
