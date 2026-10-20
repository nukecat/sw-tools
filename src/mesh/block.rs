//! Building custom-shape blocks out of geometry.
//!
//! A custom-shape block (metal/wood/glass/neon) is described by two horizontal
//! cross-sections, each an axis-aligned rectangle in the block's local frame:
//! vector 0 is the top center, 1 the top size (x, z), 2 the bottom center and 3 the
//! bottom size. Make the rectangles thin in z and the block becomes a flat
//! *trapezoid*: its bottom edge and top edge are parallel to local X. That is exactly
//! what is needed to represent
//!
//! * a **quad** that has a pair of parallel opposite edges (rectangles,
//!   parallelograms, trapezoids), and
//! * a **triangle**, as the degenerate trapezoid whose top edge has zero width.
//!
//! # In-game limits
//!
//! The game restricts the planes more tightly than the file format does:
//!
//! |          | x    | y    | z    |
//! |----------|------|------|------|
//! | min pos  | -6   | -5   | -6   |
//! | max pos  | 6    | 5    | 6    |
//! | max size | 10   | 0    | 10   |
//!
//! Blocks are therefore centered on their face (the block's origin is not a corner),
//! which makes the most of those ranges: a plate can be 10 × 10. Faces that are still
//! too big are cut in half, repeatedly, until every piece fits.

use glam::{EulerRot, Mat3, Quat, Vec3};
use sw_formats::{
    primitives::{Rgba32, Vec3 as SwVec3},
    structure::{Block, BlockSettings},
};

use super::MeshError;

/// Largest plane size in x and z, which also bounds the thickness.
pub const MAX_SIZE: f32 = 10.0;
const MAX_POS_X: f32 = 6.0;
const MAX_POS_Y: f32 = 5.0;
const MAX_POS_Z: f32 = 6.0;
/// Slack for float error when comparing against the limits.
const TOL: f32 = 1e-4;
const EPS: f32 = 1e-6;
/// Halving more often than this means something is badly wrong (or the face is
/// astronomically large).
const MAX_DEPTH: u32 = 24;

/// Unity order: Z, then X, then Y == glam YXZ.
fn unity_euler_deg(q: Quat) -> SwVec3 {
    let (y, x, z) = q.to_euler(EulerRot::YXZ);
    SwVec3::new(x.to_degrees(), y.to_degrees(), z.to_degrees())
}

fn paint(color: Option<[u8; 4]>) -> Option<Rgba32> {
    color.map(|c| Rgba32::new(c[0], c[1], c[2], c[3]))
}

/// Picks the longest edge of a triangle as the base and returns it as a degenerate
/// quad `[base_start, base_end, apex, apex]` with the winding preserved.
pub fn triangle_as_quad(p: [Vec3; 3]) -> [Vec3; 4] {
    let mut best = 0;
    let mut best_len = -1.0;
    for i in 0..3 {
        let len = (p[(i + 1) % 3] - p[i]).length_squared();
        if len > best_len {
            best_len = len;
            best = i;
        }
    }
    let apex = p[(best + 2) % 3];
    [p[best], p[(best + 1) % 3], apex, apex]
}

/// Normal of a quad `[q0, q1, q2, q3]` given in winding order, from its diagonals.
/// Works for the degenerate `[a, b, c, c]` triangle too.
pub fn quad_normal(q: &[Vec3; 4]) -> Vec3 {
    (q[2] - q[0]).cross(q[3] - q[1]).normalize_or_zero()
}

/// A quad described in its own frame: `x` along the parallel edges, `n` the face
/// normal, `y = n × x`, origin at `q0`.
struct Frame {
    x: Vec3,
    y: Vec3,
    n: Vec3,
    /// Bottom edge runs from 0 to `bottom_w` along x at y = 0.
    bottom_w: f32,
    /// Top edge runs from `top_x0` to `top_x1` along x at y = `height`.
    top_x0: f32,
    top_x1: f32,
    height: f32,
}

impl Frame {
    fn new(q: &[Vec3; 4]) -> Option<Frame> {
        let n = quad_normal(q);
        let ea = q[1] - q[0];
        let x = (ea - n * ea.dot(n)).normalize_or_zero();
        if n == Vec3::ZERO || x == Vec3::ZERO {
            return None;
        }
        let y = n.cross(x);
        let d2 = q[2] - q[0];
        let d3 = q[3] - q[0];
        let height = 0.5 * (d2.dot(y) + d3.dot(y));
        let bottom_w = ea.dot(x);
        if height <= EPS || bottom_w <= EPS {
            return None;
        }
        let (top_x0, top_x1) = (d3.dot(x), d2.dot(x).max(d3.dot(x)));
        Some(Frame { x, y, n, bottom_w, top_x0, top_x1, height })
    }
}

/// Adds the block(s) for the quad `q` (winding order) to `out` and returns how many
/// were needed. The edges `q0 -> q1` and `q3 -> q2` must be parallel; `q2 == q3`
/// gives a triangle. The thickness extends *behind* the face (against its normal).
///
/// A face too big for the in-game limits is split into several blocks. Degenerate
/// input adds nothing.
pub fn trapezoid(
    q: [Vec3; 4],
    thickness: f32,
    id: i32,
    color: Option<[u8; 4]>,
    out: &mut Vec<Block>,
) -> Result<usize, MeshError> {
    let before = out.len();
    plate(q, thickness, id, color, out, 0)?;
    Ok(out.len() - before)
}

fn plate(
    q: [Vec3; 4],
    t: f32,
    id: i32,
    color: Option<[u8; 4]>,
    out: &mut Vec<Block>,
    depth: u32,
) -> Result<(), MeshError> {
    let Some(f) = Frame::new(&q) else { return Ok(()) };

    // center the block on its face so the position ranges are used symmetrically
    let x_min = 0.0f32.min(f.top_x0);
    let x_max = f.bottom_w.max(f.top_x1);
    let cx = 0.5 * (x_min + x_max);
    let cy = 0.5 * f.height;

    let top_w = f.top_x1 - f.top_x0;
    let top_c = 0.5 * (f.top_x0 + f.top_x1) - cx;
    let bottom_c = 0.5 * f.bottom_w - cx;

    let fits = f.height <= 2.0 * MAX_POS_Y + TOL
        && f.bottom_w <= MAX_SIZE + TOL
        && top_w <= MAX_SIZE + TOL
        && top_c.abs() <= MAX_POS_X + TOL
        && bottom_c.abs() <= MAX_POS_X + TOL;

    if !fits {
        if depth >= MAX_DEPTH {
            return Err(MeshError::TooLarge(
                "a face is too large to be split into custom blocks; use a smaller --scale".into(),
            ));
        }
        // Too wide: cut the width. Too tall, or wide enough but skewed so far sideways
        // that the edge centers leave their range: cut the height, which halves the
        // skew (cutting the width would keep the same slope and never help).
        let widths_fit = f.bottom_w <= MAX_SIZE + TOL && top_w <= MAX_SIZE + TOL;
        let (a, b) = if f.height > 2.0 * MAX_POS_Y || widths_fit {
            // cut across, halfway up; the new edge stays parallel to the other two
            let m0 = q[0].lerp(q[3], 0.5);
            let m1 = q[1].lerp(q[2], 0.5);
            ([q[0], q[1], m1, m0], [m0, m1, q[2], q[3]])
        } else {
            // cut from the middle of the bottom edge to the middle of the top edge
            let pb = q[0].lerp(q[1], 0.5);
            let pt = q[3].lerp(q[2], 0.5);
            ([q[0], pb, pt, q[3]], [pb, q[1], q[2], pt])
        };
        plate(a, t, id, color, out, depth + 1)?;
        return plate(b, t, id, color, out, depth + 1);
    }

    let clamp_pos = |v: SwVec3| {
        SwVec3::new(
            v.x.clamp(-MAX_POS_X, MAX_POS_X),
            v.y.clamp(-MAX_POS_Y, MAX_POS_Y),
            v.z.clamp(-MAX_POS_Z, MAX_POS_Z),
        )
    };
    let size = |w: f32| SwVec3::new(w.clamp(0.0, MAX_SIZE), 0.0, t);
    let vectors = vec![
        // top plane: center, then size (x, 0, z)
        clamp_pos(SwVec3::new(top_c, cy, -t / 2.0)),
        size(top_w),
        // bottom plane: center, then size
        clamp_pos(SwVec3::new(bottom_c, -cy, -t / 2.0)),
        size(f.bottom_w),
    ];

    let pos = q[0] + f.x * cx + f.y * cy;
    let rotation = Quat::from_mat3(&Mat3::from_cols(f.x, f.y, f.n));
    out.push(Block {
        position: SwVec3::new(pos.x, pos.y, pos.z),
        rotation: unity_euler_deg(rotation),
        settings: Some(BlockSettings { vectors, ..Default::default() }),
        color: paint(color),
        ..Block::new(id, 0)
    });
    Ok(())
}

/// A thin square-section bar from `a` to `b`, as one block or, when longer than the
/// game allows, a chain of equal ones. Returns how many were added.
pub fn line(
    a: Vec3,
    b: Vec3,
    thickness: f32,
    id: i32,
    color: Option<[u8; 4]>,
    out: &mut Vec<Block>,
) -> Result<usize, MeshError> {
    let dir = b - a;
    let total = dir.length();
    if total < EPS {
        return Ok(0);
    }
    // the bar spans y = -len/2 ..= len/2 around its midpoint, so len <= 2 * MAX_POS_Y
    let pieces = (((total - TOL) / (2.0 * MAX_POS_Y)).ceil() as usize).max(1);
    if pieces > 1 << MAX_DEPTH {
        return Err(MeshError::TooLarge("an edge is too long; use a smaller --scale".into()));
    }
    let dir = dir / total;
    let len = total / pieces as f32;
    let rotation = unity_euler_deg(Quat::from_rotation_arc(Vec3::Y, dir));

    for i in 0..pieces {
        let mid = a + dir * (len * (i as f32 + 0.5));
        out.push(Block {
            position: SwVec3::new(mid.x, mid.y, mid.z),
            rotation,
            settings: Some(BlockSettings {
                vectors: vec![
                    SwVec3::new(0.0, len / 2.0, 0.0),
                    SwVec3::new(thickness, 0.0, thickness),
                    SwVec3::new(0.0, -len / 2.0, 0.0),
                    SwVec3::new(thickness, 0.0, thickness),
                ],
                ..Default::default()
            }),
            color: paint(color),
            ..Block::new(id, 0)
        });
    }
    Ok(pieces)
}
