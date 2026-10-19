use clap::Parser;
use midly::Smf;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::error::Error;

use midi2swstruct::generate_music_player;

#[derive(Parser, Debug)]
#[command(name = "midi2swstruct")]
#[command(
    version,
    about = "Converts MIDI-file to SW building",
    long_about = "Converts MIDI-file to Sandbox World structure file with music player, that contains data from MIDI-file."
)]
struct Args {
    /// Input MIDI-file.
    #[arg(value_name = "INPUT", required = true)]
    input: PathBuf,

    /// Optional output path.
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Write output to stdout instead of file.
    #[arg(long, default_value_t = false)]
    stdout: bool,

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
    let args = Args::parse();

    // Read MIDI file
    let mut buffer = Vec::new();
    if args.input == PathBuf::from("-") {
        std::io::stdin().read_to_end(&mut buffer)?;
    } else {
        File::open(&args.input)?.read_to_end(&mut buffer)?;
    }

    let smf = Smf::parse(&buffer)?;

    // Generate building
    let structure = generate_music_player(
        smf,
        args.notes_per_value,
        args.min_pitch,
        args.max_pitch,
        args.min_velocity,
        args.repeat,
        args.max_events_per_func,
    )?;

    // Write output
    let path = match args.output {
        Some(p) => p,
        None => Path::new(args.input.file_name().unwrap()).with_extension("structure"),
    };

    structure.save(path, &Default::default())?;

    Ok(())
}
