use std::{error::Error, fmt};
use sw_formats::{catalog::{BlockCatalog, CustomDataKind}, structure::{CustomData, Structure}};

#[derive(Debug)]
pub enum InsertDataError {
    Unambiguous,
    NotFound,
    UnknownBlockId(i32),
}

impl fmt::Display for InsertDataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InsertDataError::Unambiguous => write!(f, "found multiple blocks or data fields, try being more specific"),
            InsertDataError::NotFound => write!(f, "no block found"),
            InsertDataError::UnknownBlockId(id) => write!(f, "unknown block with id {id}"),
        }
    }
}

impl Error for InsertDataError {}

type Result<T> = std::result::Result<T, InsertDataError>;

pub struct Filter {
    pub name: Option<String>,
    pub blob: Option<Vec<u8>>,
}

pub fn insert_data(s: &mut Structure, data: &[u8], filter: Filter) -> Result<()> {
    let catalog = BlockCatalog::shared();
    let mut place: Option<(usize, usize)> = None;

    for (i, b) in s.blocks.iter().enumerate() {
        let info = catalog
        .get(b.id)
        .ok_or(InsertDataError::UnknownBlockId(b.id))?;
        if info.custom_data != CustomDataKind::Microcontroller {
            continue;
        }

        if let Some(f_name) = &filter.name {
            if b.name.as_ref() != Some(f_name) {
                continue;
            }
        }

        let Some(CustomData::Microcontroller(m)) =
        b.settings.as_ref().and_then(|st| st.custom.as_ref())
        else {
            continue;
        };

        match &filter.blob {
            Some(f_blob) => {
                for (j, blob) in m.data_blobs.iter().enumerate() {
                    if blob == f_blob {
                        if place.is_some() {
                            return Err(InsertDataError::Unambiguous);
                        }
                        place = Some((i, j));
                    }
                }
            }
            None => match m.data_blobs.len() {
                0 => {}
                1 => {
                    if place.is_some() {
                        return Err(InsertDataError::Unambiguous);
                    }
                    place = Some((i, 0));
                }
                _ => return Err(InsertDataError::Unambiguous),
            },
        }
    }

    let (block_idx, data_idx) = place.ok_or(InsertDataError::NotFound)?;

    let Some(CustomData::Microcontroller(mc)) = s.blocks[block_idx]
    .settings
    .as_mut()
    .and_then(|st| st.custom.as_mut())
    else {
        unreachable!("block was matched in the search loop above");
    };
    mc.data_blobs[data_idx] = data.into();

    Ok(())
}
