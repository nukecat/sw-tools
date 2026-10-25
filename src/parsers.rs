use sw_formats::catalog::{BlockCatalog, BlockInfo, BlockKind};

type Result<T> = std::result::Result<T, String>;

pub fn block_id<F: FnMut(&&BlockInfo) -> bool>(mut filter: F) -> impl FnMut(&str) -> Result<i32> {
    move |s| {
        let catalog = BlockCatalog::shared();
        let s = s.trim().to_lowercase();

        if let Ok(id) = s.parse::<i32>() {
            if let Some(b) = catalog.get(id) {
                if filter(&b) {
                    return Ok(id);
                } else {
                    return Err(format!(
                        "block with id \"{id}\" doesn't meet filter criteria"
                    ));
                }
            } else {
                return Err(format!("unknown block id: \"{id}\""));
            }
        }

        let matches: Vec<&BlockInfo> = catalog
            .iter()
            .filter(|b| filter(b))
            .filter(|b| b.name.to_lowercase().contains(&s))
            .collect();

        if matches.len() > 1 {
            return Err(format!(
                "found multiple matches for the \"{s}\", try being more specific"
            ));
        }

        let found = matches.last();

        match found {
            Some(b) => Ok(b.id),
            None => Err(format!("block doesn't meet criteria or unknown: \"{s}\"")),
        }
    }
}

pub fn custom_block(s: &str) -> Result<i32> {
    block_id(|b| b.custom_shape)(s)
}

pub fn not_a_tool(s: &str) -> Result<i32> {
    block_id(|b| b.kind != BlockKind::Tool)(s)
}
