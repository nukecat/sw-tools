# sw-tools

Command line tools for reading, generating and manipulating Sandbox World files
(`.structure`, `.swmap` and `.save`).

| Command | Input | Output | Purpose |
|---|---|---|---|
| [`pixel-art`](#pixel-art) | image | `.structure` | Builds a picture out of colored blocks |
| [`music-player`](#music-player) | MIDI file | `.structure` | Builds a working music player |
| [`mesh`](#mesh) | glTF / GLB | `.structure` | Builds a 3D model out of plates or bars |
| [`catalog`](#catalog) | none | text | Lists blocks, props or layers and their IDs |
| [`insert-data`](#insert-data) | `.structure`, binary | `.structure` | Insert binary data into microcontrollers |

## Installation

Download a prebuilt binary for Windows, Linux or macOS from the
[Releases](https://github.com/nukecat/sw-tools/releases) page, or build it from source
with a recent Rust toolchain:

```
cargo install --git https://github.com/nukecat/sw-tools
```

## Usage

```
sw-tools <COMMAND> [OPTIONS] <INPUT>
sw-tools <COMMAND> --help
```

Conventions shared by all commands that produce a structure:

- The output is written next to where you run the command, named after the input file with a
  `.structure` extension. Use `-o, --output <PATH>` to choose another path.
- Options that take a block accept a **name** (`metal`, `glass`, ...) or a numeric **ID**.
  Run [`sw-tools catalog blocks`](#catalog) to see what is available. A name only needs to be
  unambiguous; if it matches several blocks, the command stops and asks you to be more specific.

______________________________________________________________________

## pixel-art

Turns an image into a `.structure` with one colored block per pixel.

```
sw-tools pixel-art picture.png
sw-tools pixel-art picture.png -b glass -s 0.5 -o picture.structure
```

Common image formats are supported (PNG, JPEG, BMP). The picture is built in the XY plane,
centered horizontally, with the top row of the image at the top.

| Option | Default | Description |
|---|---|---|
| `-o, --output` | `<input>.structure` | Output path. |
| `-b, --block` | `0` | Block to build with, by name or ID. Tools cannot be used. |
| `-s, --spacing` | `1` | Distance between neighboring blocks. |
| `-v, --value` | `0` | State value applied to every block. |

______________________________________________________________________

## music-player

Turns a MIDI file into a `.structure` that plays the song.

```
sw-tools music-player song.mid
sw-tools music-player song.mid -o song.structure --repeat --title "My song"
```

The structure contains:

- a **microcontroller** running a generated PixelScript program,
- a second **microcontroller** that is the screen (left out with `--no-display`),
- a **switch**,
- one **OR gate** per pitch (only with a screen, or when a long song is split over several
  microcontrollers), and
- one **tone generator** per pitch used by the song.

How it works:

- **Controls:** switch on plays the song; switch off stops it and rewinds.
- **Output wiring:** the microcontroller's outputs drive the tone generators (through the OR
  gates, when there are any). Output *n* is the *n*-th lowest pitch used.
- **Storage:** the whole song is a single `data` table in the script, stored as raw bytes, so it
  takes no script text.
- **Tempo:** tempo changes are baked into the table, so playback needs no tempo logic.
- **Display:** a separate microcontroller whose screen shows the title, the keys that are down
  (as a strip of keys and as note names), elapsed and total time, and a progress bar. It adapts
  to any display size. It learns which keys are down from the OR gates, which feed both a tone
  generator and the screen, so it shows what you hear. The switch is wired to it as well, and
  it counts the time itself while the switch is on. Its inputs are the switch (A), then one per
  pitch, lowest first.

| Option | Default | Description |
|---|---|---|
| `-o, --output` | `<input>.structure` | Output path. |
| `--min-pitch` | `27` | Lowest MIDI pitch to include (0–127). |
| `--max-pitch` | `111` | Highest MIDI pitch to include (0–127). |
| `--min-velocity` | `1` | Notes quieter than this are ignored (0–127). |
| `-r, --repeat` | off | Loop the song. |
| `--include-drums` | off | Also play the percussion channel (MIDI channel 10). It is skipped by default because drum notes are not pitches. |
| `--no-display` | off | Leave out the screen microcontroller and the OR gates it needs. |
| `--title` | input file name | Title shown on the screen (up to 40 characters). |

**Limit:** a microcontroller has at most 127 usable outputs here, so a song may use at most
127 distinct pitches. If it uses more, narrow the range with `--min-pitch` and `--max-pitch`.

______________________________________________________________________

## mesh

Turns a glTF model (`.glb`, or `.gltf` with external `.bin` and image files) into a
`.structure` made of thin plates or bars. Node transforms are applied.

```
sw-tools mesh model.glb                        # metal plates, painted like the model
sw-tools mesh model.glb -b glass -t 0.1        # glass plates, 10 cm thick
sw-tools mesh model.glb -m wireframe -b neon   # neon edges
```

| Option | Default | Description |
|---|---|---|
| `-o, --output` | `<input>.structure` | Output path. |
| `-b, --block` | `metal` | Block to build with: `metal`, `wood`, `glass`, `neon`, or any custom-shape block by name or ID. |
| `-m, --mode` | `hollow` | `hollow` fills faces with plates; `wireframe` draws edges as bars. |
| `-t, --thickness` | `0.05` | Thickness of plates and bars. Must be above 0 and at most 10. |
| `-c, --color` | `auto` | `auto` paints from the glTF; `none` paints nothing. |
| `--merge-angle` | `0.5` | Largest angle error, in degrees, tolerated when merging triangles into quads. `0` turns merging off. |
| `--scale` | `1` | Multiplies the size of the whole model. |
| `--mirror` | `x` | Handedness conversion: `x`, `z` or `none`. |

Each of `block`, `mode`, `thickness` and `color` can also be set inside the model itself; see
[Settings stored in the glTF](#settings-stored-in-the-gltf).

### Coordinates

glTF is right-handed and the game is left-handed, so one axis has to be mirrored.

| `--mirror` | Effect |
|---|---|
| `x` | Negates X. The model's front (+Z in glTF) keeps facing +Z. |
| `z` | Negates Z. Gives the same shape turned 180° around the vertical axis. |
| `none` | Leaves glTF coordinates untouched. |

Face winding is flipped along with the mirror, so plates always extend *behind* a face, never
out of it.

### In-game size limits

The game limits custom block planes to positions of ±6 × ±5 × ±6 and sizes of up to 10 × 10,
which is stricter than the file format. To cope with this:

- Blocks are centered on their face to make the most of the range, so a plate can be 10 × 10.
- Anything bigger is cut into as many blocks as needed, so large models work without manual
  splitting.
- Wireframe bars longer than 10 are chained.

A model that would need more than 65535 blocks stops with an error. Lower the detail or use
`--scale`.

### One block per quad

glTF stores only triangles, so a quad arrives as two. A custom block is a top and a bottom
rectangle, which can be made thin, so it can represent a flat trapezoid.

Whenever two coplanar triangles form a convex quad with a pair of parallel edges (rectangles,
parallelograms, trapezoids, which is most of what a modeler makes), they become a single block
instead of two. Triangles and quads without parallel edges still get one block per triangle.
Triangles with clearly different colors are never merged.

`--merge-angle` controls how strict the check is: it is the largest angle error that still
counts as "coplanar" for the triangles and "parallel" for the edges.

### Colors

With `--color auto`, a face is painted the way a glTF viewer would show it:

```
baseColorFactor × COLOR_0 vertex color × baseColorTexture
```

converted to sRGB. Vertex colors are averaged over the triangle and textures are sampled at its
center, so a very detailed texture on a large face comes out as one flat color per block.
Primitives without a material and without vertex colors are left unpainted. A `color` set in
the glTF extras beats all of the above.

### Settings stored in the glTF

Every default that the command line sets can be overridden inside the model through `extras`.
In Blender, add them under *Object / Mesh / Material Properties → Custom Properties*, then
tick **Custom Properties** under *Data* in the glTF export options.

Use either flat keys or a single nested `sw` object:

```json
{ "sw_block": "glass", "sw_color": "#ff8800" }
{ "sw": { "block": "neon", "thickness": 0.1, "mode": "wireframe" } }
```

| Key | Value |
|---|---|
| `block` | Name (`metal`, `wood`, `glass`, `neon`, ...) or block ID. Must be a custom-shape block. |
| `thickness` | Number above 0. |
| `mode` | `hollow` or `wireframe`. |
| `color` | `"#rrggbb"` or `"#rrggbbaa"` (sRGB), or `[r, g, b]` / `[r, g, b, a]` as 0–1 linear values, like `baseColorFactor`. |

Settings can be placed on a node, mesh, material or primitive. For each key the most specific
place wins:

```
primitive > material > mesh > node
```

A node's values are inherited by its children. Putting `sw_block` on a *material* is the usual
way to build one model out of several block types.

Mistakes, such as an unknown block name or a bad color, stop the run with a message that names
the object they are on.

______________________________________________________________________

## catalog

Prints a table of everything the game knows about, so you can look up IDs for the `--block`
options above.

______________________________________________________________________

```
sw-tools catalog blocks
sw-tools catalog props
sw-tools catalog layers
```

| Category | Lists |
|---|---|
| `blocks` | Blocks |
| `props` | Map props, grouped by category (trees, rocks, buildings, roads, bridges, ...) |
| `layers` | Map layers, followed by the layers that a new map starts with |

______________________________________________________________________

## insert-data

Inserts data into microcontroller in the `.structure`

```
sw-tools insert-data display.structure nukcat.png -i
sw-tools insert-data number_recognition.structure layer_5_weights.bin --name "Layer 5" --magic "deadbeef" -i
```

| Option | Default | Description |
|---|---|---|
| `-o, --output` | input file path | Optional output path |
| `-n, --name` | none | Optional name to help identify a specific microcontroller block |
| `-m, --magic` | none | Optional base16 string to help identify a specific data array |
| `-i, --in-place` | false | Overwrite the input structure |

If there is multiple microcontrollers in one `.structure`, you need to give the name to one of them
and then specify that name when using this command.

If output file is not specified, it will re-write the input file.

Microcontroller data array can be empty, but it must exist. You cannot put data into microcontroller
if it doesn't declare data array in the script.

If there is multiple data arrays, you can use `--magic` option to specify what bytes of the target array are.

______________________________________________________________________

## License

[MIT](LICENSE)
