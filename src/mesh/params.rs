//! Per-object settings: the block type, thickness, mode and color.
//!
//! They can be stored in the glTF itself, in `extras` (in Blender: *Custom
//! Properties*, with "Custom Properties" ticked in the glTF export options). Either
//! flat keys or one nested `sw` object work:
//!
//! ```json
//! { "sw_block": "glass", "sw_color": "#ff8800" }
//! { "sw": { "block": "glass", "thickness": 0.1, "mode": "wireframe", "color": [1, 0.5, 0] } }
//! ```
//!
//! | key         | value                                                              |
//! |-------------|--------------------------------------------------------------------|
//! | `block`     | name (`metal`, `wood`, `glass`, `neon`, …) or numeric block ID     |
//! | `thickness` | number above 0, at most 10, in the model's units after `--scale`   |
//! | `mode`      | `hollow` or `wireframe`                                            |
//! | `color`     | `"#rrggbb"` / `"#rrggbbaa"` (sRGB) or `[r, g, b(, a)]` linear 0–1  |
//!
//! Extras can sit on a node, mesh, material or primitive. The most specific one wins,
//! per key: primitive > material > mesh > node (a node also passes its values on to
//! its children, which may override them). Anything unset falls back to the command
//! line.

use clap::ValueEnum;
use serde_json::Value;
use sw_formats::catalog::BlockCatalog;

use super::{MeshError, Mode, color};

/// Looks up a custom-shape block by ID, by catalog name (`Custom block (glass)`), by
/// internal name (`CustomBlockGlass`) or by short name (`glass`).
pub fn parse_block(s: &str) -> Result<i32, String> {
    let catalog = BlockCatalog::shared();
    let s = s.trim();

    if let Ok(id) = s.parse::<i32>() {
        return check_custom_shape(id);
    }

    let found = catalog.iter().filter(|b| b.custom_shape).find(|b| {
        let short = b
            .internal_name
            .strip_prefix("CustomBlock")
            .unwrap_or(&b.internal_name);
        b.name.eq_ignore_ascii_case(s)
            || b.internal_name.eq_ignore_ascii_case(s)
            || short.eq_ignore_ascii_case(s)
            || (short == "Wood" && s.eq_ignore_ascii_case("wooden"))
    });
    match found {
        Some(b) => Ok(b.id),
        None => Err(format!("unknown block \"{s}\"; {}", valid_blocks())),
    }
}

fn check_custom_shape(id: i32) -> Result<i32, String> {
    match BlockCatalog::shared().get(id) {
        Some(b) if b.custom_shape => Ok(id),
        Some(b) => Err(format!(
            "block {id} ({}) is not a custom-shape block; {}",
            b.name,
            valid_blocks()
        )),
        None => Err(format!("no block with ID {id}; {}", valid_blocks())),
    }
}

fn valid_blocks() -> String {
    let list: Vec<String> = BlockCatalog::shared()
        .iter()
        .filter(|b| b.custom_shape)
        .map(|b| {
            let short = b
                .internal_name
                .strip_prefix("CustomBlock")
                .unwrap_or(&b.internal_name);
            format!("{} = {}", b.id, short.to_ascii_lowercase())
        })
        .collect();
    format!("valid blocks: {}", list.join(", "))
}

fn parse_color(v: &Value) -> Result<[u8; 4], String> {
    match v {
        Value::String(s) => {
            let hex = s.trim().trim_start_matches('#');
            if !(hex.len() == 6 || hex.len() == 8) || !hex.is_ascii() {
                return Err(bad_hex(s));
            }
            let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
            let a = if hex.len() == 8 { byte(6) } else { Some(255) };
            match (byte(0), byte(2), byte(4), a) {
                (Some(r), Some(g), Some(b), Some(a)) => Ok([r, g, b, a]),
                _ => Err(bad_hex(s)),
            }
        }
        Value::Array(items) if items.len() == 3 || items.len() == 4 => {
            let mut c = [0.0f32, 0.0, 0.0, 1.0];
            for (slot, item) in c.iter_mut().zip(items) {
                *slot = item.as_f64().ok_or("color arrays must hold numbers")? as f32;
            }
            Ok(color::encode(c))
        }
        _ => Err("expected \"#rrggbb\" or an array of 3-4 numbers between 0 and 1".into()),
    }
}

fn bad_hex(s: &str) -> String {
    format!("\"{s}\" is not a color like \"#rrggbb\" or \"#rrggbbaa\"")
}

/// Settings found in one place of the glTF. Unset fields defer to a less specific
/// place, and finally to the command line.
#[derive(Clone, Debug, Default)]
pub struct Overrides {
    pub block: Option<i32>,
    pub thickness: Option<f32>,
    pub mode: Option<Mode>,
    pub color: Option<[u8; 4]>,
}

impl Overrides {
    /// `top` wins wherever it has a value.
    pub fn over(self, top: Overrides) -> Overrides {
        Overrides {
            block: top.block.or(self.block),
            thickness: top.thickness.or(self.thickness),
            mode: top.mode.or(self.mode),
            color: top.color.or(self.color),
        }
    }

    /// Reads the settings out of an `extras` value. `who` names the owner for error
    /// messages. Extras that aren't a JSON object, or hold no `sw` settings, are
    /// ignored (other tools put their own data there).
    pub fn from_extras(extras: &gltf::json::Extras, who: &str) -> Result<Overrides, MeshError> {
        let none = Overrides::default();
        let Some(raw) = extras.as_ref() else {
            return Ok(none);
        };
        let Ok(value) = serde_json::from_str::<Value>(raw.get()) else {
            return Ok(none);
        };
        let Some(obj) = value.as_object() else {
            return Ok(none);
        };

        let nested = obj.get("sw").and_then(Value::as_object);
        let get = |key: &str| {
            obj.get(&format!("sw_{key}"))
                .or_else(|| nested.and_then(|n| n.get(key)))
        };
        let fail = |key: &str, msg: String| MeshError::Param(format!("{who}: {key}: {msg}"));

        let mut o = Overrides::default();

        if let Some(v) = get("block") {
            o.block = Some(
                match v {
                    Value::String(s) => parse_block(s),
                    Value::Number(n) => match n.as_f64() {
                        Some(f) if f.fract() == 0.0 => check_custom_shape(f as i32),
                        _ => Err("block IDs are whole numbers".into()),
                    },
                    _ => Err("expected a block name or ID".into()),
                }
                .map_err(|m| fail("block", m))?,
            );
        }

        if let Some(v) = get("thickness") {
            match v.as_f64() {
                Some(t) if t > 0.0 && t <= super::block::MAX_SIZE as f64 => {
                    o.thickness = Some(t as f32)
                }
                _ => {
                    return Err(fail(
                        "thickness",
                        format!(
                            "expected a number above 0 and at most {}",
                            super::block::MAX_SIZE
                        ),
                    ));
                }
            }
        }

        if let Some(v) = get("mode") {
            let mode = v.as_str().and_then(|s| Mode::from_str(s, true).ok());
            o.mode = Some(
                mode.ok_or_else(|| fail("mode", "expected \"hollow\" or \"wireframe\"".into()))?,
            );
        }

        if let Some(v) = get("color") {
            o.color = Some(parse_color(v).map_err(|m| fail("color", m))?);
        }

        Ok(o)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_names() {
        assert_eq!(parse_block("metal"), Ok(109));
        assert_eq!(parse_block("Wood"), Ok(120));
        assert_eq!(parse_block("wooden"), Ok(120));
        assert_eq!(parse_block("glass"), Ok(121));
        assert_eq!(parse_block("NEON"), Ok(130));
        assert_eq!(parse_block("CustomBlockGlass"), Ok(121));
        assert_eq!(parse_block("Custom block (glass)"), Ok(121));
        assert_eq!(parse_block("121"), Ok(121));
    }

    #[test]
    fn block_errors() {
        let e = parse_block("plastic").unwrap_err();
        assert!(
            e.contains("valid blocks") && e.contains("109 = metal"),
            "{e}"
        );
        // a real block, but not a custom-shape one
        assert!(parse_block("0").unwrap_err().contains("not a custom-shape"));
        assert!(parse_block("9999").unwrap_err().contains("no block"));
    }

    #[test]
    fn colors() {
        assert_eq!(parse_color(&Value::from("#ff8000")), Ok([255, 128, 0, 255]));
        assert_eq!(
            parse_color(&Value::from("ff800080")),
            Ok([255, 128, 0, 128])
        );
        assert_eq!(
            parse_color(&serde_json::json!([1, 0, 0])),
            Ok([255, 0, 0, 255])
        );
        assert_eq!(
            parse_color(&serde_json::json!([0.8, 0.8, 0.8, 0.5])),
            Ok([231, 231, 231, 128])
        );
        assert!(parse_color(&Value::from("#12345")).is_err());
        assert!(parse_color(&Value::from("#gg0000")).is_err());
        assert!(parse_color(&Value::from("#ééé")).is_err());
        assert!(parse_color(&serde_json::json!([1, 2])).is_err());
    }

    #[test]
    fn layering() {
        let base = Overrides {
            block: Some(109),
            thickness: Some(0.1),
            ..Default::default()
        };
        let top = Overrides {
            block: Some(121),
            ..Default::default()
        };
        let merged = base.over(top);
        assert_eq!(merged.block, Some(121));
        assert_eq!(merged.thickness, Some(0.1));
    }
}
