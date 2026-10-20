//! Turning glTF appearance data into block paint.
//!
//! The color of a face is what a glTF viewer would show for it:
//! `baseColorFactor × vertex color (COLOR_0) × baseColorTexture`, evaluated in linear
//! space and then encoded to sRGB, which is what paint colors in the game are.
//! Vertex colors are averaged over the triangle, and the texture is sampled at the
//! triangle's centroid.

use gltf::{
    image::{Data as Image, Format},
    texture::WrappingMode,
    Material,
};

pub fn srgb_to_linear(v: u8) -> f32 {
    let c = v as f32 / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

pub fn linear_to_srgb(v: f32) -> u8 {
    let c = v.clamp(0.0, 1.0);
    let s = if c <= 0.0031308 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 };
    (s * 255.0).round() as u8
}

/// Linear RGB(A) floats to a paint color.
pub fn encode(c: [f32; 4]) -> [u8; 4] {
    [
        linear_to_srgb(c[0]),
        linear_to_srgb(c[1]),
        linear_to_srgb(c[2]),
        (c[3].clamp(0.0, 1.0) * 255.0).round() as u8,
    ]
}

fn wrap(v: f32, mode: WrappingMode) -> f32 {
    match mode {
        WrappingMode::ClampToEdge => v.clamp(0.0, 1.0),
        WrappingMode::Repeat => v - v.floor(),
        WrappingMode::MirroredRepeat => {
            let t = v.rem_euclid(2.0);
            if t > 1.0 { 2.0 - t } else { t }
        }
    }
}

struct Texture<'a> {
    image: &'a Image,
    wrap_s: WrappingMode,
    wrap_t: WrappingMode,
}

impl Texture<'_> {
    /// Nearest-neighbour sample, as linear RGBA. `None` for pixel formats we don't read.
    fn sample(&self, uv: [f32; 2]) -> Option<[f32; 4]> {
        let img = self.image;
        if img.width == 0 || img.height == 0 {
            return None;
        }
        let u = wrap(uv[0], self.wrap_s);
        let v = wrap(uv[1], self.wrap_t);
        let x = ((u * img.width as f32) as u32).min(img.width - 1) as usize;
        let y = ((v * img.height as f32) as u32).min(img.height - 1) as usize;
        let channels = match img.format {
            Format::R8 => 1,
            Format::R8G8 => 2,
            Format::R8G8B8 => 3,
            Format::R8G8B8A8 => 4,
            _ => return None,
        };
        let px = img.pixels.get((y * img.width as usize + x) * channels..)?.get(..channels)?;
        let (r, g, b) = match channels {
            1 | 2 => (px[0], px[0], px[0]),
            _ => (px[0], px[1], px[2]),
        };
        let a = if channels == 4 { px[3] as f32 / 255.0 } else { 1.0 };
        Some([srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b), a])
    }
}

/// Everything needed to color the triangles of one primitive.
pub struct ColorSource<'a> {
    /// Paint at all? A primitive with no material and no vertex colors stays unpainted.
    active: bool,
    factor: [f32; 4],
    vertex: Option<Vec<[f32; 4]>>,
    uvs: Option<Vec<[f32; 2]>>,
    texture: Option<Texture<'a>>,
}

impl<'a> ColorSource<'a> {
    pub fn new(
        material: &Material,
        vertex: Option<Vec<[f32; 4]>>,
        uvs: Option<Vec<[f32; 2]>>,
        images: &'a [Image],
    ) -> Self {
        let pbr = material.pbr_metallic_roughness();
        let texture = pbr.base_color_texture().and_then(|info| {
            let tex = info.texture();
            let sampler = tex.sampler();
            Some(Texture {
                image: images.get(tex.source().index())?,
                wrap_s: sampler.wrap_s(),
                wrap_t: sampler.wrap_t(),
            })
        });
        ColorSource {
            active: material.index().is_some() || vertex.is_some(),
            factor: pbr.base_color_factor(),
            vertex,
            uvs,
            texture,
        }
    }

    /// Paint for the triangle with these vertex indices.
    pub fn triangle(&self, idx: [usize; 3]) -> Option<[u8; 4]> {
        if !self.active {
            return None;
        }
        let mut c = self.factor;

        if let Some(colors) = &self.vertex {
            let mut avg = [0.0f32; 4];
            for &i in &idx {
                let v = colors.get(i)?;
                for k in 0..4 {
                    avg[k] += v[k] / 3.0;
                }
            }
            for k in 0..4 {
                c[k] *= avg[k];
            }
        }

        if let (Some(tex), Some(uvs)) = (&self.texture, &self.uvs) {
            let mut uv = [0.0f32; 2];
            for &i in &idx {
                let t = uvs.get(i)?;
                uv[0] += t[0] / 3.0;
                uv[1] += t[1] / 3.0;
            }
            if let Some(t) = tex.sample(uv) {
                for k in 0..4 {
                    c[k] *= t[k];
                }
            }
        }

        Some(encode(c))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_round_trips() {
        for v in 0..=255u8 {
            assert_eq!(linear_to_srgb(srgb_to_linear(v)), v);
        }
    }

    #[test]
    fn blender_default_grey() {
        // Blender's default material is 0.8 linear, which is 231 in sRGB
        assert_eq!(linear_to_srgb(0.8), 231);
    }

    #[test]
    fn wrapping() {
        assert!((wrap(1.25, WrappingMode::Repeat) - 0.25).abs() < 1e-6);
        assert!((wrap(-0.25, WrappingMode::Repeat) - 0.75).abs() < 1e-6);
        assert_eq!(wrap(2.0, WrappingMode::ClampToEdge), 1.0);
        assert!((wrap(1.25, WrappingMode::MirroredRepeat) - 0.75).abs() < 1e-6);
    }
}
