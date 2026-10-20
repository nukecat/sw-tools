use std::{error::Error, fmt};
use glam::{EulerRot, Mat3, Quat, Vec3};
use sw_formats::{primitives::Vec3 as SwVec3, structure::{Block, BlockSettings, Root, Structure}};
use clap::ValueEnum;
use std::collections::HashSet;

#[derive(Debug)]
pub enum MeshError {
    Gltf(gltf::Error)
}

impl fmt::Display for MeshError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MeshError::Gltf(err) => write!(f, "gltf error: {err}"),
        }
    }
}

impl Error for MeshError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            MeshError::Gltf(err) => Some(err),
            // _ => None,
        }
    }
}

impl From<gltf::Error> for MeshError {
    fn from(value: gltf::Error) -> Self {
        MeshError::Gltf(value)
    }
}

type Result<T> = std::result::Result<T, MeshError>;

#[derive(Copy, Clone)]
struct Triangle {
    a: Vec3,
    b: Vec3,
    c: Vec3,
}

fn unity_euler_deg(q: Quat) -> SwVec3 {
    // Unity order: Z, then X, then Y  ==  glam YXZ
    let (y, x, z) = q.to_euler(EulerRot::YXZ);
    SwVec3::new(x.to_degrees(), y.to_degrees(), z.to_degrees())
}

fn line(a: Vec3, b: Vec3, thickness: f32) -> Block {
    let dir = b - a;
    let len = dir.length();

    let rot = if len > 0.0001 {
        Quat::from_rotation_arc(Vec3::Y, dir / len)
    } else {
        Quat::IDENTITY
    };

    Block {
        position: SwVec3::new(a.x, a.y, a.z),
        rotation: unity_euler_deg(rot),
        settings: Some(BlockSettings {
            vectors: vec![
                SwVec3::new(0.0, len, 0.0),
                SwVec3::new(thickness, 0.0, thickness),
                SwVec3::new(0.0, 0.0, 0.0),
                SwVec3::new(thickness, 0.0, thickness),
            ],
            ..Default::default()
        }),
        ..Block::new(109, 0)
    }
}

fn triangle_to_custom_block(t: Triangle, base: Block, thickness: f32) -> Option<Block> {
    let ab = t.b - t.a;
    let ac = t.c - t.a;

    let normal = ab.cross(ac);
    if ab.length() < 1e-6 || normal.length() < 1e-9 {
        return None; // degenerate triangle
    }

    let x_axis = ab.normalize();             // base edge
    let z_axis = normal.normalize();         // triangle normal (thickness direction)
    let y_axis = z_axis.cross(x_axis);       // in-plane, points toward c; right-handed

    let rot_mat = Mat3::from_cols(x_axis, y_axis, z_axis); // local -> world
    let to_local = rot_mat.transpose();                    // world -> local

    let p2 = to_local * ab; // (width, 0, 0)
    let p3 = to_local * ac; // (apex_x, height, 0)

    let width = p2.x;
    let apex_x = p3.x;
    let height = p3.y;

    Some(Block {
        position: SwVec3::new(t.a.x, t.a.y, t.a.z),
        rotation: unity_euler_deg(Quat::from_mat3(&rot_mat)),
        settings: Some(BlockSettings {
            vectors: vec![
                // top plane: position, then xz size (stored as x, 0, z)
                SwVec3::new(apex_x, height, -thickness / 2.0),
                SwVec3::new(0.0, 0.0, thickness),
                // bottom plane: position, then xz size
                SwVec3::new(width / 2.0, 0.0, -thickness / 2.0),
                SwVec3::new(width, 0.0, thickness),
            ],
            ..Default::default()
        }),
        ..base
    })
}

#[derive(Default, Clone, ValueEnum)]
pub enum Mode {
    #[default]
    Hollow,
    Wireframe,
}

pub fn generate(glb: &[u8], mode: Mode, thickness: f32) -> Result<Structure> {
    let (document, buffers, _images) = gltf::import_slice(&glb)?;

    let mut s = Structure::default();

    s.roots.push(Root::default());

    for mesh in document.meshes() {
        for primitive in mesh.primitives() {
            let reader = primitive.reader(|buffer| Some(&buffers[buffer.index()]));

            let positions: Vec<[f32; 3]> = match reader.read_positions() {
                Some(iter) => iter.collect(),
                None => continue,
            };

            use gltf::mesh::util::ReadIndices;
            let indices: Vec<u32> = match reader.read_indices() {
                Some(ReadIndices::U8(iter)) => iter.map(|i| i as u32).collect(),
                Some(ReadIndices::U16(iter)) => iter.map(|i| i as u32).collect(),
                Some(ReadIndices::U32(iter)) => iter.collect(),
                None => (0..positions.len() as u32).collect()
            };

            match mode {
                Mode::Hollow => {
                    let mut triangles = Vec::with_capacity(indices.len() / 3);

                    for chunk in indices.chunks_exact(3) {
                        let i0 = chunk[0] as usize;
                        let i1 = chunk[1] as usize;
                        let i2 = chunk[2] as usize;

                        let triangle = Triangle {
                            a: positions[i0].into(),
                            b: positions[i1].into(),
                            c: positions[i2].into(),
                        };

                        triangles.push(triangle);
                    }

                    for t in triangles {
                        if let Some(b) = triangle_to_custom_block(t, Block::new(109, 0), thickness) {
                            s.blocks.push(b);
                        }
                    }
                },
                Mode::Wireframe => {
                    // Key edges by vertex position, not index, so duplicated vertices
                    // (UV/normal seams in glTF) still count as the same edge.
                    let key = |i: u32| {
                        let p = positions[i as usize];
                        [p[0].to_bits(), p[1].to_bits(), p[2].to_bits()]
                    };

                    let mut seen: HashSet<([u32; 3], [u32; 3])> = HashSet::new();

                    for chunk in indices.chunks_exact(3) {
                        let edges = [
                            (chunk[0], chunk[1]),
                            (chunk[1], chunk[2]),
                            (chunk[2], chunk[0]),
                        ];

                        for (i, j) in edges {
                            let (ka, kb) = (key(i), key(j));
                            if ka == kb {
                                continue; // zero-length edge
                            }

                            // order-independent key so a->b and b->a count as the same edge
                            let edge = if ka < kb { (ka, kb) } else { (kb, ka) };
                            if !seen.insert(edge) {
                                continue;
                            }

                            s.blocks.push(line(
                                positions[i as usize].into(),
                                positions[j as usize].into(),
                                thickness,
                            ));
                        }
                    }
                }
            }
        }
    }

    Ok(s)
}
