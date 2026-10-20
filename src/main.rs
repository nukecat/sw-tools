use clap::{Parser, Subcommand, Args};
use std::path::Path;
use std::{error::Error, fs::File, path::PathBuf};
use std::io::Read;

use sw_tools::*;

#[derive(Parser)]
#[command(name = "midi2swstruct")]
#[command(
    version,
    about = "Tools for Sandbox World formats",
    long_about = "Set of tools that can read, generate and manipulate with Sandbox World .structure, .swmap and .save files"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Converts MIDI-file to structure
    MusicPlayer(MusicPlayerArgs),
    /// Converts glTF file to structure
    Mesh(MeshArgs),
}

#[derive(Args)]
pub struct MusicPlayerArgs {
    /// Input MIDI-file.
    #[arg(value_name = "INPUT", required = true)]
    input: PathBuf,

    /// Optional output path.
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Minimal note pitch.
    #[arg(long, default_value_t = 27, value_parser = clap::value_parser!(u8).range(0..127))]
    min_pitch: u8,

    /// Maximal note pitch.
    #[arg(long, default_value_t = 111, value_parser = clap::value_parser!(u8).range(0..127))]
    max_pitch: u8,

    /// Maximal amount of events per function.
    #[arg(long, default_value_t = 1024)]
    max_events_per_func: usize,

    /// Minimal velocity for note to be flagged as active.
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u8).range(0..127))]
    min_velocity: u8,

    /// If true, music will repeat.
    #[arg(short, long, default_value_t = false)]
    repeat: bool,

    /// How many note changes can be encoded in one value.
    #[arg(short, long, default_value_t = 24, value_parser = clap::value_parser!(u8).range(1..=24))]
    notes_per_value: u8,
}

#[derive(Args)]
struct MeshArgs {
    /// Input glTF (.glb or .gltf).
    #[arg(value_name = "INPUT", required = true)]
    input: PathBuf,

    /// Optional output path.
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Hollow fills faces with plates, wireframe only draws the edges.
    /// A glTF can override this per object with `sw_mode` in its extras.
    #[arg(short, long, default_value = "hollow")]
    mode: mesh::Mode,

    /// Thickness of plates and bars.
    /// A glTF can override this per object with `sw_thickness` in its extras.
    #[arg(short, long, default_value_t = 0.05)]
    thickness: f32,

    /// Block to build with: metal, wood, glass, neon, or any custom-shape block by
    /// name or ID. A glTF can override this per object with `sw_block` in its extras.
    #[arg(short, long, default_value = "metal", value_parser = mesh::parse_block)]
    block: i32,

    /// Auto paints blocks from the material color, vertex colors, textures and
    /// `sw_color` extras. None leaves everything unpainted.
    #[arg(short, long, default_value = "auto")]
    color: mesh::ColorMode,

    /// Merge two coplanar triangles into one block when they form a quad with a pair
    /// of parallel edges. The value is the largest angle error in degrees; 0 turns
    /// merging off.
    #[arg(long, default_value_t = 0.5, value_name = "DEGREES")]
    merge_angle: f32,

    /// Multiplies the size of the whole model.
    #[arg(long, default_value_t = 1.0)]
    scale: f32,

    /// glTF is right-handed and the game is left-handed, so one axis is mirrored.
    /// x keeps the model's front facing +Z; z gives the same shape turned 180°
    /// around the vertical axis; none uses glTF coordinates untouched.
    #[arg(long, default_value = "x")]
    mirror: mesh::Mirror,
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = Cli::parse();

    match args.command {
        Commands::MusicPlayer(args) => {
            let mut buffer = Vec::new();
            File::open(&args.input)?.read_to_end(&mut buffer)?;

            let structure = music_player::generate(
                &buffer,
                args.notes_per_value,
                args.min_pitch,
                args.max_pitch,
                args.min_velocity,
                args.repeat,
                args.max_events_per_func,
            )?;

            let path = match args.output {
                Some(p) => p,
                None => Path::new(args.input.file_name().unwrap()).with_extension("structure"),
            };

            structure.save(path, &Default::default())?;
        },
        Commands::Mesh(args) => {
            let mut buffer = Vec::new();
            File::open(&args.input)?.read_to_end(&mut buffer)?;

            let options = mesh::Options {
                mode: args.mode,
                thickness: args.thickness,
                block: args.block,
                color: args.color,
                merge_angle: args.merge_angle,
                scale: args.scale,
                mirror: args.mirror,
            };
            let (structure, stats) = mesh::generate(&buffer, args.input.parent(), &options)?;

            let path = match args.output {
                Some(p) => p,
                None => Path::new(args.input.file_name().unwrap()).with_extension("structure"),
            };

            structure.save(&path, &Default::default())?;
            eprintln!(
                "{} triangles -> {} blocks ({} quads merged) -> {}",
                stats.triangles,
                stats.blocks,
                stats.merged_quads,
                path.display()
            );
        }
    }

    Ok(())
}
