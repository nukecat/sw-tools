use clap::{Parser, Subcommand, Args};
use midly::Smf;
use std::path::Path;
use std::{error::Error, fs::File, path::PathBuf};
use std::io::Read;

use sw_tools::*;

#[derive(Parser)]
#[command(name = "midi2swstruct")]
#[command(
    version,
    about = "Tools for Sandbox World formats",
    long_about = "Set of tools that can read, generate and manipulate Sandbox World .structure, .swmap and .save files"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Converts MIDI-file to structure
    MusicPlayer(MusicPlayerArgs),
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

fn main() -> Result<(), Box<dyn Error>> {
    let args = Cli::parse();

    match args.command {
        Commands::MusicPlayer(args) => {
            let mut buffer = Vec::new();
            File::open(&args.input)?.read_to_end(&mut buffer)?;

            let smf = Smf::parse(&buffer)?;

            let structure = music_player::generate(
                smf,
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
        }
    }

    Ok(())
}
