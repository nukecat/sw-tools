# midi2swstruct

`midi2swstruct` is a Rust CLI tool that **converts MIDI files into Sandbox World structure files**. The generated structure includes a music player with encoded MIDI data, playable within Sandbox World.

---

## Features

* Converts standard MIDI files into `.structure` files.
* Supports configurable pitch ranges, velocities, and event limits.
* Encodes multiple notes into single values to optimize structure size.
* Optional looping of music.
* Fully customizable structure version for Sandbox World compatibility.

---

## Usage

```bash
midi2swstruct song.mid [options]
```

### Options

| Flag                      | Description                           | Default               |
| ------------------------- | ------------------------------------- | --------------------- |
| `-o, --output`            | Optional output path                  | `./<input>.structure` |
| `--stdout`                | Output to stdout                      | false                 |
| `--min-pitch`             | Minimal note pitch                    | 27                    |
| `--max-pitch`             | Maximal note pitch                    | 111                   |
| `--max-events-per-func`   | Max events per function               | 1024                  |
| `--min-velocity`          | Minimal note velocity to trigger note | 1                     |
| `-r, --repeat`            | Repeat music (loop)                   | false                 |
| `-n, --notes-per-value`   | Number of notes encoded per value     | 24                    |

---

## Example

```bash
midi2swstruct my_song.mid -o output.structure --repeat --min-pitch 30 --max-pitch 100
```

This converts `my_song.mid` into `output.structure` with looping enabled and only notes between pitch 30 and 100.

---

## How it Works

1. Parses MIDI file and collects note events across all tracks.
2. Filters notes by pitch and velocity.
3. Encodes note changes into a compressed function-based format.
4. Generates Sandbox World building with math blocks which contain data.
5. Outputs a `.structure` file ready for import.

---

## License

[MIT](LICENSE)
