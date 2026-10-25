//! MIDI -> Sandbox World structure.
//!
//! The generated structure is small: a microcontroller running a PixelScript program, one switch
//! that starts and stops it, and one tone generator per pitch used by the song. The song lives in
//! a `data` table inside the script (see `part.pxs`).
//!
//! The screen is a second microcontroller (see `display.pxs`). It never plays anything: it learns
//! which keys are down from one OR gate per key, which sit between the player and the tone
//! generator and also feed the screen, and it counts the elapsed time itself from the switch:
//!
//! ```text
//! switch -> player ---> OR gate (per key) ---> tone generator
//!    |                        |
//!    +------> display <-------+
//! ```
//!
//! The switch is listed first, so it is the screen's input A, and the OR gates follow in the
//! order of the keys, so key `n` is input `n + 1`.
//!
//! A script is limited in size, and when a structure is sent over multiplayer the `data` tables
//! are turned back into text (`0, 12, 255, ...`), so that limit applies to the expanded script. A
//! long song does not fit in one microcontroller, so it is split **in time** into parts, one per
//! microcontroller, chained one after another:
//!
//! ```text
//! switch -> MC 1 -> MC 2 -> MC 3        (every wire costs one tick)
//!             |       |       |
//!             +-------+-------+--> one OR gate per key --> tone generator
//! ```
//!
//! A microcontroller plays its part and raises its output to the next one on the very tick its
//! part ends, so the next one starts on the following tick, exactly where the song goes on. Keys
//! that are still down at the boundary are pressed again by the next part before the previous one
//! lets go, and the OR gates keep them down. With `repeat`, the last microcontroller also tells
//! the first one that the song is over.

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
const OR_GATE: i32 = 78;
const MICROCONTROLLER: i32 = 138;

/// The script template of a player. `@NAME@` placeholders are filled in by [`render_script`].
const SCRIPT: &str = include_str!("part.pxs");
/// The script template of the screen, filled in by [`display_program`].
const DISPLAY_SCRIPT: &str = include_str!("display.pxs");

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
/// No part of a chained song is shorter than this many ticks. The microcontrollers reset one
/// after another when the song restarts, and this keeps each one reset before its turn comes.
const MIN_PART_TICKS: u64 = 8;
/// Display microcontroller position
const DISPLAY_MC_POS: Vec3 = Vec3 { x: 0.0, y: 0.125, z: 0.0 };
/// Position of microcontrollers that store and play song
const MICROCONTROLLER_POS: Vec3 = Vec3 { x: 0.0, y: -0.125, z: 0.0 };

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
    /// Add a second microcontroller whose screen shows the title, the keys that are down, the time
    /// and a progress bar. It needs an OR gate per key, also when the song fits in one player.
    pub display: bool,
    /// Title shown on the screen.
    pub title: String,
    /// Longest script one microcontroller may have, counted the way multiplayer sees it: with
    /// the `data` tables written out as text. A song that needs more is split in time over
    /// several microcontrollers.
    pub max_source_len: usize,
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
            max_source_len: MAX_SOURCE_LEN,
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
// Sound preview
// ---------------------------------------------------------------------------------------------

/// Length of a game tick in milliseconds (50 Hz). The microcontroller only acts on ticks, so an
/// event lands on the first tick at or after its time.
pub const TICK_MS: u64 = 20;

/// A tone generator going on or off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToneEvent {
    /// Time since the switch was turned on, as the game plays it (a multiple of [`TICK_MS`]).
    pub ms: u64,
    /// Index into [`Playback::frequencies`].
    pub tone: usize,
    pub on: bool,
}

/// What the tone generators of the generated structure do over time, for previewing the sound.
///
/// Every tone generator makes a plain sine wave at its fixed frequency that is switched on and
/// off by its microcontroller output: no envelope or other shaping. The game smooths the volume
/// change very slightly; how much is left to whatever plays this.
#[derive(Debug, Clone, PartialEq)]
pub struct Playback {
    /// Hz of each tone generator, lowest first (the same order as the structure's blocks).
    pub frequencies: Vec<f32>,
    pub events: Vec<ToneEvent>,
    /// When the song is over (and, with `repeat`, starts again), as the game plays it.
    pub total_ms: u64,
}

fn on_tick(ms: u64) -> u64 {
    // The first tick already spends `dt`, so even an event at 0 ms fires one tick in.
    ms.div_ceil(TICK_MS).max(1) * TICK_MS
}

/// Same parsing as [`generate`] (so the same options apply), but returns the sound instead of
/// the structure. `repeat`, `display` and `title` have no effect on it.
pub fn playback(midi_raw: &[u8], options: &Options) -> Result<Playback> {
    let song = parse_midi(midi_raw, options)?;
    Ok(Playback {
        frequencies: song.keys.iter().map(|&k| pitch_to_freq(k)).collect(),
        events: song
            .events
            .iter()
            .map(|e| ToneEvent { ms: on_tick(e.ms), tone: e.key, on: e.on })
            .collect(),
        total_ms: on_tick(song.total_ms),
    })
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

/// Keeps the lines of `template` that belong to the chosen `flags`. `# @IF name@`, `# @ELSE@` and
/// `# @END@` mark conditional parts (they may be nested); the marker lines are dropped.
fn preprocess(template: &str, flags: &[&str]) -> String {
    // (enclosing part is on, condition, in the else branch)
    let mut stack: Vec<(bool, bool, bool)> = Vec::new();
    let on = |stack: &[(bool, bool, bool)]| stack.last().is_none_or(|&(p, c, e)| p && (c != e));
    let mut out = String::new();
    for line in template.lines() {
        let trimmed = line.trim();
        if let Some(name) = trimmed.strip_prefix("# @IF ").and_then(|r| r.strip_suffix('@')) {
            let parent = on(&stack);
            stack.push((parent, flags.contains(&name), false));
        } else if trimmed == "# @ELSE@" {
            stack.last_mut().expect("@ELSE@ without @IF").2 = true;
        } else if trimmed == "# @END@" {
            stack.pop().expect("@END@ without @IF");
        } else if on(&stack) {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// A byte table as the text of a PixelScript array literal.
fn format_table(bytes: &[u8]) -> Result<String> {
    let mut table = String::new();
    for (n, chunk) in bytes.chunks(24).enumerate() {
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
    Ok(format!("\n    {table}\n"))
}

/// What a microcontroller that is one link of a chain needs to know besides its part of the song.
#[derive(Debug, Clone)]
struct Link {
    /// Index in the whole song of the key behind each of the part's outputs.
    ids: Vec<usize>,
    /// The part tells another microcontroller (the next, or for the last one the first) when it
    /// is over, through the output after the keys.
    has_link: bool,
    /// This is the first microcontroller of a repeating song; its input B is the last one.
    restarts: bool,
}

/// `song` is the whole song, or one part of it when `link` is given. `whole_ms` is always the
/// length of the whole song.
fn render_script(
    song: &Song,
    whole_ms: u64,
    options: &Options,
    link: Option<&Link>,
) -> Result<String> {
    let mut flags = Vec::new();
    if link.is_some() {
        flags.push("chain");
    }
    let script = preprocess(SCRIPT, &flags);

    let mut script = script
        .replace("@SONG@", &format_table(&encode_events(song))?)
        .replace("@KEYS@", &song.keys.len().to_string())
        .replace("@TOTAL_MS@", &whole_ms.to_string())
        .replace("@REPEAT@", if options.repeat { "1" } else { "0" });
    if let Some(link) = link {
        let ids: Vec<u8> = link.ids.iter().map(|&id| id as u8).collect();
        script = script
            .replace("@KEY_IDS@", &format_table(&ids)?)
            .replace("@HAS_LINK@", if link.has_link { "1" } else { "0" })
            .replace("@RESTARTS@", if link.restarts { "1" } else { "0" });
    }
    Ok(script)
}

/// The screen's script. `whole_ms` is the time after which the song starts over (or stops).
fn display_program(song: &Song, whole_ms: u64, options: &Options) -> Result<MicrocontrollerData> {
    let pitches = format_table(&song.keys)?;
    let script = DISPLAY_SCRIPT
        .replace("@PITCHES@", &pitches)
        .replace("@KEYS@", &song.keys.len().to_string())
        .replace("@TOTAL_MS@", &whole_ms.to_string())
        .replace("@REPEAT@", if options.repeat { "1" } else { "0" })
        .replace("@TITLE@", &sanitize_title(&options.title));
    let mut program = MicrocontrollerData::default();
    program.set_source(&script);
    check_fits(&program, options)?;
    Ok(program)
}

fn build_program(
    song: &Song,
    whole_ms: u64,
    options: &Options,
    link: Option<&Link>,
) -> Result<MicrocontrollerData> {
    let mut program = MicrocontrollerData::default();
    program.set_source(&render_script(song, whole_ms, options, link)?);
    Ok(program)
}

/// What multiplayer has to carry for a program: the script with its `data` tables as text.
fn expanded_len(program: &MicrocontrollerData) -> usize {
    program.source().len()
}

fn check_fits(program: &MicrocontrollerData, options: &Options) -> Result<()> {
    let limit = options.max_source_len.min(MAX_SOURCE_LEN);
    let len = expanded_len(program);
    if len > limit || program.stripped_source.len() > MAX_SOURCE_LEN {
        return Err(MusicPlayerError::Unsupported(format!(
            "generated script is {len} characters, the limit is {limit}"
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Splitting in time
// ---------------------------------------------------------------------------------------------

/// A key going down or up on a game tick (counted from 1: the first tick of playback).
#[derive(Debug, Clone, Copy)]
struct TickEvent {
    tick: u64,
    /// Index into [`Song::keys`].
    key: usize,
    on: bool,
}

/// The song as the game plays it: events snapped to ticks (see [`on_tick`]).
struct Timeline<'a> {
    song: &'a Song,
    events: Vec<TickEvent>,
    /// Ticks until the song is over.
    ticks: u64,
}

impl<'a> Timeline<'a> {
    fn new(song: &'a Song) -> Self {
        Timeline {
            song,
            events: song
                .events
                .iter()
                .map(|e| TickEvent { tick: on_tick(e.ms) / TICK_MS, key: e.key, on: e.on })
                .collect(),
            ticks: on_tick(song.total_ms) / TICK_MS,
        }
    }

    /// The part of the song from tick `start + 1` to tick `end`, as the microcontroller that
    /// plays it sees it. Its first tick is local tick 1, and an event of local tick `n` is placed
    /// in the middle of the tick before it (`n * 20 - 10` ms), so it fires on tick `n` whatever
    /// the rounding of the game's clock.
    fn part(&self, start: u64, end: u64) -> Part {
        let local_ms = |tick: u64| (tick - start) * TICK_MS - TICK_MS / 2;

        // Keys that are already down when the part starts get pressed on its first tick.
        let mut down = vec![false; self.song.keys.len()];
        for e in self.events.iter().filter(|e| e.tick <= start) {
            down[e.key] = e.on;
        }
        let inside = || self.events.iter().filter(|e| e.tick > start && e.tick <= end);

        let mut ids: Vec<usize> = (0..down.len()).filter(|&k| down[k]).collect();
        ids.extend(inside().map(|e| e.key));
        ids.sort_unstable();
        ids.dedup();
        let local = |key: usize| ids.binary_search(&key).unwrap();

        let mut events: Vec<KeyEvent> = (0..down.len())
            .filter(|&k| down[k])
            .map(|k| KeyEvent { ms: local_ms(start + 1), key: local(k), on: true })
            .collect();
        events.extend(
            inside().map(|e| KeyEvent { ms: local_ms(e.tick), key: local(e.key), on: e.on }),
        );

        Part {
            song: Song {
                keys: ids.iter().map(|&k| self.song.keys[k]).collect(),
                events,
                total_ms: local_ms(end),
            },
            ids,
        }
    }
}

/// One microcontroller's share of a chained song.
struct Part {
    /// Its keys and events, in the part's own time.
    song: Song,
    /// Index in the whole song of each of its keys.
    ids: Vec<usize>,
}

impl Part {
    fn link(&self, has_link: bool, restarts: bool) -> Link {
        Link { ids: self.ids.clone(), has_link, restarts }
    }
}

enum Plan {
    /// The whole song in one microcontroller.
    Single,
    /// Tick ranges `(start, end]` of the parts, in order.
    Chain(Vec<(u64, u64)>),
}

fn plan(song: &Song, options: &Options) -> Result<Plan> {
    let limit = options.max_source_len.min(MAX_SOURCE_LEN);
    if expanded_len(&build_program(song, song.total_ms, options, None)?) <= limit {
        return Ok(Plan::Single);
    }

    let timeline = Timeline::new(song);
    let total = timeline.ticks;
    // Measured the way the biggest script of the chain would look.
    let size = |start: u64, end: u64| -> Result<usize> {
        let part = timeline.part(start, end);
        let link = part.link(true, true);
        Ok(expanded_len(&build_program(&part.song, song.total_ms, options, Some(&link))?))
    };

    let mut parts = Vec::new();
    let mut start = 0;
    while start < total {
        if size(start, total)? <= limit {
            parts.push((start, total));
            break;
        }
        let shortest = start + MIN_PART_TICKS;
        if shortest >= total || size(start, shortest)? > limit {
            return Err(MusicPlayerError::Unsupported(format!(
                "{MIN_PART_TICKS} ticks of the song at {:.1} s need more characters of script than \
                 the limit of {limit}",
                (start * TICK_MS) as f64 / 1000.0
            )));
        }
        // Longest part from `start` that fits; `lo` always does.
        let (mut lo, mut hi) = (shortest, total - 1);
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            if size(start, mid)? <= limit {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        parts.push((start, lo));
        start = lo;
    }

    if parts.len() == 1 {
        return Ok(Plan::Single);
    }
    // When the song restarts the first microcontroller starts over while the signal that the
    // song was over still travels back along the chain; it must be gone by the time the first
    // part ends.
    if options.repeat && parts[0].1 < parts.len() as u64 + 2 {
        return Err(MusicPlayerError::Unsupported(
            "the first part of the song is too short to repeat it; raise --max-source-len".into(),
        ));
    }
    Ok(Plan::Chain(parts))
}

// ---------------------------------------------------------------------------------------------
// Structures
// ---------------------------------------------------------------------------------------------

fn tone_generator(key: u8) -> Block {
    Block {
        position: Vec3::new(0.0, 0.0, 0.0),
        settings: Some(BlockSettings {
            values: vec![pitch_to_freq(key), 100.0],
            ..Default::default()
        }),
        ..Block::new(TONE_GENERATOR, 0)
    }
}

fn microcontroller(name: &str, wires: Vec<i32>, program: MicrocontrollerData) -> Block {
    Block {
        name: Some(name.into()),
        wires,
        settings: Some(BlockSettings {
            custom: Some(CustomData::Microcontroller(program)),
            values: vec![0.5],
            ..Default::default()
        }),
        ..Block::new(MICROCONTROLLER, 0)
    }
}

/// The order of the blocks in the structure. It is not only bookkeeping: the blocks wired *to* a
/// microcontroller are its inputs A, B, ... in the order of the file. So the switch comes first
/// (it is input A of the first player and of the screen), then the players, the screen, an OR gate
/// per key and a tone generator per key.
///
/// Without a screen a song that fits in one microcontroller needs no OR gates: the player drives
/// the tone generators directly. Otherwise every key gets one, because they are what the screen
/// listens to.
struct Layout {
    players: usize,
    keys: usize,
    display: bool,
}

impl Layout {
    fn gates(&self) -> bool {
        self.display || self.players > 1
    }

    fn player(&self, part: usize) -> i32 {
        1 + part as i32
    }

    fn screen(&self) -> i32 {
        debug_assert!(self.display);
        1 + self.players as i32
    }

    fn gate(&self, key: usize) -> i32 {
        debug_assert!(self.gates());
        (1 + self.players + self.display as usize + key) as i32
    }

    fn tone(&self, key: usize) -> i32 {
        let gates = if self.gates() { self.keys } else { 0 };
        (1 + self.players + self.display as usize + gates + key) as i32
    }

    /// The block a player's output for `key` is wired to.
    fn target(&self, key: usize) -> i32 {
        if self.gates() { self.gate(key) } else { self.tone(key) }
    }

    fn len(&self) -> usize {
        self.tone(self.keys) as usize
    }
}

/// The song in one microcontroller, or in a chain of them (see the module documentation), with
/// the screen and the blocks between them and the tone generators.
fn build_structure(song: &Song, options: &Options) -> Result<Structure> {
    let plan = plan(song, options)?;
    let timeline = Timeline::new(song);
    let keys = song.keys.len();
    let (count, whole_ms) = match &plan {
        Plan::Single => (1, song.total_ms),
        // The parts add up to whole ticks, and so does the time the song takes to start over.
        Plan::Chain(ranges) => (ranges.len(), timeline.ticks * TICK_MS),
    };
    let layout = Layout { players: count, keys, display: options.display };

    let mut blocks = Vec::with_capacity(layout.len());

    // The switch drives the first player (its only input, or the first of two when repeating)
    // and the screen, which counts the time from it.
    let mut switch_wires = vec![layout.player(0)];
    if layout.display {
        switch_wires.push(layout.screen());
    }
    blocks.push(Block {
        name: Some("Play".into()),
        position: Vec3::new(0.0, 0.015625, 0.0),
        wires: switch_wires,
        ..Block::new(SWITCH, 0)
    });

    match &plan {
        Plan::Single => {
            let program = build_program(song, song.total_ms, options, None)?;
            check_fits(&program, options)?;
            // Outputs are numbered in the order of the block's wires, so wire `n` is output `n`:
            // the script's `out(n + 1, ..)` drives the block of key `n`.
            let wires = (0..keys).map(|key| layout.target(key)).collect();
            blocks.push(Block {
                position: MICROCONTROLLER_POS,
                ..microcontroller("Music player", wires, program)
            });
        }
        Plan::Chain(ranges) => {
            for (index, &(start, end)) in ranges.iter().enumerate() {
                let part = timeline.part(start, end);
                let last = index + 1 == count;
                let has_link = !last || options.repeat;
                let restarts = index == 0 && options.repeat;
                let link = part.link(has_link, restarts);
                let program = build_program(&part.song, song.total_ms, options, Some(&link))?;
                check_fits(&program, options)?;

                // The keys, then the link: the script's `out(n + 1, ..)` is wire `n`. The link
                // goes to the next player, or after the last one back to the first.
                let mut wires: Vec<i32> = part.ids.iter().map(|&key| layout.target(key)).collect();
                if has_link {
                    wires.push(layout.player(if last { 0 } else { index + 1 }));
                }
                blocks.push(Block {
                    position: MICROCONTROLLER_POS,
                    ..microcontroller(
                        &format!("Music player {}/{count}", index + 1),
                        wires,
                        program,
                    )
                });
            }
        }
    }
    debug_assert_eq!(blocks.len() as i32, layout.player(count));

    if options.display {
        let program = display_program(song, whole_ms, options)?;
        blocks.push(Block {
            position: DISPLAY_MC_POS,
            ..microcontroller("Music display", Vec::new(), program)
        });
    }

    if layout.gates() {
        for key in 0..keys {
            // The screen is wired after the tone generator; an OR gate pushes its value to both.
            let mut wires = vec![layout.tone(key)];
            if options.display {
                wires.push(layout.screen());
            }
            blocks.push(Block {
                position: Vec3::new(0.0, 0.0, 0.0),
                wires,
                ..Block::new(OR_GATE, 0)
            });
        }
    }
    blocks.extend(song.keys.iter().map(|&key| tone_generator(key)));
    debug_assert_eq!(blocks.len(), layout.len());

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
    fn playback_matches_the_structure_and_snaps_to_ticks() {
        // 480 ticks = 500 ms. The second note starts 10 ms into a tick.
        let raw = midi(vec![vec![
            (0, note(0, 60, 100)),
            (480, off(0, 60)),
            (490, note(0, 69, 100)),
            (970, off(0, 69)),
        ]]);
        let pb = playback(&raw, &Options::default()).unwrap();
        assert_eq!(pb.frequencies.len(), 2);
        assert!((pb.frequencies[0] - 261.6256).abs() < 0.01);
        assert!((pb.frequencies[1] - 440.0).abs() < 0.01);
        let evs: Vec<_> = pb.events.iter().map(|e| (e.ms, e.tone, e.on)).collect();
        // 0 -> 20 (first tick), 500 stays, 510 -> 520, 1010 -> 1020
        assert_eq!(evs, vec![(20, 0, true), (500, 0, false), (520, 1, true), (1020, 1, false)]);
        assert!(pb.events.iter().all(|e| e.ms % TICK_MS == 0));
        assert_eq!(pb.total_ms % TICK_MS, 0);
        assert!(pb.total_ms >= pb.events.last().unwrap().ms);

        // the structure has one tone generator per frequency, with the same frequencies
        let s = generate(&raw, &Options::default()).unwrap();
        let freqs: Vec<f32> = s
            .blocks
            .iter()
            .filter(|b| b.id == TONE_GENERATOR)
            .map(|b| b.settings.as_ref().unwrap().values[0])
            .collect();
        assert_eq!(freqs, pb.frequencies);
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

    /// Indices of the blocks wired to block `to`, in file order: its inputs A, B, ...
    fn inputs_of(structure: &Structure, to: usize) -> Vec<usize> {
        (0..structure.blocks.len())
            .filter(|&i| structure.blocks[i].wires.contains(&(to as i32)))
            .collect()
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

        // switch, player, screen, an OR gate and a tone generator per key (lowest first)
        assert_eq!(reread.blocks.len(), 7);
        assert_eq!(reread.blocks[0].id, SWITCH);
        assert_eq!(reread.blocks[0].wires, vec![1, 2], "the player and the screen");
        assert_eq!(reread.blocks[1].id, MICROCONTROLLER);
        assert_eq!(reread.blocks[1].wires, vec![3, 4], "the player drives the OR gates");
        assert_eq!(reread.blocks[2].id, MICROCONTROLLER);
        assert_eq!(reread.blocks[2].wires, Vec::<i32>::new());
        for (key, freq) in [261.6256, 440.0].into_iter().enumerate() {
            let gate = &reread.blocks[3 + key];
            assert_eq!(gate.id, OR_GATE);
            assert_eq!(gate.wires, vec![5 + key as i32, 2], "tone generator and screen");
            let tone = &reread.blocks[5 + key];
            assert_eq!(tone.id, TONE_GENERATOR);
            let values = &tone.settings.as_ref().unwrap().values;
            assert!((values[0] - freq).abs() < 0.01, "{values:?}");
        }
        // The screen's input A is the switch and key `n` is input `n + 1`.
        assert_eq!(inputs_of(&reread, 2), vec![0, 3, 4]);
        assert_eq!(inputs_of(&reread, 1), vec![0], "the player only has the switch");

        let player = program(&reread.blocks[1]);
        let source = player.source();
        assert!(!source.contains('@'), "unfilled placeholder:\n{source}");
        assert!(source.contains("keys = 2\n"));
        assert!(source.contains("total_ms = 500\n"));
        assert!(source.contains("repeat = 1\n"));
        assert!(!source.contains("draw_keys"), "the player does not draw anything");
        assert_eq!(player.data_blobs.len(), 1, "the song must be stored as one raw blob");
        assert_eq!(player.data_blobs[0], encode_events(&parse_midi(&raw, &options).unwrap()));

        let screen = program(&reread.blocks[2]);
        let source = screen.source();
        assert!(!source.contains('@'), "unfilled placeholder:\n{source}");
        assert!(source.contains("keys = 2\n"));
        assert!(source.contains("total_ms = 500\n"));
        assert!(source.contains("repeat = 1\n"));
        assert!(source.contains("\"Test _song_\""));
        assert_eq!(screen.data_blobs[0], vec![60, 69], "the pitch behind each input");
        assert_eq!(reread.blocks[2].name.as_deref(), Some("Music display"));
    }

    #[test]
    fn display_can_be_left_out() {
        let raw = midi(vec![vec![(0, note(0, 60, 100)), (480, off(0, 60))]]);
        let with = generate(&raw, &Options::default()).unwrap();
        let without = generate(&raw, &Options { display: false, ..Options::default() }).unwrap();
        let count = |s: &Structure, id| s.blocks.iter().filter(|b| b.id == id).count();

        assert_eq!(count(&with, MICROCONTROLLER), 2);
        assert_eq!(count(&with, OR_GATE), 1);
        assert!(program(&with.blocks[2]).source().contains("draw_keys"));

        // No screen: the player drives the tone generators itself, as before.
        assert_eq!(count(&without, MICROCONTROLLER), 1);
        assert_eq!(count(&without, OR_GATE), 0);
        assert_eq!(without.blocks.len(), 3);
        assert_eq!(without.blocks[0].wires, vec![1]);
        assert_eq!(without.blocks[1].wires, vec![2]);
        assert_eq!(without.blocks[2].id, TONE_GENERATOR);
        assert!(program(&without.blocks[1]).source().contains("out(k + 1, held[k])"));
    }

    /// `notes` short notes cycling over `pitches` keys, with one note held through the whole song.
    fn long_song(notes: u32, pitches: u32) -> Vec<u8> {
        let mut events = vec![(0, note(0, 30, 100))];
        for i in 0..notes {
            let key = 40 + (i * 7) % pitches;
            events.push((i * 120, note(0, key as u8, 100)));
            events.push((i * 120 + 90, off(0, key as u8)));
        }
        events.push((notes * 120 + 240, off(0, 30)));
        midi(vec![events])
    }

    fn program(block: &Block) -> &MicrocontrollerData {
        match block.settings.as_ref().unwrap().custom.as_ref() {
            Some(CustomData::Microcontroller(program)) => program,
            _ => panic!("no program"),
        }
    }

    fn number_after(source: &str, name: &str) -> u64 {
        let line = source.lines().find(|l| l.starts_with(&format!("{name} = "))).unwrap();
        line[name.len() + 3..].split_whitespace().next().unwrap().parse().unwrap()
    }

    #[test]
    fn a_long_song_is_chained_over_several_microcontrollers() {
        for (repeat, display) in [(false, false), (true, false), (false, true), (true, true)] {
            let raw = long_song(6000, 50);
            let options = Options {
                max_source_len: 10_000,
                repeat,
                display,
                title: "Long".into(),
                ..Options::default()
            };
            let song = parse_midi(&raw, &options).unwrap();
            let timeline = Timeline::new(&song);
            let structure = generate(&raw, &options).unwrap();

            let d = display as usize;
            // the screen is a microcontroller too, but not one of the chain
            let count = structure.blocks.iter().filter(|b| b.id == MICROCONTROLLER).count() - d;
            let keys = song.keys.len();
            assert!(count > 2, "expected a long chain, got {count}");
            assert_eq!(structure.blocks.len(), 1 + count + d + 2 * keys);

            let micro = |p: usize| 1 + p as i32;
            let screen = (1 + count) as i32;
            let or_gate = |k: usize| (1 + count + d + k) as i32;
            let tone = |k: usize| (1 + count + d + keys + k) as i32;

            // Switch -> first microcontroller only; the switch is listed before every microcontroller
            // so that it is input A.
            assert_eq!(structure.blocks[0].id, SWITCH);
            let mut switch_wires = vec![micro(0)];
            if display {
                switch_wires.push(screen);
            }
            assert_eq!(structure.blocks[0].wires, switch_wires);
            for k in 0..keys {
                let gate = &structure.blocks[or_gate(k) as usize];
                assert_eq!(gate.id, OR_GATE);
                let mut wires = vec![tone(k)];
                if display {
                    wires.push(screen);
                }
                assert_eq!(gate.wires, wires);
                assert_eq!(structure.blocks[tone(k) as usize].id, TONE_GENERATOR);
            }

            if display {
                // The screen hears the switch, then every key from the lowest.
                let mut inputs = vec![0];
                inputs.extend((0..keys).map(|k| or_gate(k) as usize));
                assert_eq!(inputs_of(&structure, screen as usize), inputs);

                let mc = &structure.blocks[screen as usize];
                assert_eq!(mc.id, MICROCONTROLLER);
                assert!(mc.wires.is_empty());
                let source = program(mc).source();
                assert!(!source.contains('@'), "unfilled placeholder");
                assert!(source.contains("\"Long\""));
                assert!(source.contains(&format!("keys = {keys}\n")));
                assert!(source.contains(&format!("repeat = {}\n", repeat as u8)));
                // the song starts over when the last part ends
                assert_eq!(number_after(&source, "total_ms"), timeline.ticks * TICK_MS);
                let pitches: Vec<u8> = program(mc).data_blobs[0].clone();
                assert_eq!(pitches, song.keys);
            } else {
                assert_eq!(structure.blocks.iter().filter(|b| b.id == MICROCONTROLLER).count(), count);
            }

            // Replay every microcontroller the way the script does and OR the outputs together.
            let mut out = vec![vec![false; keys]; timeline.ticks as usize + 2];
            let mut next_start = 0;
            for p in 0..count {
                let mc = &structure.blocks[micro(p) as usize];
                assert_eq!(mc.id, MICROCONTROLLER);
                let program = program(mc);
                let source = program.source();
                assert!(source.len() <= 10_000, "part {p}: {}", source.len());
                assert!(!source.contains('@'), "unfilled placeholder");
                assert_eq!(mc.name.as_deref(), Some(format!("Music player {}/{count}", p + 1).as_str()));

                let last = p + 1 == count;
                assert_eq!(number_after(&source, "has_link"), (!last || repeat) as u64);
                assert_eq!(number_after(&source, "restarts"), (p == 0 && repeat) as u64);

                // Wires: the keys, then the link to the next microcontroller (the first again
                // after the last one, when repeating).
                let ids: Vec<usize> = program.data_blobs[1].iter().map(|&b| b as usize).collect();
                let mut wires: Vec<i32> = ids.iter().map(|&k| or_gate(k)).collect();
                if !last {
                    wires.push(micro(p + 1));
                } else if repeat {
                    wires.push(micro(0));
                }
                assert_eq!(mc.wires, wires, "part {p}");

                let first_tick = next_start / TICK_MS + 1;

                let table = decode(&program.data_blobs[0]);
                let mut now = 0;
                let mut events = Vec::new();
                for (delay, event) in &table {
                    now += delay;
                    if let Some((key, on)) = event {
                        events.push(((now + TICK_MS / 2) / TICK_MS, *key, *on));
                    }
                }
                let ticks = (now + TICK_MS / 2) / TICK_MS; // the part ends on this local tick
                next_start += ticks * TICK_MS;

                let mut held = vec![false; ids.len()];
                let mut i = 0;
                for local in 1..=ticks + 1 {
                    while i < events.len() && events[i].0 <= local {
                        held[events[i].1] = events[i].2;
                        i += 1;
                    }
                    if local == ticks + 1 {
                        held.fill(false); // lets go one tick after handing over
                    }
                    let t = (first_tick + local - 1) as usize;
                    if t < out.len() {
                        for (j, &id) in ids.iter().enumerate() {
                            out[t][id] |= held[j];
                        }
                    }
                }
            }
            assert_eq!(next_start, timeline.ticks * TICK_MS, "the parts must cover the song");

            // What the OR gates see on each tick is what the song asks for.
            let mut want = vec![false; keys];
            let mut i = 0;
            for t in 1..=timeline.ticks as usize {
                while i < timeline.events.len() && timeline.events[i].tick <= t as u64 {
                    want[timeline.events[i].key] = timeline.events[i].on;
                    i += 1;
                }
                assert_eq!(out[t], want, "tick {t}");
            }
        }
    }

    #[test]
    fn a_song_that_fits_stays_in_one_microcontroller() {
        let raw = long_song(50, 10);
        let structure = generate(&raw, &Options::default()).unwrap();
        // one player and the screen
        assert_eq!(structure.blocks.iter().filter(|b| b.id == MICROCONTROLLER).count(), 2);
        // the OR gates in front of the tone generators are what the screen listens to
        assert_eq!(structure.blocks.iter().filter(|b| b.id == OR_GATE).count(), 10 + 1);
        assert_eq!(structure.blocks[1].name.as_deref(), Some("Music player"));
    }

    #[test]
    fn an_impossible_limit_is_an_error() {
        let raw = long_song(6000, 1);
        let options = Options { max_source_len: 1000, ..Options::default() };
        assert!(generate(&raw, &options).is_err());
    }

    #[test]
    fn preprocessing_handles_nesting() {
        let template = "a\n# @IF x@\nb\n# @IF y@\nc\n# @ELSE@\nd\n# @END@\n# @ELSE@\ne\n# @END@\nf\n";
        assert_eq!(preprocess(template, &["x", "y"]), "a\nb\nc\nf\n");
        assert_eq!(preprocess(template, &["x"]), "a\nb\nd\nf\n");
        assert_eq!(preprocess(template, &["y"]), "a\ne\nf\n");
    }
}
