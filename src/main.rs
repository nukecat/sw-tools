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
    /// Input glTF.
    #[arg(value_name = "INPUT", required = true)]
    input: PathBuf,

    /// Optional output path.
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Mode
    #[arg(short, long, default_value = "hollow")]
    mode: mesh::Mode,

    /// Thickness
    #[arg(short, long, default_value_t = 0.05)]
    thickness: f32,
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

            let structure = mesh::generate(&buffer, args.mode, args.thickness)?;

            let path = match args.output {
                Some(p) => p,
                None => Path::new(args.input.file_name().unwrap()).with_extension("structure"),
            };

            structure.save(path, &Default::default())?;
        }
    }

    Ok(())
}
