//! PNG/JPEG → RGBA8 [`Texture`] with a full mip chain (plan 0005, decision 7). Color images are
//! filtered in **linear** space (averaging sRGB bytes directly darkens mips); data images
//! (normal, metallic-roughness) are averaged as-is. BCn encoding happens in the cooker (M10).

use std::sync::LazyLock;

use crate::AssetError;
use crate::types::{PixelFormat, Texture};

/// Largest texture side we accept (matches desktop GPU limits).
pub const MAX_SIDE: u32 = 8192;

pub fn decode(bytes: &[u8], srgb: bool, generate_mips: bool) -> Result<Texture, AssetError> {
    let img = image::load_from_memory(bytes)
        .map_err(|e| AssetError::Import(format!("image decode: {e}")))?
        .to_rgba8();
    let (width, height) = img.dimensions();
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
        return Err(AssetError::Import(format!(
            "image is {width}x{height}; sides must be 1..={MAX_SIDE}"
        )));
    }
    Ok(from_rgba8(
        width,
        height,
        img.into_raw(),
        srgb,
        generate_mips,
    ))
}

pub fn from_rgba8(
    width: u32,
    height: u32,
    pixels: Vec<u8>,
    srgb: bool,
    generate_mips: bool,
) -> Texture {
    let mut mips = vec![pixels];
    if generate_mips {
        let (mut w, mut h) = (width, height);
        while w > 1 || h > 1 {
            let next = downsample(mips.last().expect("level 0"), w, h, srgb);
            w = (w / 2).max(1);
            h = (h / 2).max(1);
            mips.push(next);
        }
    }
    Texture {
        format: if srgb {
            PixelFormat::Rgba8Srgb
        } else {
            PixelFormat::Rgba8
        },
        width,
        height,
        mips,
    }
}

static SRGB_TO_LINEAR: LazyLock<[f32; 256]> = LazyLock::new(|| {
    let mut t = [0.0; 256];
    for (i, v) in t.iter_mut().enumerate() {
        let c = i as f32 / 255.0;
        *v = if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        };
    }
    t
});

fn linear_to_srgb_byte(l: f32) -> u8 {
    let s = if l <= 0.003_130_8 {
        l * 12.92
    } else {
        1.055 * l.powf(1.0 / 2.4) - 0.055
    };
    (s.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

/// 2×2 box filter to the next level (odd sizes clamp the last row/column).
fn downsample(src: &[u8], w: u32, h: u32, srgb: bool) -> Vec<u8> {
    let nw = (w / 2).max(1);
    let nh = (h / 2).max(1);
    let lut = &*SRGB_TO_LINEAR;
    let mut out = Vec::with_capacity((nw * nh * 4) as usize);
    for y in 0..nh {
        for x in 0..nw {
            let mut acc = [0.0f32; 4];
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let sx = (x * 2 + dx).min(w - 1);
                let sy = (y * 2 + dy).min(h - 1);
                let i = ((sy * w + sx) * 4) as usize;
                for c in 0..4 {
                    let v = src[i + c];
                    acc[c] += if srgb && c < 3 {
                        lut[v as usize]
                    } else {
                        f32::from(v) / 255.0
                    };
                }
            }
            for (c, a) in acc.iter().enumerate() {
                let avg = a / 4.0;
                out.push(if srgb && c < 3 {
                    linear_to_srgb_byte(avg)
                } else {
                    (avg * 255.0 + 0.5) as u8
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_mip_chain_for_non_square() {
        let t = from_rgba8(8, 2, vec![255; 8 * 2 * 4], true, true);
        let sizes: Vec<usize> = t.mips.iter().map(Vec::len).collect();
        assert_eq!(sizes, [64, 16, 8, 4], "8x2, 4x1, 2x1, 1x1");
        assert!(t.is_valid());
    }

    #[test]
    fn srgb_mips_average_in_linear_space() {
        // Black and white checker: the linear average (0.5) is sRGB ≈ 188, not 128.
        let px = [
            0, 0, 0, 255, 255, 255, 255, 255, 255, 255, 255, 255, 0, 0, 0, 255,
        ];
        let srgb = from_rgba8(2, 2, px.to_vec(), true, true);
        assert!(
            (187..=189).contains(&srgb.mips[1][0]),
            "{}",
            srgb.mips[1][0]
        );
        let data = from_rgba8(2, 2, px.to_vec(), false, true);
        assert_eq!(data.mips[1][0], 128);
        assert_eq!(srgb.mips[1][3], 255, "alpha stays linear");
    }

    #[test]
    fn decodes_png() {
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(4, 4, image::Rgba([10, 20, 30, 255]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let t = decode(&png, true, true).unwrap();
        assert_eq!((t.width, t.height, t.mips.len()), (4, 4, 3));
        assert_eq!(&t.mips[0][..4], &[10, 20, 30, 255]);
        assert!(decode(b"not an image", true, true).is_err());
    }
}
