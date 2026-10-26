use clap::{Args, Parser, Subcommand};
use std::io::Read;
use std::path::Path;
use std::{error::Error, fs::File, path::PathBuf};
use sw_formats::{catalog::BlockCatalog, structure::Structure};

use sw_tools::*;

#[derive(Parser)]
#[command(name = "sw-tools")]
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
    /// Converts image to the structure
    PixelArt(PixelArtArgs),
    /// Prints whole list of blocks, props or layers
    Catalog(CatalogArgs),
    /// Inserts binary data into microcontroller
    InsertData(InsertDataArgs),
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
    #[arg(long, default_value_t = 27, value_parser = clap::value_parser!(u8).range(0..=127))]
    min_pitch: u8,

    /// Maximal note pitch.
    #[arg(long, default_value_t = 111, value_parser = clap::value_parser!(u8).range(0..=127))]
    max_pitch: u8,

    /// Notes quieter than this are ignored.
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u8).range(0..=127))]
    min_velocity: u8,

    /// If true, music will repeat.
    #[arg(short, long, default_value_t = false)]
    repeat: bool,

    /// Also play the percussion channel (MIDI channel 10), which is skipped by default
    /// because drum notes are not pitches.
    #[arg(long, default_value_t = false)]
    include_drums: bool,

    /// Don't draw anything on the microcontroller's screen.
    #[arg(long, default_value_t = false)]
    no_display: bool,

    /// Title shown on the screen. Defaults to the name of the MIDI file.
    #[arg(long)]
    title: Option<String>,
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
    #[arg(short, long, default_value = "metal", value_parser = parsers::custom_block)]
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

    /// Value stored in every block's state.
    /// A glTF can override this per object with `sw_value` in its extras.
    #[arg(short, long, default_value_t = 0.0)]
    value: f32,
}

#[derive(Args)]
struct PixelArtArgs {
    /// Input image. Many of common formats are supported.
    #[arg(value_name = "INPUT", required = true)]
    input: PathBuf,

    /// Optional output path.
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Optional path to the building that will represent one pixel.
    #[arg(short, long)]
    element: Option<PathBuf>,

    /// Block to build with: metal, wood, glass, neon, or any block by
    /// name or ID.
    #[arg(short, long, default_value_t = 0, value_parser = parsers::not_a_tool)]
    block: i32,

    /// Spacing between blocks
    #[arg(short, long, default_value_t = 1.0)]
    spacing: f32,

    /// Value stored in every block's state.
    #[arg(short, long, default_value_t = 0.0)]
    value: f32,
}

#[derive(Args)]
struct CatalogArgs {
    #[arg(value_name = "CATEGORY", required = true)]
    category: catalog::Category,
}

#[derive(Args)]
struct InsertDataArgs {
    /// Input structure.
    #[arg(value_name = "INPUT", required = true)]
    input: PathBuf,

    /// Input binary whose bytes will be inserted. Use `-` for stdin
    #[arg(value_name = "BINARY", required = true)]
    binary: PathBuf,

    /// Optional output path.
    #[arg(short, long, conflicts_with = "in_place")]
    output: Option<PathBuf>,

    /// Overwrite the input structure.
    #[arg(short, long)]
    in_place: bool,

    /// Optional name to help identify a specific microcontroller block
    #[arg(short, long)]
    name: Option<String>,

    /// Optional base16 string to help identify a specific data array
    #[arg(short, long)]
    magic: Option<String>,
}

fn print_block(id: i32) -> Result<(), Box<dyn Error>> {
    let catalog = BlockCatalog::shared();
    let block_name = catalog
        .get(id)
        .ok_or(format!("unknown block with id {}", id))?
        .name
        .clone();
    println!("Using block with ID {}, name \"{}\"", id, block_name);
    Ok(())
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = Cli::parse();

    match args.command {
        Commands::MusicPlayer(args) => {
            let mut buffer = Vec::new();
            File::open(&args.input)?.read_to_end(&mut buffer)?;

            let title = args.title.unwrap_or_else(|| {
                args.input
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default()
            });

            let structure = music_player::generate(
                &buffer,
                &music_player::Options {
                    min_pitch: args.min_pitch,
                    max_pitch: args.max_pitch,
                    min_velocity: args.min_velocity,
                    repeat: args.repeat,
                    include_drums: args.include_drums,
                    display: !args.no_display,
                    title,
                    ..Default::default()
                },
            )?;

            let path = match args.output {
                Some(p) => p,
                None => Path::new(args.input.file_name().unwrap()).with_extension("structure"),
            };

            structure.save(path, &Default::default())?;
        }
        Commands::Mesh(args) => {
            let mut buffer = Vec::new();
            File::open(&args.input)?.read_to_end(&mut buffer)?;

            print_block(args.block)?;

            let options = mesh::Options {
                mode: args.mode,
                thickness: args.thickness,
                block: args.block,
                color: args.color,
                merge_angle: args.merge_angle,
                scale: args.scale,
                mirror: args.mirror,
                value: args.value,
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
        Commands::PixelArt(args) => {
            let mut buffer = Vec::new();
            File::open(&args.input)?.read_to_end(&mut buffer)?;

            print_block(args.block)?;

            let structure = pixel_art::generate(&buffer, args.block, args.spacing, args.value)?;

            let path = match args.output {
                Some(p) => p,
                None => Path::new(args.input.file_name().unwrap()).with_extension("structure"),
            };

            structure.save(&path, &Default::default())?;
        }
        Commands::Catalog(args) => args.category.print(),
        Commands::InsertData(args) => {
            let mut structure = Structure::load(&args.input)?;

            let mut buffer = Vec::new();
            File::open(&args.binary)?.read_to_end(&mut buffer)?;

            let blob: Option<Vec<u8>> = if let Some(magic) = args.magic {
                Some(hex::decode(&magic)?)
            } else {
                None
            };

            insert_data::insert_data(
                &mut structure,
                &buffer,
                insert_data::Filter {
                    name: args.name,
                    blob: blob,
                },
            )?;

            let path = if args.in_place {
                args.input.clone()
            } else if let Some(p) = args.output {
                p
            } else {
                let p = PathBuf::from(args.input.file_name().unwrap());
                if same_file(&p, &args.input) {
                    return Err(format!(
                        "{} is in the current directory, so the default output would overwrite it; \
                        use --in-place or -o <path>",
                        args.input.display()
                    )
                    .into());
                }
                p
            };

            structure.save(&path, &Default::default())?;
        }
    }

    Ok(())
}
