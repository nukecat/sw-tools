//! MIDI -> Sandbox World structure.
//!
//! The generated structure is tiny: one microcontroller running a PixelScript program, one
//! switch that starts and stops it, and one tone generator per pitch used by the song. The
//! whole song lives in a single `data` table inside the script (see `music_player.pxs`).

use midly::{MetaMessage, MidiMessage, Smf, Timing, TrackEventKind};
use std::error::Error;
use std::fmt::{self, Write};
use sw_formats::primitives::Vec3;
use sw_formats::structure::{
    Block, BlockSettings, CustomData, MicrocontrollerData, Root, Structure,
};

/// Block IDs, see `sw-formats/docs/block-ids.md`.
const SWITCH: i32 = 9;
const TONE_GENERATOR: i32 = 125;
const MICROCONTROLLER: i32 = 138;

/// The script template. `@NAME@` placeholders are filled in by [`render_script`].
const SCRIPT: &str = include_str!("music_player.pxs");

/// Event byte that means "only wait". Every other value is `(key << 1) | on`.
const WAIT: u8 = 0xFF;
/// A microcontroller has 255 output wires at most and `WAIT` takes one key's place.
const MAX_KEYS: usize = 127;
/// Largest delay one event can carry (two bytes, milliseconds).
const MAX_DELAY_MS: u64 = 0xFFFF;
/// A key that is struck again while it is still sounding is released this long before the new
/// note, so the repeat is audible. It must span at least one game tick (20 ms at 50 Hz).
/// Strikes closer together than twice this are one attack as far as the game can tell, and are
/// merged: two instruments doubling a note a few milliseconds apart must not cut each other.
const RETRIGGER_GAP_MS: u64 = 25;
/// The microcontroller header stores the source length in 15 bits.
const MAX_SOURCE_LEN: usize = 0x7FFF;

#[derive(Debug)]
pub enum MusicPlayerError {
    Format(fmt::Error),
    Midi(midly::Error),
    Unsupported(String),
}

impl fmt::Display for MusicPlayerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MusicPlayerError::Unsupported(msg) => write!(f, "unsupported: {msg}"),
            MusicPlayerError::Format(err) => write!(f, "format error: {err}"),
            MusicPlayerError::Midi(err) => write!(f, "midi error: {err}"),
        }
    }
}

impl Error for MusicPlayerError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            MusicPlayerError::Format(err) => Some(err),
            MusicPlayerError::Midi(err) => Some(err),
            _ => None,
        }
    }
}

impl From<fmt::Error> for MusicPlayerError {
    fn from(value: fmt::Error) -> Self {
        Self::Format(value)
    }
}

impl From<midly::Error> for MusicPlayerError {
    fn from(value: midly::Error) -> Self {
        Self::Midi(value)
    }
}

type Result<T> = std::result::Result<T, MusicPlayerError>;

#[derive(Debug, Clone)]
pub struct Options {
    /// Lowest MIDI key that is played.
    pub min_pitch: u8,
    /// Highest MIDI key that is played.
    pub max_pitch: u8,
    /// Note-ons quieter than this are ignored.
    pub min_velocity: u8,
    /// Start over when the song ends.
    pub repeat: bool,
    /// Also play the General MIDI percussion channel (channel 10) as if it were pitched.
    pub include_drums: bool,
    /// Draw a title, key activity, time and a progress bar on the microcontroller's screen.
    pub display: bool,
    /// Title shown on the screen.
    pub title: String,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            min_pitch: 27,
            max_pitch: 111,
            min_velocity: 1,
            repeat: false,
            include_drums: false,
            display: true,
            title: String::new(),
        }
    }
}

/// A key going down or up, `ms` after the start of the song.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct KeyEvent {
    ms: u64,
    /// Index into [`Song::keys`] (and so the microcontroller output that drives the key).
    key: usize,
    on: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Song {
    /// MIDI keys that sound at some point, lowest first.
    keys: Vec<u8>,
    events: Vec<KeyEvent>,
    total_ms: u64,
}

pub fn generate(midi_raw: &[u8], options: &Options) -> Result<Structure> {
    let song = parse_midi(midi_raw, options)?;
    build_structure(&song, options)
}

fn pitch_to_freq(midi: u8) -> f32 {
    440.0 * 2.0_f32.powf((midi as f32 - 69.0) / 12.0)
}

// ---------------------------------------------------------------------------------------------
// MIDI -> timeline
// ---------------------------------------------------------------------------------------------

/// Tempo from `tick` onwards, and the time at which `tick` is reached.
struct TempoPoint {
    tick: u64,
    micros_per_beat: u64,
    at_micros: u128,
}

struct TempoMap {
    ppq: u64,
    points: Vec<TempoPoint>,
}

impl TempoMap {
    /// `changes` is `(tick, microseconds per beat)`. MIDI's default is 120 BPM.
    fn new(ppq: u64, mut changes: Vec<(u64, u32)>) -> TempoMap {
        changes.sort_by_key(|c| c.0); // stable: for equal ticks the last one wins
        let mut points = vec![TempoPoint { tick: 0, micros_per_beat: 500_000, at_micros: 0 }];
        for (tick, tempo) in changes {
            let tempo = u64::from(tempo).max(1);
            let last = points.last_mut().unwrap();
            if tick == last.tick {
                last.micros_per_beat = tempo;
                continue;
            }
            let at_micros = last.at_micros
                + u128::from(tick - last.tick) * u128::from(last.micros_per_beat) / u128::from(ppq);
            points.push(TempoPoint { tick, micros_per_beat: tempo, at_micros });
        }
        TempoMap { ppq, points }
    }

    fn millis(&self, tick: u64) -> u64 {
        let point = &self.points[self.points.partition_point(|p| p.tick <= tick) - 1];
        let micros = point.at_micros
            + u128::from(tick - point.tick) * u128::from(point.micros_per_beat)
                / u128::from(self.ppq);
        ((micros + 500) / 1000) as u64
    }
}

fn parse_midi(midi_raw: &[u8], options: &Options) -> Result<Song> {
    let smf = Smf::parse(midi_raw)?;

    let ppq = match smf.header.timing {
        Timing::Metrical(t) if t.as_int() > 0 => u64::from(t.as_int()),
        Timing::Metrical(_) => {
            return Err(MusicPlayerError::Unsupported("zero ticks per beat".into()));
        }
        Timing::Timecode(_, _) => {
            return Err(MusicPlayerError::Unsupported("timecode".into()));
        }
    };

    // Collect everything from all tracks; tracks are merged by sorting on the tick.
    let mut notes: Vec<(u64, u8, bool)> = Vec::new();
    let mut tempos: Vec<(u64, u32)> = Vec::new();
    let mut end_tick = 0;

    for track in &smf.tracks {
        let mut tick = 0u64;
        for event in track {
            tick += u64::from(event.delta.as_int());
            match event.kind {
                TrackEventKind::Midi { channel, message } => {
                    if channel.as_int() == 9 && !options.include_drums {
                        continue;
                    }
                    let (key, on) = match message {
                        // A note-on with velocity 0 is a note-off. One that is merely too
                        // quiet is ignored, so it can't cut a note that is already sounding.
                        MidiMessage::NoteOn { key, vel } if vel.as_int() == 0 => {
                            (key.as_int(), false)
                        }
                        MidiMessage::NoteOn { vel, .. } if vel.as_int() < options.min_velocity => {
                            continue;
                        }
                        MidiMessage::NoteOn { key, .. } => (key.as_int(), true),
                        MidiMessage::NoteOff { key, .. } => (key.as_int(), false),
                        _ => continue,
                    };
                    if key < options.min_pitch || key > options.max_pitch {
                        continue;
                    }
                    notes.push((tick, key, on));
                }
                TrackEventKind::Meta(MetaMessage::Tempo(t)) => {
                    tempos.push((tick, t.as_int()));
                }
                _ => {}
            }
        }
        end_tick = end_tick.max(tick);
    }

    notes.sort_by_key(|n| n.0);
    let tempo_map = TempoMap::new(ppq, tempos);

    // Several tracks can hold the same key at once, so holders are counted and a key is only
    // reported going down or up when the count crosses zero. A note-on on a key that is already
    // sounding is a new attack though (a repeated note, or two instruments on one pitch): it
    // releases the key briefly first, otherwise repeated notes fuse into one long note.
    let mut holders = [0u32; 128];
    let mut sounding = [false; 128];
    let mut last_down_ms = [0u64; 128];
    // (ms, key, on); the order inside one millisecond is kept (stable sort below).
    let mut changes: Vec<(u64, u8, bool)> = Vec::new();

    let mut i = 0;
    while i < notes.len() {
        let tick = notes[i].0;
        let ms = tempo_map.millis(tick);
        let mut j = i;
        let mut struck = [false; 128];
        while j < notes.len() && notes[j].0 == tick {
            let (_, key, on) = notes[j];
            let count = &mut holders[key as usize];
            *count = if on { *count + 1 } else { count.saturating_sub(1) };
            struck[key as usize] |= on;
            j += 1;
        }
        for &(_, key, _) in &notes[i..j] {
            let k = key as usize;
            let now = holders[k] > 0;
            if now && sounding[k] && struck[k] && ms - last_down_ms[k] >= 2 * RETRIGGER_GAP_MS {
                changes.push((ms - RETRIGGER_GAP_MS, key, false));
                changes.push((ms, key, true));
                last_down_ms[k] = ms;
            } else if now != sounding[k] {
                changes.push((ms, key, now));
                if now {
                    last_down_ms[k] = ms;
                }
            }
            sounding[k] = now;
            struck[k] = false; // handle each key once per tick
        }
        i = j;
    }
    changes.sort_by_key(|c| c.0);

    let mut keys: Vec<u8> = changes.iter().map(|c| c.1).collect();
    keys.sort_unstable();
    keys.dedup();

    if keys.is_empty() {
        return Err(MusicPlayerError::Unsupported(
            "no notes to play (check --min-pitch, --max-pitch, --min-velocity and --include-drums)"
                .into(),
        ));
    }
    if keys.len() > MAX_KEYS {
        return Err(MusicPlayerError::Unsupported(format!(
            "the song uses {} different pitches but a microcontroller can drive {MAX_KEYS}; \
             narrow --min-pitch/--max-pitch",
            keys.len()
        )));
    }

    let events: Vec<KeyEvent> = changes
        .iter()
        .map(|&(ms, key, on)| KeyEvent { ms, key: keys.binary_search(&key).unwrap(), on })
        .collect();

    let last_ms = changes.last().map_or(0, |c| c.0);
    let total_ms = tempo_map.millis(end_tick).max(last_ms);

    Ok(Song { keys, events, total_ms })
}

// ---------------------------------------------------------------------------------------------
// Timeline -> bytes
// ---------------------------------------------------------------------------------------------

/// Three bytes per event: delay in ms (little endian), then `(key << 1) | on`, or [`WAIT`].
/// The table always ends with a wait that lands exactly on the end of the song.
fn encode_events(song: &Song) -> Vec<u8> {
    let mut bytes = Vec::with_capacity((song.events.len() + 1) * 3);
    let mut now = 0;

    fn push(bytes: &mut Vec<u8>, mut delay: u64, value: u8) {
        // Delays longer than two bytes can hold are split into waits.
        while delay > MAX_DELAY_MS {
            bytes.extend_from_slice(&[0xFF, 0xFF, WAIT]);
            delay -= MAX_DELAY_MS;
        }
        bytes.extend_from_slice(&[delay as u8, (delay >> 8) as u8, value]);
    }

    for event in &song.events {
        push(&mut bytes, event.ms - now, ((event.key as u8) << 1) | event.on as u8);
        now = event.ms;
    }
    push(&mut bytes, song.total_ms - now, WAIT);
    bytes
}

// ---------------------------------------------------------------------------------------------
// Bytes -> script -> structure
// ---------------------------------------------------------------------------------------------

/// Keeps the title safe to put inside a PixelScript string literal.
fn sanitize_title(title: &str) -> String {
    title
        .chars()
        .take(40)
        .map(|c| if c.is_ascii_alphanumeric() || " -_.,()+&'".contains(c) { c } else { '_' })
        .collect()
}

fn render_script(song: &Song, options: &Options) -> Result<String> {
    let mut table = String::new();
    for (n, chunk) in encode_events(song).chunks(24).enumerate() {
        if n > 0 {
            table.push_str(",\n    ");
        }
        for (i, byte) in chunk.iter().enumerate() {
            if i > 0 {
                table.push_str(", ");
            }
            write!(table, "{byte}")?;
        }
    }

    // Drop the screen code, if it isn't wanted, along with its marker lines.
    let mut script = String::new();
    let mut skipping = false;
    for line in SCRIPT.lines() {
        match line.trim() {
            "# @DISPLAY_BEGIN@" => skipping = !options.display,
            "# @DISPLAY_END@" => skipping = false,
            _ if !skipping => {
                script.push_str(line);
                script.push('\n');
            }
            _ => {}
        }
    }

    Ok(script
        .replace("@SONG@", &format!("\n    {table}\n"))
        .replace("@KEYS@", &song.keys.len().to_string())
        .replace("@TOTAL_MS@", &song.total_ms.to_string())
        .replace("@REPEAT@", if options.repeat { "1" } else { "0" })
        .replace("@TITLE@", &sanitize_title(&options.title)))
}

fn build_structure(song: &Song, options: &Options) -> Result<Structure> {
    let source = render_script(song, options)?;

    let mut program = MicrocontrollerData::default();
    program.set_source(&source);
    if program.stripped_source.len() > MAX_SOURCE_LEN {
        return Err(MusicPlayerError::Unsupported(format!(
            "generated script is {} bytes, the game allows {MAX_SOURCE_LEN}",
            program.stripped_source.len()
        )));
    }

    // Block indices.
    const MICRO: i32 = 0;
    const PLAY_SWITCH: i32 = 1;
    const FIRST_TONE: i32 = 2;

    let micro_position = Vec3::new(0.0, 0.125, 0.0);
    let switch_position = Vec3::new(0.0, 0.015625, 0.0);
    let tone_gen_position = Vec3::new(0.0, 0.0, 0.0);

    let mut blocks = Vec::with_capacity(2 + song.keys.len());

    // Outputs are numbered in the order of the block's wires, so wire `n` is output `n`: the
    // script's `out(n, ..)` drives the tone generator for key `n`.
    blocks.push(Block {
        name: Some("Music player".into()),
        position: micro_position,
        wires: (0..song.keys.len() as i32).map(|k| FIRST_TONE + k).collect(),
        settings: Some(BlockSettings {
            custom: Some(CustomData::Microcontroller(program)),
            values: vec![0.5],
            ..Default::default()
        }),
        ..Block::new(MICROCONTROLLER, 0)
    });

    // The only block wired *to* the microcontroller, so it is input A.
    blocks.push(Block {
        name: Some("Play".into()),
        position: switch_position,
        wires: vec![MICRO],
        ..Block::new(SWITCH, 0)
    });
    debug_assert_eq!(blocks.len() as i32, FIRST_TONE);
    debug_assert_eq!(PLAY_SWITCH, 1);

    for &key in &song.keys {
        blocks.push(Block {
            position: tone_gen_position,
            settings: Some(BlockSettings {
                values: vec![pitch_to_freq(key), 100.0],
                ..Default::default()
            }),
            ..Block::new(TONE_GENERATOR, 0)
        });
    }

    Ok(Structure { roots: vec![Root::default()], blocks, ..Default::default() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use midly::num::{u15, u24, u28, u4, u7};
    use midly::{Format, Header, Track, TrackEvent};

    fn note(channel: u8, key: u8, vel: u8) -> TrackEventKind<'static> {
        TrackEventKind::Midi {
            channel: u4::new(channel),
            message: MidiMessage::NoteOn { key: u7::new(key), vel: u7::new(vel) },
        }
    }

    fn off(channel: u8, key: u8) -> TrackEventKind<'static> {
        TrackEventKind::Midi {
            channel: u4::new(channel),
            message: MidiMessage::NoteOff { key: u7::new(key), vel: u7::new(0) },
        }
    }

    fn tempo(micros_per_beat: u32) -> TrackEventKind<'static> {
        TrackEventKind::Meta(MetaMessage::Tempo(u24::new(micros_per_beat)))
    }

    /// Builds a 480 ticks-per-beat MIDI file. Each track is a list of `(absolute tick, event)`.
    fn midi(tracks: Vec<Vec<(u32, TrackEventKind<'static>)>>) -> Vec<u8> {
        let tracks: Vec<Track> = tracks
            .into_iter()
            .map(|events| {
                let mut last = 0;
                let mut track: Track = events
                    .into_iter()
                    .map(|(tick, kind)| {
                        let delta = u28::new(tick - last);
                        last = tick;
                        TrackEvent { delta, kind }
                    })
                    .collect();
                track.push(TrackEvent {
                    delta: u28::new(0),
                    kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
                });
                track
            })
            .collect();
        let smf = Smf {
            header: Header::new(Format::Parallel, Timing::Metrical(u15::new(480))),
            tracks,
        };
        let mut bytes = Vec::new();
        smf.write_std(&mut bytes).unwrap();
        bytes
    }

    fn decode(bytes: &[u8]) -> Vec<(u64, Option<(usize, bool)>)> {
        bytes
            .chunks(3)
            .map(|c| {
                let delay = u64::from(c[0]) | u64::from(c[1]) << 8;
                let event = (c[2] != WAIT).then(|| ((c[2] >> 1) as usize, c[2] & 1 == 1));
                (delay, event)
            })
            .collect()
    }

    fn ev(ms: u64, key: usize, on: bool) -> KeyEvent {
        KeyEvent { ms, key, on }
    }

    #[test]
    fn tempo_changes_are_applied() {
        // 480 ticks = 500 ms at the default 120 BPM, then the tempo doubles.
        let raw = midi(vec![vec![
            (0, note(0, 60, 100)),
            (480, off(0, 60)),
            (480, tempo(250_000)),
            (480, note(0, 62, 100)),
            (960, off(0, 62)),
        ]]);
        let song = parse_midi(&raw, &Options::default()).unwrap();
        assert_eq!(song.keys, vec![60, 62]);
        assert_eq!(
            song.events,
            vec![ev(0, 0, true), ev(500, 0, false), ev(500, 1, true), ev(750, 1, false)]
        );
        assert_eq!(song.total_ms, 750);
    }

    #[test]
    fn missing_tempo_means_120_bpm() {
        let raw = midi(vec![vec![(0, note(0, 60, 100)), (960, off(0, 60))]]);
        let song = parse_midi(&raw, &Options::default()).unwrap();
        assert_eq!(song.total_ms, 1000);
    }

    #[test]
    fn overlapping_notes_on_one_key_are_merged() {
        let raw = midi(vec![
            vec![(0, note(0, 60, 100)), (480, off(0, 60))],
            vec![(240, note(1, 60, 100)), (720, off(1, 60))],
        ]);
        let song = parse_midi(&raw, &Options::default()).unwrap();
        // Down at 0. The second track strikes the key again at 250 (released 30 ms before),
        // and it only comes up when the *last* holder lets go at 750.
        assert_eq!(
            song.events,
            vec![ev(0, 0, true), ev(225, 0, false), ev(250, 0, true), ev(750, 0, false)]
        );
    }

    #[test]
    fn drums_velocity_and_pitch_range() {
        let raw = midi(vec![vec![
            (0, note(9, 40, 100)),   // drums
            (0, note(0, 60, 100)),   // fine
            (0, note(0, 62, 3)),     // too quiet
            (0, note(0, 10, 100)),   // below the range
            (480, note(0, 60, 0)),   // note-on with velocity 0 is a note-off
        ]]);
        let song = parse_midi(&raw, &Options { min_velocity: 10, ..Options::default() }).unwrap();
        assert_eq!(song.keys, vec![60]);
        assert_eq!(song.events, vec![ev(0, 0, true), ev(500, 0, false)]);

        let with_drums = Options { include_drums: true, ..Options::default() };
        assert_eq!(parse_midi(&raw, &with_drums).unwrap().keys, vec![40, 60, 62]);
    }

    #[test]
    fn repeated_note_is_released_before_the_next_strike() {
        // The first note ends on the very tick the second one starts.
        let raw = midi(vec![vec![
            (0, note(0, 60, 100)),
            (480, off(0, 60)),
            (480, note(0, 60, 100)),
            (960, off(0, 60)),
        ]]);
        let song = parse_midi(&raw, &Options::default()).unwrap();
        assert_eq!(
            song.events,
            vec![ev(0, 0, true), ev(475, 0, false), ev(500, 0, true), ev(1000, 0, false)]
        );
    }

    #[test]
    fn strikes_a_few_milliseconds_apart_are_one_attack() {
        // Two tracks doubling a note 5 ms apart: no gap may be cut between them.
        let raw = midi(vec![
            vec![(0, note(0, 60, 100)), (480, off(0, 60))],
            vec![(5, note(1, 60, 100)), (480, off(1, 60))],
        ]);
        let song = parse_midi(&raw, &Options::default()).unwrap();
        assert_eq!(song.events, vec![ev(0, 0, true), ev(500, 0, false)]);
    }

    #[test]
    fn a_quiet_note_on_does_not_cut_a_sounding_note() {
        let raw = midi(vec![vec![
            (0, note(0, 60, 100)),
            (480, note(0, 60, 2)), // ignored, not an off
            (960, off(0, 60)),
        ]]);
        let song = parse_midi(&raw, &Options { min_velocity: 10, ..Options::default() }).unwrap();
        assert_eq!(song.events, vec![ev(0, 0, true), ev(1000, 0, false)]);
    }

    #[test]
    fn silence_is_an_error() {
        let raw = midi(vec![vec![(0, note(9, 40, 100))]]);
        assert!(parse_midi(&raw, &Options::default()).is_err());
    }

    #[test]
    fn events_encode_and_long_gaps_are_split() {
        let song = Song {
            keys: vec![60, 64],
            events: vec![ev(10, 0, true), ev(10, 1, true), ev(200_000, 0, false)],
            total_ms: 300_000,
        };
        let decoded = decode(&encode_events(&song));
        let mut now = 0;
        let mut seen = Vec::new();
        for (delay, event) in &decoded {
            now += delay;
            if let Some((key, on)) = event {
                seen.push(ev(now, *key, *on));
            }
        }
        assert_eq!(seen, song.events);
        assert_eq!(now, 300_000, "the table must end exactly at the end of the song");
        assert!(decoded.iter().all(|(delay, _)| *delay <= MAX_DELAY_MS));
        assert_eq!(decoded.last().unwrap().1, None);
    }

    #[test]
    fn structure_round_trips() {
        let raw = midi(vec![vec![
            (0, note(0, 69, 100)),
            (0, note(0, 60, 100)),
            (480, off(0, 69)),
            (480, off(0, 60)),
        ]]);
        let options = Options { repeat: true, title: "Test \"song\"".into(), ..Options::default() };
        let structure = generate(&raw, &options).unwrap();
        let reread = Structure::from_bytes(&structure.to_bytes(&Default::default()).unwrap()).unwrap();

        // microcontroller, switch, and a tone generator per key (lowest first)
        assert_eq!(reread.blocks.len(), 4);
        assert_eq!(reread.blocks[0].id, MICROCONTROLLER);
        assert_eq!(reread.blocks[0].wires, vec![2, 3]);
        assert_eq!(reread.blocks[1].id, SWITCH);
        assert_eq!(reread.blocks[1].wires, vec![0]);
        for (block, freq) in reread.blocks[2..].iter().zip([261.6256, 440.0]) {
            assert_eq!(block.id, TONE_GENERATOR);
            let values = &block.settings.as_ref().unwrap().values;
            assert!((values[0] - freq).abs() < 0.01, "{values:?}");
        }

        let Some(CustomData::Microcontroller(program)) =
            reread.blocks[0].settings.as_ref().unwrap().custom.as_ref()
        else {
            panic!("no program");
        };
        let source = program.source();
        assert!(!source.contains('@'), "unfilled placeholder:\n{source}");
        assert!(source.contains("keys = 2\n"));
        assert!(source.contains("total_ms = 500\n"));
        assert!(source.contains("repeat = 1\n"));
        assert!(source.contains("\"Test _song_\""));
        assert_eq!(program.data_blobs.len(), 1, "the song must be stored as one raw blob");
        assert_eq!(program.data_blobs[0], encode_events(&parse_midi(&raw, &options).unwrap()));
    }

    #[test]
    fn display_can_be_left_out() {
        let raw = midi(vec![vec![(0, note(0, 60, 100)), (480, off(0, 60))]]);
        let source = |display| {
            let structure = generate(&raw, &Options { display, ..Options::default() }).unwrap();
            match structure.blocks[0].settings.as_ref().unwrap().custom.as_ref() {
                Some(CustomData::Microcontroller(program)) => program.source(),
                _ => panic!(),
            }
        };
        assert!(source(true).contains("draw_keys"));
        let plain = source(false);
        assert!(!plain.contains("draw_keys") && !plain.contains("DISPLAY"));
        assert!(plain.contains("out(k + 1, held[k])"));
    }
}
