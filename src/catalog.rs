use clap::ValueEnum;
use comfy_table::{presets::NOTHING, Attribute, Cell, Color, Table};
use sw_formats::catalog::{
    BlockCatalog, BlockKind, CustomDataKind, LayerInfo, MapCatalog, MapPropCategory, PropInfo,
};
// use sw_formats::primitives::Vec3;

#[derive(Clone, ValueEnum)]
pub enum Category {
    Blocks,
    Props,
    Layers,
}

impl Category {
    pub fn table(&self) -> Table {
        match self {
            Category::Blocks => block_table(),
            Category::Props => prop_table(),
            Category::Layers => layer_table(),
        }
    }

    /// Prints the table plus any category-specific footer.
    pub fn print(&self) {
        println!("{}", self.table());

        if let Category::Layers = self {
            let catalog = MapCatalog::shared();
            let names: Vec<String> = catalog
                .new_map_layers
                .iter()
                .map(|id| match catalog.layer(*id) {
                    Some(l) => format!("{} ({})", l.name, id),
                    None => format!("? ({id})"),
                })
                .collect();

            println!();
            println!("New map layers: {}", names.join(", "));
        }
    }
}

// ---------- shared helpers ----------

fn new_table(headers: &[&str]) -> Table {
    let mut table = Table::new();
    table.load_style(NOTHING);
    table.set_header(headers.iter().map(|h| header_cell(h)).collect::<Vec<Cell>>());
    table
}

fn header_cell(text: &str) -> Cell {
    Cell::new(text).add_attribute(Attribute::Bold).fg(Color::Cyan)
}

fn id_cell(id: impl ToString) -> Cell {
    Cell::new(id.to_string()).fg(Color::DarkGrey)
}

fn name_cell(name: &str) -> Cell {
    Cell::new(name).add_attribute(Attribute::Bold)
}

fn bool_cell(value: bool) -> Cell {
    if value {
        Cell::new(value).fg(Color::Green)
    } else {
        Cell::new(value).fg(Color::DarkGrey)
    }
}

// fn vec3_cell(v: &Vec3) -> Cell {
//     Cell::new(format!("{:.2}, {:.2}, {:.2}", v.x, v.y, v.z))
// }

// ---------- blocks ----------

fn kind_cell(kind: &BlockKind) -> Cell {
    match kind {
        BlockKind::Block => Cell::new("Block").fg(Color::Blue),
        BlockKind::Part => Cell::new("Part").fg(Color::Yellow),
        BlockKind::Tool => Cell::new("Tool").fg(Color::Magenta),
    }
}

fn custom_data_cell(kind: &CustomDataKind) -> Cell {
    match kind {
        CustomDataKind::None => Cell::new("None").fg(Color::DarkGrey),
        CustomDataKind::Math => Cell::new("Math").fg(Color::Green),
        CustomDataKind::Microcontroller => Cell::new("Microcontroller").fg(Color::Green),
    }
}

pub fn block_table() -> Table {
    let mut table = new_table(&["ID", "Name", "Kind", "Stateless", "Custom shape", "Custom data"]);

    for b in BlockCatalog::shared().iter() {
        table.add_row::<Vec<Cell>>(vec![
            id_cell(b.id),
            name_cell(&b.name),
            kind_cell(&b.kind),
            bool_cell(b.stateless),
            bool_cell(b.custom_shape),
            custom_data_cell(&b.custom_data),
        ]);
    }

    table
}

// ---------- props ----------

fn prop_category_cell(category: MapPropCategory) -> Cell {
    let (text, color) = match category {
        MapPropCategory::Trees => ("Trees", Color::Green),
        MapPropCategory::Rocks => ("Rocks", Color::Grey),
        MapPropCategory::Buildings => ("Buildings", Color::Yellow),
        MapPropCategory::Roads => ("Roads", Color::DarkYellow),
        MapPropCategory::Bridges => ("Bridges", Color::Blue),
        MapPropCategory::Props => ("Props", Color::Magenta),
        MapPropCategory::Shapes => ("Shapes", Color::Cyan),
        MapPropCategory::Background => ("Background", Color::DarkGrey),
    };
    Cell::new(text).fg(color)
}

pub fn prop_table() -> Table {
    let mut table = new_table(&[
        "ID", "Name", "Category", "Hidden", "Free scale", "Floors", "Default floors",
        "Colorable", "Glowable", "Transparent", "Hidden in game", "Destroyable",
    ]);

    let catalog = MapCatalog::shared();
    let mut props: Vec<&PropInfo> = catalog.props.values().collect();
    props.sort_by_key(|p| p.id);

    for p in props {
        table.add_row::<Vec<Cell>>(vec![
            id_cell(p.id),
            name_cell(&p.name),
            prop_category_cell(p.category),
            bool_cell(p.hidden),
            bool_cell(p.allow_free_scale),
            bool_cell(p.has_floors),
            if p.has_floors {
                Cell::new(p.default_floors)
            } else {
                Cell::new(p.default_floors).fg(Color::DarkGrey)
            },
            bool_cell(p.colorable),
            bool_cell(p.glowable),
            bool_cell(p.transparent),
            bool_cell(p.hidden_in_game),
            bool_cell(p.destroyable),
        ]);
    }

    table
}

// ---------- layers ----------

pub fn layer_table() -> Table {
    let mut table = new_table(&["ID", "Name", "Grass", "Hidden"]);

    let catalog = MapCatalog::shared();
    let mut layers: Vec<&LayerInfo> = catalog.layers.values().collect();
    layers.sort_by_key(|l| l.id);

    for l in layers {
        table.add_row::<Vec<Cell>>(vec![
            id_cell(l.id),
            name_cell(&l.name),
            bool_cell(l.grass),
            bool_cell(l.hidden),
        ]);
    }

    table
}
