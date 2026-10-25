//! glTF → Sandbox World structure.
//!
//! Every triangle (or, when possible, every *quad*) of the model becomes one custom
//! block. See [`params`] for the settings that can be stored in the glTF itself.

mod block;
mod color;
mod params;
mod quad;

use std::{collections::HashSet, error::Error, fmt, path::Path};

use clap::ValueEnum;
use glam::{Mat4, Vec3};
use sw_formats::structure::{Root, Structure};

use color::ColorSource;
use params::Overrides;
use quad::{Piece, Tri};

pub use params::parse_block;

#[derive(Debug)]
pub enum MeshError {
    Gltf(gltf::Error),
    /// A bad value in the glTF's `extras`.
    Param(String),
    /// Geometry that can't be stored in a block.
    TooLarge(String),
    /// A malformed glTF.
    Invalid(String),
    /// A bad command-line/API option.
    Option(String),
}

impl fmt::Display for MeshError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MeshError::Gltf(err) => write!(f, "gltf error: {err}"),
            MeshError::Param(msg) => write!(f, "invalid sw setting in glTF extras, {msg}"),
            MeshError::TooLarge(msg) => write!(f, "{msg}"),
            MeshError::Invalid(msg) => write!(f, "invalid glTF: {msg}"),
            MeshError::Option(msg) => write!(f, "{msg}"),
        }
    }
}

impl Error for MeshError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            MeshError::Gltf(err) => Some(err),
            _ => None,
        }
    }
}

impl From<gltf::Error> for MeshError {
    fn from(value: gltf::Error) -> Self {
        MeshError::Gltf(value)
    }
}

type Result<T> = std::result::Result<T, MeshError>;

/// The game stores at most this many blocks in a structure.
const MAX_BLOCKS: usize = 65535;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Mode {
    /// Fill every face with a thin plate.
    #[default]
    Hollow,
    /// Only the edges, as thin bars.
    Wireframe,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ColorMode {
    /// Paint blocks like the glTF looks: material color, vertex colors, textures,
    /// and any `color` set in the extras.
    #[default]
    Auto,
    /// Don't paint anything.
    None,
}

/// glTF is right-handed and the game (Unity) is left-handed, so one axis has to be
/// mirrored. Faces keep their outward side: winding is flipped to match.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Mirror {
    /// Negate X. The model's front (+Z in glTF) stays +Z, like glTFast does.
    #[default]
    X,
    /// Negate Z. The same mirror image, turned 180° around the vertical axis.
    Z,
    /// Use glTF coordinates as they are.
    None,
}

impl Mirror {
    fn matrix(self) -> Mat4 {
        match self {
            Mirror::X => Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0)),
            Mirror::Z => Mat4::from_scale(Vec3::new(1.0, 1.0, -1.0)),
            Mirror::None => Mat4::IDENTITY,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Options {
    /// Default mode; the glTF may override it.
    pub mode: Mode,
    /// Default thickness; the glTF may override it.
    pub thickness: f32,
    /// Default block ID, which must be a custom-shape block; the glTF may override it.
    pub block: i32,
    pub color: ColorMode,
    /// Largest angle error, in degrees, at which two triangles still merge into one
    /// quad block. `0` turns merging off.
    pub merge_angle: f32,
    /// Everything is multiplied by this.
    pub scale: f32,
    /// Handedness conversion, applied after `scale` and the node transforms.
    pub mirror: Mirror,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            mode: Mode::Hollow,
            thickness: 0.05,
            block: 109,
            color: ColorMode::Auto,
            merge_angle: 0.5,
            scale: 1.0,
            mirror: Mirror::X,
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Stats {
    pub triangles: usize,
    pub blocks: usize,
    /// Blocks that stand for two triangles.
    pub merged_quads: usize,
}

struct Ctx<'a> {
    opts: &'a Options,
    buffers: &'a [gltf::buffer::Data],
    images: &'a [gltf::image::Data],
    out: Structure,
    stats: Stats,
}

fn who(kind: &str, index: usize, name: Option<&str>) -> String {
    match name {
        Some(n) => format!("{kind} #{index} \"{n}\""),
        None => format!("{kind} #{index}"),
    }
}

/// Converts a glTF/GLB file to a structure.
///
/// `base` is the directory that relative `.bin`/image URIs resolve against; pass the
/// input file's directory so multi-file `.gltf` models load.
pub fn generate(data: &[u8], base: Option<&Path>, opts: &Options) -> Result<(Structure, Stats)> {
    if !(opts.thickness > 0.0 && opts.thickness <= block::MAX_SIZE) {
        return Err(MeshError::Option(format!(
            "thickness must be above 0 and at most {} (the game's plate size limit)",
            block::MAX_SIZE
        )));
    }
    if !(opts.scale > 0.0 && opts.scale.is_finite()) {
        return Err(MeshError::Option("scale must be a positive number".into()));
    }

    let gltf::Gltf { document, blob } = gltf::Gltf::from_slice(data)?;
    let buffers = gltf::import_buffers(&document, base, blob)?;

    // decoding textures is slow, so only do it when something will look at them
    let wants_textures = opts.color == ColorMode::Auto
        && document
            .materials()
            .any(|m| m.pbr_metallic_roughness().base_color_texture().is_some());
    let images = if wants_textures {
        gltf::import_images(&document, base, &buffers)?
    } else {
        Vec::new()
    };

    let mut ctx = Ctx {
        opts,
        buffers: &buffers,
        images: &images,
        out: Structure::default(),
        stats: Stats::default(),
    };
    ctx.out.roots.push(Root::default());

    // the default scene, else the first one, else every node nobody is a child of
    let scene_roots: Vec<gltf::Node> = match document
        .default_scene()
        .or_else(|| document.scenes().next())
    {
        Some(scene) => scene.nodes().collect(),
        None => {
            let children: HashSet<usize> = document
                .nodes()
                .flat_map(|n| n.children().map(|c| c.index()))
                .collect();
            document
                .nodes()
                .filter(|n| !children.contains(&n.index()))
                .collect()
        }
    };

    for node in scene_roots {
        visit(&mut ctx, node, Mat4::IDENTITY, &Overrides::default())?;
    }

    check_block_count(&ctx)?;

    ctx.stats.blocks = ctx.out.blocks.len();
    Ok((ctx.out, ctx.stats))
}

/// Faces and edges larger than the game allows are split into several blocks, so a
/// big model can run past the limit; stop as soon as that happens.
fn check_block_count(ctx: &Ctx) -> Result<()> {
    if ctx.out.blocks.len() > MAX_BLOCKS {
        return Err(MeshError::TooLarge(format!(
            "the model needs more than {MAX_BLOCKS} blocks, which is all a structure holds; \
             simplify the mesh or use a smaller --scale (plates are at most 10 units wide)"
        )));
    }
    Ok(())
}

fn visit(ctx: &mut Ctx, node: gltf::Node, parent: Mat4, inherited: &Overrides) -> Result<()> {
    let node_who = who("node", node.index(), node.name());
    // a node's settings also apply to its children
    let inherited = inherited
        .clone()
        .over(Overrides::from_extras(node.extras(), &node_who)?);
    let world = parent * Mat4::from_cols_array_2d(&node.transform().matrix());

    if let Some(mesh) = node.mesh() {
        let mesh_over =
            Overrides::from_extras(mesh.extras(), &who("mesh", mesh.index(), mesh.name()))?;
        let base = inherited.clone().over(mesh_over);
        for (pi, primitive) in mesh.primitives().enumerate() {
            let label = format!(
                "primitive {pi} of {}",
                who("mesh", mesh.index(), mesh.name())
            );
            emit_primitive(ctx, &primitive, &world, &base, &label)?;
        }
    }

    for child in node.children() {
        visit(ctx, child, world, &inherited)?;
    }
    Ok(())
}

fn emit_primitive(
    ctx: &mut Ctx,
    primitive: &gltf::Primitive,
    world: &Mat4,
    base: &Overrides,
    label: &str,
) -> Result<()> {
    if primitive.mode() != gltf::mesh::Mode::Triangles {
        eprintln!("warning: skipping {label}: only triangle lists are supported");
        return Ok(());
    }

    let material = primitive.material();
    let material_over = if material.index().is_some() {
        let w = who(
            "material",
            material.index().unwrap_or_default(),
            material.name(),
        );
        Overrides::from_extras(material.extras(), &w)?
    } else {
        Overrides::default()
    };
    let primitive_over = Overrides::from_extras(primitive.extras(), label)?;
    let settings = base.clone().over(material_over).over(primitive_over);

    let opts = ctx.opts;
    let block_id = settings.block.unwrap_or(opts.block);
    let thickness = settings.thickness.unwrap_or(opts.thickness);
    let mode = settings.mode.unwrap_or(opts.mode);

    let buffers = ctx.buffers;
    let reader = primitive.reader(|b| Some(&buffers[b.index()]));
    let Some(positions) = reader.read_positions() else {
        return Ok(());
    };

    let transform = opts.mirror.matrix() * Mat4::from_scale(Vec3::splat(opts.scale)) * *world;
    // an odd number of mirrors (the handedness fix, negative node scales) reverses winding
    let mirrored = transform.determinant() < 0.0;
    let points: Vec<Vec3> = positions
        .map(|p| transform.transform_point3(Vec3::from(p)))
        .collect();

    let indices: Vec<u32> = match reader.read_indices() {
        Some(i) => i.into_u32().collect(),
        None => (0..points.len() as u32).collect(),
    };

    // colors
    let colors = if opts.color == ColorMode::Auto {
        let uv_set = material
            .pbr_metallic_roughness()
            .base_color_texture()
            .map(|t| t.tex_coord());
        Some(ColorSource::new(
            &material,
            reader.read_colors(0).map(|c| c.into_rgba_f32().collect()),
            uv_set
                .and_then(|set| reader.read_tex_coords(set))
                .map(|t| t.into_f32().collect()),
            ctx.images,
        ))
    } else {
        None
    };
    let explicit = if opts.color == ColorMode::Auto {
        settings.color
    } else {
        None
    };

    let mut tris = Vec::with_capacity(indices.len() / 3);
    for chunk in indices.chunks_exact(3) {
        let mut idx = [chunk[0] as usize, chunk[1] as usize, chunk[2] as usize];
        if idx.iter().any(|&i| i >= points.len()) {
            return Err(MeshError::Invalid(format!(
                "{label} indexes a vertex that doesn't exist"
            )));
        }
        let color = explicit.or_else(|| colors.as_ref().and_then(|c| c.triangle(idx)));
        if mirrored {
            idx.swap(1, 2);
        }
        tris.push(Tri {
            p: [points[idx[0]], points[idx[1]], points[idx[2]]],
            color,
        });
    }
    ctx.stats.triangles += tris.len();

    match mode {
        Mode::Hollow => {
            for piece in quad::build(&tris, opts.merge_angle) {
                let (q, color, merged) = match piece {
                    Piece::Quad { q, color } => (q, color, true),
                    Piece::Tri(i) => (block::triangle_as_quad(tris[i].p), tris[i].color, false),
                };
                let n = block::trapezoid(q, thickness, block_id, color, &mut ctx.out.blocks)?;
                ctx.stats.merged_quads += (merged && n > 0) as usize;
                check_block_count(ctx)?;
            }
        }
        Mode::Wireframe => {
            // Key edges by vertex position, not index, so duplicated vertices
            // (UV/normal seams in glTF) still count as the same edge.
            let mut seen: HashSet<(quad::Key, quad::Key)> = HashSet::new();
            for t in &tris {
                for e in 0..3 {
                    let (a, b) = (t.p[e], t.p[(e + 1) % 3]);
                    let (ka, kb) = (quad::key(a), quad::key(b));
                    if ka == kb {
                        continue; // zero-length edge
                    }
                    // order-independent, so a->b and b->a count as the same edge
                    let edge = if ka < kb { (ka, kb) } else { (kb, ka) };
                    if seen.insert(edge) {
                        block::line(a, b, thickness, block_id, t.color, &mut ctx.out.blocks)?;
                        check_block_count(ctx)?;
                    }
                }
            }
        }
    }

    Ok(())
}
