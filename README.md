# sw-tools

Set of tools that can read, generate and manipulate Sandbox World `.structure`, `.swmap` and `.save` files.

## `mesh`: glTF to structure

```
sw-tools mesh model.glb                       # metal plates, painted like the model
sw-tools mesh model.glb -b glass -t 0.1       # glass plates, 10 cm thick
sw-tools mesh model.glb -m wireframe -b neon  # neon edges
```

Both `.glb` and `.gltf` (with external `.bin`/images) work, and node transforms are applied.

| option | default | |
|---|---|---|
| `-b, --block` | `metal` | block to build with: `metal`, `wood`, `glass`, `neon`, or any custom-shape block by name or ID |
| `-m, --mode` | `hollow` | `hollow` fills faces with plates, `wireframe` draws edges as bars |
| `-t, --thickness` | `0.05` | thickness of plates and bars |
| `-c, --color` | `auto` | `auto` paints from the glTF, `none` paints nothing |
| `--merge-angle` | `0.5` | see below; `0` turns merging off |
| `--scale` | `1` | multiplies the size of the whole model |
| `--mirror` | `x` | handedness conversion, see below |

### Coordinates

glTF is right-handed and the game is left-handed, so one axis has to be mirrored.
`--mirror x` (the default) negates X, which keeps the model's front (+Z in glTF)
facing +Z; `--mirror z` negates Z instead, giving the same shape turned 180° around
the vertical axis; `--mirror none` leaves glTF coordinates alone. Face winding is
flipped along with it, so plates always extend *behind* the face, never out of it.

### In-game size limits

The game limits custom block planes to positions of ±6 × ±5 × ±6 and sizes of up to
10 × 10, which is stricter than the file format. Blocks are centered on their face to
make the most of that range (a plate can be 10 × 10), and anything bigger is cut into
as many blocks as needed, so large models work without manual splitting. Wireframe
bars longer than 10 are chained. `thickness` can be at most 10. A model that would
need more than 65535 blocks stops with an error: lower the detail or `--scale`.

### One block per quad

glTF only stores triangles, so a quad arrives as two. A custom block is a top and a
bottom rectangle, which can be made thin: that is a flat trapezoid. Whenever two
coplanar triangles form a convex quad with a pair of parallel edges (rectangles,
parallelograms, trapezoids, which is most of what a modeler makes), they become a
single block instead of two. Triangles and quads without parallel edges still get
one block per triangle. `--merge-angle` is the largest angle error (in degrees) that
is tolerated when deciding that two triangles are coplanar and two edges parallel.
Triangles with clearly different colors are never merged.

### Settings stored in the glTF

Everything the command line sets as a default can be overridden inside the model,
through `extras` (in Blender: *Object / Mesh / Material Properties → Custom
Properties*, then tick **Custom Properties** under *Data* in the glTF export
options). Use either flat keys or one nested `sw` object:

```json
{ "sw_block": "glass", "sw_color": "#ff8800" }
{ "sw": { "block": "neon", "thickness": 0.1, "mode": "wireframe" } }
```

| key | value |
|---|---|
| `block` | name (`metal`, `wood`, `glass`, `neon`, …) or block ID; must be a custom-shape block |
| `thickness` | number above 0 |
| `mode` | `hollow` or `wireframe` |
| `color` | `"#rrggbb"` / `"#rrggbbaa"` (sRGB), or `[r, g, b]` / `[r, g, b, a]` as 0–1 linear values like `baseColorFactor` |

They can be put on a node, mesh, material or primitive. For each key the most
specific place wins: **primitive > material > mesh > node**, and a node's values are
inherited by its children. Putting `sw_block` on a *material* is the usual way to
build one model out of several block types. Mistakes (an unknown block name, a bad
color) stop the run with a message saying which object they are on.

### Colors

With `--color auto` a face is painted the way a glTF viewer would show it:
`baseColorFactor × COLOR_0 vertex color × baseColorTexture`, converted to sRGB.
Vertex colors are averaged over the triangle and textures are sampled at its center,
so very detailed textures on large faces come out as one flat color per block.
Primitives without a material and without vertex colors are left unpainted.
A `color` in the extras beats all of that.

## License

[MIT](LICENSE)
