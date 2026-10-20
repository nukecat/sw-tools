//! Merging triangle pairs back into quads.
//!
//! glTF only stores triangles, so a quad arrives as two triangles sharing a diagonal.
//! A pair can be rebuilt as a single block when
//!
//! 1. the triangles share an edge and wind the same way,
//! 2. they are coplanar (within `merge_angle`),
//! 3. together they form a convex quad, and
//! 4. that quad has a pair of parallel opposite edges (see [`super::block`]).
//!
//! Pairs are then picked so that as many triangles as possible get a partner,
//! preferring triangles that were neighbours in the index buffer (which is how
//! exporters emit the two halves of an original quad).

use std::collections::{BTreeSet, HashMap};

use glam::Vec3;

/// Channel difference below which two painted triangles may share one block.
const COLOR_TOLERANCE: u8 = 12;

pub type Key = [u32; 3];

/// Exact-position key, so duplicated vertices (UV/normal seams) still weld.
pub fn key(p: Vec3) -> Key {
    // `+ 0.0` turns -0.0 into 0.0
    [(p.x + 0.0).to_bits(), (p.y + 0.0).to_bits(), (p.z + 0.0).to_bits()]
}

#[derive(Clone, Copy, Debug)]
pub struct Tri {
    pub p: [Vec3; 3],
    pub color: Option<[u8; 4]>,
}

#[derive(Clone, Copy, Debug)]
pub enum Piece {
    /// A triangle that found no partner.
    Tri(usize),
    /// Two triangles as one trapezoid; `q` is arranged so that `q0 -> q1` and
    /// `q3 -> q2` are the parallel edges.
    Quad { q: [Vec3; 4], color: Option<[u8; 4]> },
}

fn tri_normal(t: &Tri) -> Vec3 {
    (t.p[1] - t.p[0]).cross(t.p[2] - t.p[0]).normalize_or_zero()
}

fn merged_color(a: Option<[u8; 4]>, b: Option<[u8; 4]>) -> Result<Option<[u8; 4]>, ()> {
    match (a, b) {
        (None, None) => Ok(None),
        (Some(a), Some(b)) => {
            if a.iter().zip(&b).all(|(x, y)| x.abs_diff(*y) <= COLOR_TOLERANCE) {
                let mut out = [0u8; 4];
                for i in 0..4 {
                    out[i] = ((a[i] as u16 + b[i] as u16 + 1) / 2) as u8;
                }
                Ok(Some(out))
            } else {
                Err(())
            }
        }
        _ => Err(()),
    }
}

/// Tries to rebuild one quad from two triangles. Returns the vertices arranged for
/// [`super::block::trapezoid`].
pub fn fit(t1: &Tri, k1: &[Key; 3], t2: &Tri, k2: &[Key; 3], angle_deg: f32) -> Option<[Vec3; 4]> {
    let (cos_tol, sin_tol) = {
        let a = angle_deg.to_radians();
        (a.cos(), a.sin())
    };

    // shared edge: t1 runs a -> b, a consistently wound t2 runs b -> a
    let mut found = None;
    'search: for i in 0..3 {
        let (a, b) = (k1[i], k1[(i + 1) % 3]);
        for j in 0..3 {
            if k2[j] == b && k2[(j + 1) % 3] == a {
                found = Some((i, j));
                break 'search;
            }
        }
    }
    let (i, j) = found?;

    // coplanar and facing the same way
    let (n1, n2) = (tri_normal(t1), tri_normal(t2));
    if n1 == Vec3::ZERO || n2 == Vec3::ZERO || n1.dot(n2) < cos_tol {
        return None;
    }

    // a -> d -> b -> c is the quad's outline in winding order
    let cyc = [t1.p[i], t2.p[(j + 2) % 3], t1.p[(i + 1) % 3], t1.p[(i + 2) % 3]];

    // convex (a straight vertex is tolerated)
    let n = (cyc[2] - cyc[0]).cross(cyc[3] - cyc[1]).normalize_or_zero();
    if n == Vec3::ZERO {
        return None;
    }
    for i in 0..4 {
        let e1 = cyc[(i + 1) % 4] - cyc[i];
        let e2 = cyc[(i + 2) % 4] - cyc[(i + 1) % 4];
        if e1.cross(e2).dot(n) < -1e-6 * e1.length() * e2.length() {
            return None;
        }
    }

    // one of the two pairs of opposite edges must be parallel
    let mut best: Option<(f32, [Vec3; 4])> = None;
    for k in 0..2 {
        let q = [cyc[k], cyc[(k + 1) % 4], cyc[(k + 2) % 4], cyc[(k + 3) % 4]];
        let ea = q[1] - q[0];
        let eb = q[2] - q[3];
        let (la, lb) = (ea.length(), eb.length());
        if la < 1e-9 || lb < 1e-9 || ea.dot(eb) <= 0.0 {
            continue;
        }
        let sin = ea.cross(eb).length() / (la * lb);
        if sin <= sin_tol && best.is_none_or(|(s, _)| sin < s) {
            best = Some((sin, q));
        }
    }
    best.map(|(_, q)| q)
}

/// Turns triangles into pieces, merging pairs into quads when `merge_angle_deg > 0`.
/// Output follows the order of the input triangles.
pub fn build(tris: &[Tri], merge_angle_deg: f32) -> Vec<Piece> {
    let n = tris.len();
    let single = || (0..n).map(Piece::Tri).collect::<Vec<_>>();
    if merge_angle_deg <= 0.0 || n < 2 {
        return single();
    }

    let keys: Vec<[Key; 3]> = tris.iter().map(|t| t.p.map(key)).collect();

    // which triangles meet along which edge
    let mut edges: HashMap<(Key, Key), Vec<usize>> = HashMap::new();
    for (ti, k) in keys.iter().enumerate() {
        for e in 0..3 {
            let (a, b) = (k[e], k[(e + 1) % 3]);
            let edge = if a < b { (a, b) } else { (b, a) };
            edges.entry(edge).or_default().push(ti);
        }
    }

    // every mergeable pair
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut quads: HashMap<(usize, usize), ([Vec3; 4], Option<[u8; 4]>)> = HashMap::new();
    for tris_on_edge in edges.values() {
        // only manifold edges; anything else is ambiguous
        let [i, j] = tris_on_edge[..] else { continue };
        let pair = (i.min(j), i.max(j));
        if i == j || quads.contains_key(&pair) {
            continue;
        }
        let Ok(color) = merged_color(tris[i].color, tris[j].color) else { continue };
        let Some(q) = fit(&tris[i], &keys[i], &tris[j], &keys[j], merge_angle_deg)
            .or_else(|| fit(&tris[j], &keys[j], &tris[i], &keys[i], merge_angle_deg))
        else {
            continue;
        };
        quads.insert(pair, (q, color));
        adj[i].push(j);
        adj[j].push(i);
    }

    // Greedy matching, lowest remaining degree first: a triangle with a single
    // candidate must take it, so nobody gets stranded needlessly. Among a
    // triangle's candidates the index-neighbour (the other half of the original
    // quad) wins.
    let mut matched: Vec<Option<usize>> = vec![None; n];
    let mut deg: Vec<usize> = adj.iter().map(Vec::len).collect();
    let mut queue: BTreeSet<(usize, usize)> =
        (0..n).filter(|&i| deg[i] > 0).map(|i| (deg[i], i)).collect();

    while let Some((_, i)) = queue.pop_first() {
        let j = adj[i]
            .iter()
            .copied()
            .filter(|&j| matched[j].is_none())
            .min_by_key(|&j| (i.abs_diff(j) != 1, deg[j], j));
        let Some(j) = j else { continue };

        matched[i] = Some(j);
        matched[j] = Some(i);
        queue.remove(&(deg[j], j));

        for &m in adj[i].iter().chain(&adj[j]) {
            if matched[m].is_some() {
                continue;
            }
            queue.remove(&(deg[m], m));
            deg[m] -= 1;
            if deg[m] > 0 {
                queue.insert((deg[m], m));
            }
        }
    }

    (0..n)
        .filter_map(|i| match matched[i] {
            None => Some(Piece::Tri(i)),
            Some(j) if j > i => {
                let (q, color) = quads[&(i, j)];
                Some(Piece::Quad { q, color })
            }
            Some(_) => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tri(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> Tri {
        Tri { p: [a.into(), b.into(), c.into()], color: None }
    }

    fn count_quads(pieces: &[Piece]) -> usize {
        pieces.iter().filter(|p| matches!(p, Piece::Quad { .. })).count()
    }

    #[test]
    fn rectangle_becomes_one_quad() {
        let tris = [
            tri([0., 0., 0.], [2., 0., 0.], [2., 1., 0.]),
            tri([0., 0., 0.], [2., 1., 0.], [0., 1., 0.]),
        ];
        let pieces = build(&tris, 0.5);
        assert_eq!(pieces.len(), 1);
        assert_eq!(count_quads(&pieces), 1);
    }

    #[test]
    fn general_quad_stays_two_triangles() {
        // a kite: no pair of parallel edges
        let tris = [
            tri([0., 0., 0.], [2., 0., 0.], [3., 2., 0.]),
            tri([0., 0., 0.], [3., 2., 0.], [-1., 3., 0.]),
        ];
        assert_eq!(build(&tris, 0.5).len(), 2);
    }

    #[test]
    fn trapezoid_is_merged() {
        let tris = [
            tri([0., 0., 0.], [4., 0., 0.], [3., 2., 0.]),
            tri([0., 0., 0.], [3., 2., 0.], [1., 2., 0.]),
        ];
        assert_eq!(count_quads(&build(&tris, 0.5)), 1);
    }

    #[test]
    fn folded_pair_is_not_merged() {
        // second triangle bends out of the plane by 45 degrees
        let tris = [
            tri([0., 0., 0.], [2., 0., 0.], [2., 1., 0.]),
            tri([0., 0., 0.], [2., 1., 0.], [0., 1., 1.]),
        ];
        assert_eq!(build(&tris, 0.5).len(), 2);
    }

    #[test]
    fn disabled_with_zero_angle() {
        let tris = [
            tri([0., 0., 0.], [2., 0., 0.], [2., 1., 0.]),
            tri([0., 0., 0.], [2., 1., 0.], [0., 1., 0.]),
        ];
        assert_eq!(build(&tris, 0.0).len(), 2);
    }

    #[test]
    fn different_colors_are_not_merged() {
        let mut tris = [
            tri([0., 0., 0.], [2., 0., 0.], [2., 1., 0.]),
            tri([0., 0., 0.], [2., 1., 0.], [0., 1., 0.]),
        ];
        tris[0].color = Some([255, 0, 0, 255]);
        tris[1].color = Some([0, 0, 255, 255]);
        assert_eq!(build(&tris, 0.5).len(), 2);
    }

    #[test]
    fn flat_grid_pairs_everything() {
        // 4x4 grid of quads, each split into two triangles: 32 triangles -> 16 blocks
        let mut tris = Vec::new();
        for x in 0..4 {
            for y in 0..4 {
                let (x, y) = (x as f32, y as f32);
                tris.push(tri([x, y, 0.], [x + 1., y, 0.], [x + 1., y + 1., 0.]));
                tris.push(tri([x, y, 0.], [x + 1., y + 1., 0.], [x, y + 1., 0.]));
            }
        }
        let pieces = build(&tris, 0.5);
        assert_eq!(pieces.len(), 16);
        assert_eq!(count_quads(&pieces), 16);
    }

    #[test]
    fn grid_with_alternating_diagonals_pairs_everything() {
        // diagonals flip in a checkerboard, so original quads are not index neighbours
        let mut tris = Vec::new();
        for x in 0..4 {
            for y in 0..4 {
                let (fx, fy) = (x as f32, y as f32);
                if (x + y) % 2 == 0 {
                    tris.push(tri([fx, fy, 0.], [fx + 1., fy, 0.], [fx + 1., fy + 1., 0.]));
                    tris.push(tri([fx, fy, 0.], [fx + 1., fy + 1., 0.], [fx, fy + 1., 0.]));
                } else {
                    tris.push(tri([fx, fy, 0.], [fx + 1., fy, 0.], [fx, fy + 1., 0.]));
                    tris.push(tri([fx + 1., fy, 0.], [fx + 1., fy + 1., 0.], [fx, fy + 1., 0.]));
                }
            }
        }
        assert_eq!(build(&tris, 0.5).len(), 16);
    }

    #[test]
    fn negative_zero_welds() {
        assert_eq!(key(Vec3::new(-0.0, 0.0, 1.0)), key(Vec3::new(0.0, 0.0, 1.0)));
    }
}
