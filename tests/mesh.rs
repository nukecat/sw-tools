use glam::{EulerRot, Mat4, Quat, Vec3};
use serde_json::{Value, json};
use sw_formats::structure::Block;
use sw_tools::mesh::{self, ColorMode, Mirror, Mode, Options};

// ---------------------------------------------------------------- GLB builder

/// One object of a test scene: a node holding a mesh with a single primitive.
struct Object {
    positions: Vec<[f32; 3]>,
    indices: Vec<u16>,
    node: Value,
    material: Option<usize>,
    primitive_extras: Option<Value>,
    mesh_extras: Option<Value>,
}

impl Object {
    fn new(positions: &[[f32; 3]], indices: &[u16]) -> Self {
        Object {
            positions: positions.to_vec(),
            indices: indices.to_vec(),
            node: json!({}),
            material: None,
            primitive_extras: None,
            mesh_extras: None,
        }
    }
    fn node(mut self, node: Value) -> Self {
        self.node = node;
        self
    }
    fn material(mut self, m: usize) -> Self {
        self.material = Some(m);
        self
    }
    fn primitive_extras(mut self, e: Value) -> Self {
        self.primitive_extras = Some(e);
        self
    }
    fn mesh_extras(mut self, e: Value) -> Self {
        self.mesh_extras = Some(e);
        self
    }
}

fn glb(objects: &[Object], materials: Vec<Value>) -> Vec<u8> {
    let mut bin = Vec::new();
    let (mut views, mut accessors, mut meshes, mut nodes) = (vec![], vec![], vec![], vec![]);

    for (i, o) in objects.iter().enumerate() {
        // positions
        let offset = bin.len();
        for p in &o.positions {
            for c in p {
                bin.extend_from_slice(&c.to_le_bytes());
            }
        }
        views
            .push(json!({"buffer": 0, "byteOffset": offset, "byteLength": o.positions.len() * 12}));
        let min: Vec<f32> = (0..3)
            .map(|a| o.positions.iter().map(|p| p[a]).fold(f32::MAX, f32::min))
            .collect();
        let max: Vec<f32> = (0..3)
            .map(|a| o.positions.iter().map(|p| p[a]).fold(f32::MIN, f32::max))
            .collect();
        accessors.push(json!({"bufferView": views.len() - 1, "componentType": 5126,
            "count": o.positions.len(), "type": "VEC3", "min": min, "max": max}));
        let pos_acc = accessors.len() - 1;

        // indices
        let offset = bin.len();
        for idx in &o.indices {
            bin.extend_from_slice(&idx.to_le_bytes());
        }
        while bin.len() % 4 != 0 {
            bin.push(0);
        }
        views.push(json!({"buffer": 0, "byteOffset": offset, "byteLength": o.indices.len() * 2}));
        accessors.push(json!({"bufferView": views.len() - 1, "componentType": 5123,
            "count": o.indices.len(), "type": "SCALAR"}));
        let idx_acc = accessors.len() - 1;

        let mut prim = json!({"attributes": {"POSITION": pos_acc}, "indices": idx_acc, "mode": 4});
        if let Some(m) = o.material {
            prim["material"] = json!(m);
        }
        if let Some(e) = &o.primitive_extras {
            prim["extras"] = e.clone();
        }
        let mut mesh = json!({"primitives": [prim]});
        if let Some(e) = &o.mesh_extras {
            mesh["extras"] = e.clone();
        }
        meshes.push(mesh);

        let mut node = o.node.clone();
        node["mesh"] = json!(i);
        nodes.push(node);
    }

    let doc = json!({
        "asset": {"version": "2.0"},
        "scene": 0,
        "scenes": [{"nodes": (0..objects.len()).collect::<Vec<_>>()}],
        "nodes": nodes, "meshes": meshes, "materials": materials,
        "accessors": accessors, "bufferViews": views,
        "buffers": [{"byteLength": bin.len()}],
    });

    let mut json_bytes = serde_json::to_vec(&doc).unwrap();
    while json_bytes.len() % 4 != 0 {
        json_bytes.push(b' ');
    }
    while bin.len() % 4 != 0 {
        bin.push(0);
    }
    let total = 12 + 8 + json_bytes.len() + 8 + bin.len();
    let mut out = Vec::new();
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&json_bytes);
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(&bin);
    out
}

// ------------------------------------------------------- geometry reconstruction

/// World-space corners of the block's front face (local z = 0, the face the
/// triangle/quad lies on), rebuilt from nothing but what is written to the file.
fn front_corners(b: &Block) -> Vec<Vec3> {
    let v = &b.settings.as_ref().unwrap().vectors;
    assert_eq!(v.len(), 4);
    let rot = Quat::from_euler(
        EulerRot::YXZ,
        b.rotation.y.to_radians(),
        b.rotation.x.to_radians(),
        b.rotation.z.to_radians(),
    );
    let pos = Vec3::new(b.position.x, b.position.y, b.position.z);
    let mut out = Vec::new();
    for s in [-1.0f32, 1.0] {
        for (c, size) in [(&v[0], &v[1]), (&v[2], &v[3])] {
            let local = Vec3::new(c.x + s * size.x / 2.0, c.y, c.z + size.z / 2.0);
            out.push(pos + rot * local);
        }
    }
    out
}

fn same(a: Vec3, b: Vec3) -> bool {
    (a - b).length() < 1e-4
}

/// The union of all blocks' front corners must be exactly the expected points.
fn assert_corners(blocks: &[Block], expected: &[Vec3]) {
    let got: Vec<Vec3> = blocks.iter().flat_map(front_corners).collect();
    for e in expected {
        assert!(
            got.iter().any(|g| same(*g, *e)),
            "no block corner at {e}; have {got:?}"
        );
    }
    for g in &got {
        assert!(
            expected.iter().any(|e| same(*g, *e)),
            "stray block corner at {g}; expected {expected:?}"
        );
    }
}

/// Existing tests check raw glTF coordinates, so they run without the handedness fix.
fn opts() -> Options {
    Options {
        mirror: Mirror::None,
        ..Default::default()
    }
}

fn run(data: &[u8], opts: &Options) -> (Vec<Block>, mesh::Stats) {
    let (s, stats) = mesh::generate(data, None, opts).expect("generate");
    (s.blocks, stats)
}

fn v(p: [f32; 3]) -> Vec3 {
    p.into()
}

const RECT: [[f32; 3]; 4] = [
    [0.0, 0.0, 0.0],
    [2.0, 0.0, 0.0],
    [2.0, 1.0, 0.0],
    [0.0, 1.0, 0.0],
];
const QUAD_IDX: [u16; 6] = [0, 1, 2, 0, 2, 3];

// ------------------------------------------------------------------------ quads

#[test]
fn rectangle_is_one_block_with_exact_corners() {
    let data = glb(&[Object::new(&RECT, &QUAD_IDX)], vec![]);
    let (blocks, stats) = run(&data, &opts());
    assert_eq!(blocks.len(), 1);
    assert_eq!(stats.merged_quads, 1);
    assert_corners(&blocks, &RECT.map(v));
}

#[test]
fn slanted_trapezoid_in_3d_is_one_block_with_exact_corners() {
    // parallel edges along (1,1,0)/sqrt2, plane tilted by a rotation
    let rot = Quat::from_euler(EulerRot::XYZ, 0.4, -0.9, 0.3);
    let base = [
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(3.0, 3.0, 0.0), // bottom edge
        Vec3::new(2.0, 3.5, 0.0),
        Vec3::new(0.5, 2.0, 0.0), // top edge, parallel and shorter
    ];
    // make the top edge truly parallel to the bottom one
    let dir = (base[1] - base[0]).normalize();
    let top0 = base[0] + Vec3::new(-0.7, 0.7, 0.0) * 1.3;
    let pts: Vec<Vec3> = [base[0], base[1], top0 + dir * 2.0, top0]
        .iter()
        .map(|p| rot * *p + Vec3::new(5.0, -2.0, 1.0))
        .collect();
    let positions: Vec<[f32; 3]> = pts.iter().map(|p| p.to_array()).collect();
    let data = glb(&[Object::new(&positions, &QUAD_IDX)], vec![]);
    let (blocks, stats) = run(&data, &opts());
    assert_eq!(blocks.len(), 1, "should be a single block");
    assert_eq!(stats.merged_quads, 1);
    assert_corners(&blocks, &pts);
}

#[test]
fn triangle_is_one_block_with_exact_corners() {
    let tri = [[0.0, 0.0, 0.0], [4.0, 0.0, 0.0], [1.0, 3.0, 0.0]];
    let data = glb(&[Object::new(&tri, &[0, 1, 2])], vec![]);
    let (blocks, stats) = run(&data, &opts());
    assert_eq!(blocks.len(), 1);
    assert_eq!(stats.merged_quads, 0);
    assert_corners(&blocks, &tri.map(v));
}

#[test]
fn general_quad_falls_back_to_two_exact_triangles() {
    let kite = [
        [0.0, 0.0, 0.0],
        [2.0, 0.0, 0.0],
        [3.0, 2.0, 0.5],
        [-1.0, 3.0, 0.2],
    ];
    let data = glb(&[Object::new(&kite, &QUAD_IDX)], vec![]);
    let (blocks, _) = run(&data, &opts());
    assert_eq!(blocks.len(), 2);
    assert_corners(&blocks, &kite.map(v));
}

#[test]
fn merging_can_be_disabled() {
    let data = glb(&[Object::new(&RECT, &QUAD_IDX)], vec![]);
    let (blocks, _) = run(
        &data,
        &Options {
            merge_angle: 0.0,
            ..opts()
        },
    );
    assert_eq!(blocks.len(), 2);
    assert_corners(&blocks, &RECT.map(v));
}

#[test]
fn cube_from_quads_is_six_blocks() {
    let c = [
        [0., 0., 0.],
        [1., 0., 0.],
        [1., 1., 0.],
        [0., 1., 0.],
        [0., 0., 1.],
        [1., 0., 1.],
        [1., 1., 1.],
        [0., 1., 1.],
    ];
    // 6 faces, outward winding, 2 triangles each
    #[rustfmt::skip]
    let idx: [u16; 36] = [
        0, 2, 1, 0, 3, 2,   // z = 0
        4, 5, 6, 4, 6, 7,   // z = 1
        0, 1, 5, 0, 5, 4,   // y = 0
        3, 6, 2, 3, 7, 6,   // y = 1
        0, 4, 7, 0, 7, 3,   // x = 0
        1, 2, 6, 1, 6, 5,   // x = 1
    ];
    let data = glb(&[Object::new(&c, &idx)], vec![]);
    let (blocks, stats) = run(&data, &opts());
    assert_eq!(stats.triangles, 12);
    assert_eq!(blocks.len(), 6);
    assert_corners(&blocks, &c.map(v));
}

#[test]
fn grid_of_quads_halves_the_block_count() {
    let n = 6usize;
    let mut positions = Vec::new();
    for y in 0..=n {
        for x in 0..=n {
            positions.push([x as f32, y as f32, 0.0]);
        }
    }
    let mut idx = Vec::new();
    for y in 0..n {
        for x in 0..n {
            let a = (y * (n + 1) + x) as u16;
            let (b, c, d) = (a + 1, a + n as u16 + 2, a + n as u16 + 1);
            idx.extend_from_slice(&[a, b, c, a, c, d]);
        }
    }
    let data = glb(&[Object::new(&positions, &idx)], vec![]);
    let (blocks, stats) = run(&data, &opts());
    assert_eq!(stats.triangles, 2 * n * n);
    assert_eq!(blocks.len(), n * n);
}

// ------------------------------------------------------------------ transforms

#[test]
fn node_transforms_are_applied() {
    let node = json!({"translation": [10.0, 0.0, -3.0], "rotation": [0.0, 0.3826834, 0.0, 0.9238795], "scale": [2.0, 3.0, 1.0]});
    let data = glb(&[Object::new(&RECT, &QUAD_IDX).node(node.clone())], vec![]);
    let m = Mat4::from_scale_rotation_translation(
        Vec3::new(2.0, 3.0, 1.0),
        Quat::from_xyzw(0.0, 0.3826834, 0.0, 0.9238795),
        Vec3::new(10.0, 0.0, -3.0),
    );
    let expected: Vec<Vec3> = RECT.iter().map(|p| m.transform_point3(v(*p))).collect();
    let (blocks, _) = run(&data, &opts());
    assert_eq!(blocks.len(), 1);
    assert_corners(&blocks, &expected);
}

#[test]
fn mirrored_nodes_still_produce_correct_blocks() {
    let data = glb(
        &[Object::new(&RECT, &QUAD_IDX).node(json!({"scale": [-1.0, 1.0, 1.0]}))],
        vec![],
    );
    let expected: Vec<Vec3> = RECT.iter().map(|p| Vec3::new(-p[0], p[1], p[2])).collect();
    let (blocks, _) = run(&data, &opts());
    assert_eq!(blocks.len(), 1);
    assert_corners(&blocks, &expected);
}

#[test]
fn scale_option_scales_the_model() {
    let data = glb(&[Object::new(&RECT, &QUAD_IDX)], vec![]);
    let (blocks, _) = run(
        &data,
        &Options {
            scale: 2.5,
            ..opts()
        },
    );
    let expected: Vec<Vec3> = RECT.iter().map(|p| v(*p) * 2.5).collect();
    assert_corners(&blocks, &expected);
}

// ------------------------------------------------------------------ block types

#[test]
fn default_block_comes_from_options() {
    let data = glb(&[Object::new(&RECT, &QUAD_IDX)], vec![]);
    assert_eq!(run(&data, &opts()).0[0].id, 109);
    let glass = Options {
        block: mesh::parse_block("glass").unwrap(),
        ..opts()
    };
    assert_eq!(run(&data, &glass).0[0].id, 121);
}

#[test]
fn extras_pick_the_block_and_the_most_specific_one_wins() {
    let materials = vec![
        json!({"name": "Plain"}),
        json!({"name": "Glassy", "extras": {"sw_block": "glass"}}),
        json!({"name": "Nested", "extras": {"sw": {"block": "neon"}}}),
    ];
    let objects = [
        // node < mesh < material < primitive
        Object::new(&RECT, &QUAD_IDX).node(json!({"extras": {"sw_block": "wood"}})),
        Object::new(&RECT, &QUAD_IDX)
            .node(json!({"extras": {"sw_block": "wood"}}))
            .mesh_extras(json!({"sw_block": "glass"})),
        Object::new(&RECT, &QUAD_IDX).material(1),
        Object::new(&RECT, &QUAD_IDX).material(2),
        Object::new(&RECT, &QUAD_IDX)
            .material(1)
            .primitive_extras(json!({"sw_block": 130})),
        Object::new(&RECT, &QUAD_IDX).material(0),
    ];
    let (blocks, _) = run(&glb(&objects, materials), &opts());
    let ids: Vec<i32> = blocks.iter().map(|b| b.id).collect();
    assert_eq!(ids, vec![120, 121, 121, 130, 130, 109]);
}

#[test]
fn node_extras_are_inherited_by_children() {
    // hand-built document: parent (with extras) -> child (with mesh)
    let objects = [Object::new(&RECT, &QUAD_IDX)];
    let mut doc: Value = {
        let bytes = glb(&objects, vec![]);
        let len = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
        serde_json::from_slice(&bytes[20..20 + len]).unwrap()
    };
    doc["nodes"] = json!([
        {"children": [1], "extras": {"sw_block": "neon"}, "translation": [0.0, 5.0, 0.0]},
        {"mesh": 0}
    ]);
    doc["scenes"] = json!([{"nodes": [0]}]);
    // re-wrap with the original binary chunk
    let bytes = glb(&objects, vec![]);
    let jlen = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    let bin = bytes[20 + jlen + 8..].to_vec();
    let mut j = serde_json::to_vec(&doc).unwrap();
    while j.len() % 4 != 0 {
        j.push(b' ');
    }
    let mut out = b"glTF".to_vec();
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&((12 + 8 + j.len() + 8 + bin.len()) as u32).to_le_bytes());
    out.extend_from_slice(&(j.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&j);
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(&bin);

    let (blocks, _) = run(&out, &opts());
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].id, 130);
    let expected: Vec<Vec3> = RECT
        .iter()
        .map(|p| v(*p) + Vec3::new(0.0, 5.0, 0.0))
        .collect();
    assert_corners(&blocks, &expected);
}

#[test]
fn wireframe_uses_the_chosen_block() {
    let data = glb(
        &[Object::new(&RECT, &QUAD_IDX)
            .mesh_extras(json!({"sw_block": "glass", "sw_mode": "wireframe"}))],
        vec![],
    );
    let (blocks, _) = run(&data, &opts());
    assert_eq!(blocks.len(), 5); // 4 sides + 1 diagonal
    assert!(blocks.iter().all(|b| b.id == 121));
}

#[test]
fn bad_block_name_reports_where() {
    let data = glb(
        &[Object::new(&RECT, &QUAD_IDX).mesh_extras(json!({"sw_block": "plastic"}))],
        vec![],
    );
    let err = mesh::generate(&data, None, &opts())
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("mesh #0") && err.contains("plastic") && err.contains("valid blocks"),
        "{err}"
    );
}

#[test]
fn non_custom_block_is_rejected() {
    let data = glb(
        &[Object::new(&RECT, &QUAD_IDX).mesh_extras(json!({"sw_block": 0}))],
        vec![],
    );
    let err = mesh::generate(&data, None, &opts())
        .unwrap_err()
        .to_string();
    assert!(err.contains("not a custom-shape block"), "{err}");
}

#[test]
fn thickness_and_mode_can_come_from_extras() {
    let data = glb(
        &[Object::new(&RECT, &QUAD_IDX).mesh_extras(json!({"sw_thickness": 0.2}))],
        vec![],
    );
    let (blocks, _) = run(&data, &opts());
    let v = &blocks[0].settings.as_ref().unwrap().vectors;
    assert!((v[1].z - 0.2).abs() < 1e-6 && (v[3].z - 0.2).abs() < 1e-6);
}

// ------------------------------------------------------------------------ color

#[test]
fn material_color_paints_blocks_in_srgb() {
    let materials =
        vec![json!({"pbrMetallicRoughness": {"baseColorFactor": [0.8, 0.8, 0.8, 1.0]}})];
    let data = glb(&[Object::new(&RECT, &QUAD_IDX).material(0)], materials);
    let (blocks, _) = run(&data, &opts());
    let c = blocks[0].color.unwrap();
    assert_eq!((c.r, c.g, c.b, c.a), (231, 231, 231, 255));
}

#[test]
fn no_material_means_unpainted() {
    let data = glb(&[Object::new(&RECT, &QUAD_IDX)], vec![]);
    assert!(run(&data, &opts()).0[0].color.is_none());
}

#[test]
fn extras_color_beats_material_color() {
    let materials = vec![
        json!({"pbrMetallicRoughness": {"baseColorFactor": [0.0, 0.0, 1.0, 1.0]}, "extras": {"sw_color": "#ff8000"}}),
    ];
    let data = glb(&[Object::new(&RECT, &QUAD_IDX).material(0)], materials);
    let c = run(&data, &opts()).0[0].color.unwrap();
    assert_eq!((c.r, c.g, c.b), (255, 128, 0));
}

#[test]
fn color_none_paints_nothing() {
    let materials = vec![
        json!({"pbrMetallicRoughness": {"baseColorFactor": [1.0, 0.0, 0.0, 1.0]}, "extras": {"sw_color": "#ff8000"}}),
    ];
    let data = glb(&[Object::new(&RECT, &QUAD_IDX).material(0)], materials);
    let (blocks, _) = run(
        &data,
        &Options {
            color: ColorMode::None,
            ..opts()
        },
    );
    assert!(blocks[0].color.is_none());
}

#[test]
fn differently_colored_halves_are_not_merged() {
    // two primitives of one quad would need vertex colors; use two objects instead
    let materials = vec![
        json!({"pbrMetallicRoughness": {"baseColorFactor": [1.0, 0.0, 0.0, 1.0]}}),
        json!({"pbrMetallicRoughness": {"baseColorFactor": [0.0, 0.0, 1.0, 1.0]}}),
    ];
    let objects = [
        Object::new(&RECT, &[0, 1, 2]).material(0),
        Object::new(&RECT, &[0, 2, 3]).material(1),
    ];
    let (blocks, _) = run(&glb(&objects, materials), &opts());
    assert_eq!(blocks.len(), 2);
}

// ------------------------------------------------------------------------ limits

#[test]
fn absurdly_large_models_give_a_clear_error() {
    // 4000 x 2000 units needs 400 x 200 plates, more blocks than a structure holds
    let data = glb(&[Object::new(&RECT, &QUAD_IDX)], vec![]);
    let err = mesh::generate(
        &data,
        None,
        &Options {
            scale: 2000.0,
            ..opts()
        },
    )
    .unwrap_err();
    assert!(err.to_string().contains("65535"), "{err}");
}

#[test]
fn degenerate_triangles_are_skipped() {
    let pts = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]]; // collinear
    let data = glb(&[Object::new(&pts, &[0, 1, 2])], vec![]);
    assert_eq!(run(&data, &opts()).0.len(), 0);
}

#[test]
fn the_structure_survives_saving() {
    let data = glb(
        &[Object::new(&RECT, &QUAD_IDX).mesh_extras(json!({"sw_block": "glass"}))],
        vec![],
    );
    let (s, _) = mesh::generate(
        &data,
        None,
        &Options {
            mode: Mode::Hollow,
            ..opts()
        },
    )
    .unwrap();
    let path = std::env::temp_dir().join("sw_tools_mesh_roundtrip.structure");
    s.save(&path, &Default::default()).unwrap();
    let back = sw_formats::structure::Structure::load(&path).unwrap();
    assert_eq!(back.blocks.len(), 1);
    assert_eq!(back.blocks[0].id, 121);
    // the format quantizes, so allow a small error; the building is also re-centered
    let (a, b) = (front_corners(&s.blocks[0]), front_corners(&back.blocks[0]));
    let shift = a[0] - b[0];
    for (x, y) in a.iter().zip(&b) {
        assert!(((*x - *y) - shift).length() < 5e-3, "{x} vs {y}");
    }
    let _ = std::fs::remove_file(path);
}

// ------------------------------------------------------------ in-game limits

/// Every plane of every block must be inside what the game allows.
fn assert_in_game_limits(blocks: &[Block]) {
    for b in blocks {
        let v = &b.settings.as_ref().unwrap().vectors;
        for c in [&v[0], &v[2]] {
            assert!(
                c.x.abs() <= 6.0 && c.y.abs() <= 5.0 && c.z.abs() <= 6.0,
                "position {c:?} out of range"
            );
        }
        for s in [&v[1], &v[3]] {
            assert!(
                (0.0..=10.0).contains(&s.x) && s.y == 0.0 && (0.0..=10.0).contains(&s.z),
                "size {s:?} out of range"
            );
        }
    }
}

/// Area of the front face of a block (trapezoid).
fn face_area(b: &Block) -> f32 {
    let v = &b.settings.as_ref().unwrap().vectors;
    0.5 * (v[1].x + v[3].x) * (v[0].y - v[2].y)
}

fn rect(w: f32, h: f32) -> [[f32; 3]; 4] {
    [[0.0, 0.0, 0.0], [w, 0.0, 0.0], [w, h, 0.0], [0.0, h, 0.0]]
}

#[test]
fn small_faces_stay_inside_the_limits() {
    let data = glb(&[Object::new(&RECT, &QUAD_IDX)], vec![]);
    let (blocks, _) = run(&data, &opts());
    assert_in_game_limits(&blocks);
    // centered on the face: the origin is the middle of the 2 x 1 rectangle
    assert!(same(
        Vec3::new(
            blocks[0].position.x,
            blocks[0].position.y,
            blocks[0].position.z
        ),
        Vec3::new(1.0, 0.5, 0.0)
    ));
}

#[test]
fn a_10_by_10_face_is_one_block() {
    let r = rect(10.0, 10.0);
    let data = glb(&[Object::new(&r, &QUAD_IDX)], vec![]);
    let (blocks, _) = run(&data, &opts());
    assert_eq!(blocks.len(), 1);
    assert_in_game_limits(&blocks);
    assert_corners(&blocks, &r.map(v));
}

#[test]
fn big_faces_are_split_to_fit_and_cover_the_same_area() {
    for (w, h) in [(11.0, 1.0), (1.0, 11.0), (25.0, 18.0), (7.0, 40.0)] {
        let r = rect(w, h);
        let data = glb(&[Object::new(&r, &QUAD_IDX)], vec![]);
        let (blocks, _) = run(&data, &opts());
        assert!(blocks.len() > 1, "{w}x{h} should need several blocks");
        assert_in_game_limits(&blocks);
        let area: f32 = blocks.iter().map(face_area).sum();
        assert!(
            (area - w * h).abs() < 1e-2 * w * h,
            "{w}x{h}: covered {area}"
        );
        // nothing sticks out of the original rectangle
        for c in blocks.iter().flat_map(front_corners) {
            assert!(
                c.x > -1e-3 && c.x < w + 1e-3 && c.y > -1e-3 && c.y < h + 1e-3 && c.z.abs() < 1e-3,
                "{c}"
            );
        }
    }
}

#[test]
fn big_triangles_and_skewed_quads_are_split_correctly() {
    // a 30 x 24 right triangle, and a very skewed parallelogram
    let tri = [[0.0, 0.0, 0.0], [30.0, 0.0, 0.0], [0.0, 24.0, 0.0]];
    let para = [
        [0.0, 0.0, 0.0],
        [8.0, 0.0, 0.0],
        [38.0, 9.0, 0.0],
        [30.0, 9.0, 0.0],
    ];
    for (pts, idx, area) in [
        (&tri[..], &[0u16, 1, 2][..], 360.0f32),
        (&para[..], &QUAD_IDX[..], 72.0),
    ] {
        let data = glb(&[Object::new(pts, idx)], vec![]);
        let (blocks, _) = run(&data, &opts());
        assert_in_game_limits(&blocks);
        let total: f32 = blocks.iter().map(face_area).sum();
        assert!(
            (total - area).abs() < 1e-2 * area,
            "covered {total}, expected {area}"
        );
    }
}

#[test]
fn split_faces_in_3d_keep_their_corners_on_the_original_plane() {
    let rot = Quat::from_euler(EulerRot::XYZ, 0.7, 0.2, -0.5);
    let pts: Vec<Vec3> = rect(23.0, 14.0)
        .iter()
        .map(|p| rot * v(*p) + Vec3::new(3.0, 4.0, 5.0))
        .collect();
    let positions: Vec<[f32; 3]> = pts.iter().map(|p| p.to_array()).collect();
    let data = glb(&[Object::new(&positions, &QUAD_IDX)], vec![]);
    let (blocks, _) = run(&data, &opts());
    assert_in_game_limits(&blocks);
    let normal = (pts[1] - pts[0]).cross(pts[3] - pts[0]).normalize();
    for c in blocks.iter().flat_map(front_corners) {
        assert!((c - pts[0]).dot(normal).abs() < 1e-3, "{c} left the plane");
    }
    let area: f32 = blocks.iter().map(face_area).sum();
    assert!((area - 23.0 * 14.0).abs() < 1.0, "covered {area}");
}

#[test]
fn long_wireframe_edges_are_chained() {
    let line = [[0.0, 0.0, 0.0], [25.0, 0.0, 0.0], [25.0, 0.1, 0.0]];
    let data = glb(
        &[Object::new(&line, &[0, 1, 2]).mesh_extras(json!({"sw_mode": "wireframe"}))],
        vec![],
    );
    let (blocks, _) = run(&data, &opts());
    assert_in_game_limits(&blocks);
    // 25 long -> 3 bars of 8.33; the other two edges are 0.1 and ~25 long
    let long_bars = blocks
        .iter()
        .filter(|b| (b.settings.as_ref().unwrap().vectors[0].y * 2.0) > 8.0)
        .count();
    assert_eq!(long_bars, 6, "two 25-long edges, 3 bars each");
    // the bars of the first edge tile it without gaps
    let mut mids: Vec<f32> = blocks
        .iter()
        .filter(|b| {
            b.position.y.abs() < 1e-3
                && b.position.x < 25.0 - 1e-3
                && (b.settings.as_ref().unwrap().vectors[0].y * 2.0) > 8.0
                && b.position.z.abs() < 1e-3
        })
        .map(|b| b.position.x)
        .collect();
    mids.sort_by(f32::total_cmp);
    assert_eq!(mids.len(), 3);
    for (m, e) in mids.iter().zip([25.0 / 6.0, 25.0 / 2.0, 25.0 * 5.0 / 6.0]) {
        assert!((m - e).abs() < 1e-3, "{mids:?}");
    }
}

#[test]
fn thickness_is_limited_to_what_a_plate_can_be() {
    let data = glb(&[Object::new(&RECT, &QUAD_IDX)], vec![]);
    let err = mesh::generate(
        &data,
        None,
        &Options {
            thickness: 10.5,
            ..opts()
        },
    )
    .unwrap_err();
    assert!(err.to_string().contains("thickness"), "{err}");
    let (blocks, _) = run(
        &data,
        &Options {
            thickness: 10.0,
            ..opts()
        },
    );
    assert_in_game_limits(&blocks);

    let data = glb(
        &[Object::new(&RECT, &QUAD_IDX).mesh_extras(json!({"sw_thickness": 11}))],
        vec![],
    );
    let err = mesh::generate(&data, None, &opts())
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("mesh #0") && err.contains("thickness"),
        "{err}"
    );
}

// ------------------------------------------------------- coordinate conversion

/// Where the block's body is, relative to its front face, along world z.
fn back_z(b: &Block) -> f32 {
    let v = &b.settings.as_ref().unwrap().vectors;
    let rot = Quat::from_euler(
        EulerRot::YXZ,
        b.rotation.y.to_radians(),
        b.rotation.x.to_radians(),
        b.rotation.z.to_radians(),
    );
    let pos = Vec3::new(b.position.x, b.position.y, b.position.z);
    (pos + rot * Vec3::new(v[2].x, v[2].y, v[2].z - v[3].z / 2.0)).z
}

#[test]
fn default_conversion_mirrors_x() {
    let data = glb(&[Object::new(&RECT, &QUAD_IDX)], vec![]);
    let (blocks, _) = run(&data, &Options::default());
    assert_eq!(blocks.len(), 1);
    let expected: Vec<Vec3> = RECT.iter().map(|p| Vec3::new(-p[0], p[1], p[2])).collect();
    assert_corners(&blocks, &expected);
}

#[test]
fn z_mirror_negates_z() {
    let tilted = [
        [0.0, 0.0, 0.0],
        [2.0, 0.0, 1.0],
        [2.0, 1.0, 1.0],
        [0.0, 1.0, 0.0],
    ];
    let data = glb(&[Object::new(&tilted, &QUAD_IDX)], vec![]);
    let (blocks, _) = run(
        &data,
        &Options {
            mirror: Mirror::Z,
            ..Default::default()
        },
    );
    assert_eq!(blocks.len(), 1);
    let expected: Vec<Vec3> = tilted
        .iter()
        .map(|p| Vec3::new(p[0], p[1], -p[2]))
        .collect();
    assert_corners(&blocks, &expected);
}

#[test]
fn thickness_stays_behind_the_face_whatever_the_mirror() {
    // RECT is counter-clockwise seen from +z, so its outward side is +z and the
    // plate should extend towards -z, in every coordinate system
    let data = glb(&[Object::new(&RECT, &QUAD_IDX)], vec![]);
    for mirror in [Mirror::None, Mirror::X, Mirror::Z] {
        let o = Options {
            mirror,
            thickness: 0.2,
            ..Default::default()
        };
        let (blocks, _) = run(&data, &o);
        let outward = if mirror == Mirror::Z { -1.0 } else { 1.0 }; // z mirror flips the face to -z
        let body = back_z(&blocks[0]);
        assert!(
            (body - (-0.2 * outward)).abs() < 1e-4,
            "{mirror:?}: back face at z = {body}"
        );
    }
}

#[test]
fn mirror_and_mirrored_node_cancel_out() {
    // a node with scale -1 on x, under the default x mirror, lands where the raw model is
    let data = glb(
        &[Object::new(&RECT, &QUAD_IDX).node(json!({"scale": [-1.0, 1.0, 1.0]}))],
        vec![],
    );
    let (blocks, _) = run(&data, &Options::default());
    assert_eq!(blocks.len(), 1);
    assert_corners(&blocks, &RECT.map(v));
}
