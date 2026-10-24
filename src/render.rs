//! Triangle meshes for previewing a [`Structure`].
//!
//! Custom-shape blocks (metal, wood, glass, neon: what [`crate::mesh`] builds out of) are
//! drawn as the solid between their two rectangles, so plates and bars look right. Every other
//! block is drawn as a unit cube, because the library does not know the shape of those; the
//! preview is for checking layout, size, orientation and paint, not for exact visuals.
//!
//! Coordinates stay in the game's (Unity, left-handed) space. A right-handed renderer such as
//! three.js should mirror X, e.g. `mesh.scale.x = -1`. Faces wind counter-clockwise when seen
//! from outside in the raw coordinates, so after that mirror they are front faces as usual.

use glam::{EulerRot, Quat, Vec3};
use sw_formats::{
    catalog::{BlockCatalog, BlockKind},
    structure::{Block, Structure},
};

/// Flat-shaded, indexed triangles. Every face has its own four vertices.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Mesh {
    /// x, y, z per vertex.
    pub positions: Vec<f32>,
    /// Outward unit normal per vertex.
    pub normals: Vec<f32>,
    /// r, g, b per vertex, sRGB 0..255.
    pub colors: Vec<u8>,
    pub indices: Vec<u32>,
    /// Blocks drawn in total.
    pub blocks: usize,
    /// Of those, how many used their custom shape (the rest are unit cubes).
    pub custom_shaped: usize,
}

impl Mesh {
    pub fn vertex_count(&self) -> usize {
        self.positions.len() / 3
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// Axis-aligned bounds as `(min, max)`, or `None` when empty.
    pub fn bounds(&self) -> Option<([f32; 3], [f32; 3])> {
        let mut it = self.positions.chunks_exact(3);
        let first = it.next()?;
        let (mut lo, mut hi) = ([first[0], first[1], first[2]], [first[0], first[1], first[2]]);
        for p in it {
            for i in 0..3 {
                lo[i] = lo[i].min(p[i]);
                hi[i] = hi[i].max(p[i]);
            }
        }
        Some((lo, hi))
    }
}

/// Corner order for both rectangles: (-x,-z), (+x,-z), (+x,+z), (-x,+z).
/// Corners 0..4 are the top rectangle, 4..8 the bottom one.
const FACES: [[usize; 4]; 6] = [
    [0, 1, 2, 3], // top
    [4, 5, 6, 7], // bottom
    [0, 1, 5, 4],
    [1, 2, 6, 5],
    [2, 3, 7, 6],
    [3, 0, 4, 7],
];

const CUBE_HALF: f32 = 0.5;
const MIN_AREA: f32 = 1e-10;

fn default_color(kind: &BlockKind) -> [u8; 3] {
    match kind {
        BlockKind::Block => [168, 172, 178],
        BlockKind::Part => [122, 162, 199],
        BlockKind::Tool => [199, 154, 122],
    }
}

fn local_corners(block: &Block, custom_shape: bool) -> ([Vec3; 8], bool) {
    if custom_shape {
        if let Some(v) = block.settings.as_ref().map(|s| &s.vectors).filter(|v| v.len() >= 4) {
            let rect = |c: &sw_formats::primitives::Vec3, size: &sw_formats::primitives::Vec3| {
                let (hx, hz) = (size.x.abs() / 2.0, size.z.abs() / 2.0);
                [
                    Vec3::new(c.x - hx, c.y, c.z - hz),
                    Vec3::new(c.x + hx, c.y, c.z - hz),
                    Vec3::new(c.x + hx, c.y, c.z + hz),
                    Vec3::new(c.x - hx, c.y, c.z + hz),
                ]
            };
            let (t, b) = (rect(&v[0], &v[1]), rect(&v[2], &v[3]));
            return ([t[0], t[1], t[2], t[3], b[0], b[1], b[2], b[3]], true);
        }
    }
    let h = CUBE_HALF;
    (
        [
            Vec3::new(-h, h, -h),
            Vec3::new(h, h, -h),
            Vec3::new(h, h, h),
            Vec3::new(-h, h, h),
            Vec3::new(-h, -h, -h),
            Vec3::new(h, -h, -h),
            Vec3::new(h, -h, h),
            Vec3::new(-h, -h, h),
        ],
        false,
    )
}

/// Builds the preview mesh for a structure.
pub fn build(s: &Structure) -> Mesh {
    let catalog = BlockCatalog::shared();
    let mut m = Mesh::default();

    for b in &s.blocks {
        let info = catalog.get(b.id);
        let (local, custom) = local_corners(b, info.as_ref().is_some_and(|i| i.custom_shape));

        // Unity applies Z, then X, then Y, which is glam's YXZ.
        let q = Quat::from_euler(
            EulerRot::YXZ,
            b.rotation.y.to_radians(),
            b.rotation.x.to_radians(),
            b.rotation.z.to_radians(),
        );
        let origin = Vec3::new(b.position.x, b.position.y, b.position.z);
        let corners: [Vec3; 8] = std::array::from_fn(|i| origin + q * local[i]);
        let center = corners.iter().copied().sum::<Vec3>() / 8.0;

        let rgb = match b.color {
            Some(c) => [c.r, c.g, c.b],
            None => info.as_ref().map(|i| default_color(&i.kind)).unwrap_or([168, 172, 178]),
        };

        for face in FACES {
            let [a, mut bb, c, mut d] = face.map(|i| corners[i]);
            // Newell normal from the diagonals; also fine for a triangle drawn as a quad.
            let mut n = (c - a).cross(d - bb);
            if n.length_squared() < MIN_AREA {
                continue;
            }
            // Wind outward: the normal must point away from the block's middle.
            let face_center = (a + bb + c + d) / 4.0;
            if n.dot(face_center - center) < 0.0 {
                std::mem::swap(&mut bb, &mut d);
                n = -n;
            }
            let n = n.normalize();

            let base = m.vertex_count() as u32;
            for p in [a, bb, c, d] {
                m.positions.extend_from_slice(&[p.x, p.y, p.z]);
                m.normals.extend_from_slice(&[n.x, n.y, n.z]);
                m.colors.extend_from_slice(&rgb);
            }
            m.indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }

        m.blocks += 1;
        m.custom_shaped += custom as usize;
    }

    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use sw_formats::primitives::{Rgba32, Vec3 as SwVec3};
    use sw_formats::structure::{BlockSettings, Root};

    fn structure(blocks: Vec<Block>) -> Structure {
        Structure { roots: vec![Root::default()], blocks, ..Default::default() }
    }

    #[test]
    fn plain_block_is_a_unit_cube() {
        let m = build(&structure(vec![Block::new(0, 0)]));
        assert_eq!((m.blocks, m.custom_shaped), (1, 0));
        assert_eq!(m.vertex_count(), 24);
        assert_eq!(m.triangle_count(), 12);
        let (lo, hi) = m.bounds().unwrap();
        assert_eq!((lo, hi), ([-0.5; 3], [0.5; 3]));
    }

    #[test]
    fn paint_and_position_are_applied() {
        let b = Block {
            position: SwVec3::new(10.0, 0.0, 0.0),
            color: Some(Rgba32::new(1, 2, 3, 255)),
            ..Block::new(0, 0)
        };
        let m = build(&structure(vec![b]));
        assert_eq!(&m.colors[..3], &[1, 2, 3]);
        assert_eq!(m.bounds().unwrap().0[0], 9.5);
    }

    #[test]
    fn normals_point_away_from_the_block() {
        let m = build(&structure(vec![Block::new(0, 0)]));
        for (p, n) in m.positions.chunks(3).zip(m.normals.chunks(3)) {
            assert!(p[0] * n[0] + p[1] * n[1] + p[2] * n[2] > 0.0);
        }
    }

    #[test]
    fn plate_is_thin_and_triangle_has_no_top_face() {
        // custom block 109 (metal): a triangle plate, top edge of zero width
        let b = Block {
            settings: Some(BlockSettings {
                vectors: vec![
                    SwVec3::new(0.0, 1.0, -0.025),
                    SwVec3::new(0.0, 0.0, 0.05),
                    SwVec3::new(0.0, -1.0, -0.025),
                    SwVec3::new(2.0, 0.0, 0.05),
                ],
                ..Default::default()
            }),
            ..Block::new(109, 0)
        };
        let m = build(&structure(vec![b]));
        assert_eq!(m.custom_shaped, 1);
        let (lo, hi) = m.bounds().unwrap();
        assert!((hi[2] - lo[2] - 0.05).abs() < 1e-6, "thickness");
        assert!((hi[0] - lo[0] - 2.0).abs() < 1e-6, "width");
        assert_eq!(m.vertex_count() / 4, 5, "top face is degenerate and skipped");
    }

    #[test]
    fn rotation_uses_unity_order() {
        // 90 degrees around Y sends +X to -Z in a left-handed Y-up space.
        let b = Block {
            rotation: SwVec3::new(0.0, 90.0, 0.0),
            settings: Some(BlockSettings {
                vectors: vec![
                    SwVec3::new(0.0, 0.5, 0.0),
                    SwVec3::new(4.0, 0.0, 1.0),
                    SwVec3::new(0.0, -0.5, 0.0),
                    SwVec3::new(4.0, 0.0, 1.0),
                ],
                ..Default::default()
            }),
            ..Block::new(109, 0)
        };
        let (lo, hi) = build(&structure(vec![b])).bounds().unwrap();
        assert!((hi[2] - lo[2] - 4.0).abs() < 1e-5);
        assert!((hi[0] - lo[0] - 1.0).abs() < 1e-5);
    }
}
