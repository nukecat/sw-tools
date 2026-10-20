use std::{error::Error, fmt, io::Cursor};

use sw_formats::{catalog::BlockCatalog, primitives::{Rgba32, Vec3}, structure::{Block, Root, Structure}};
use image::{GenericImageView, ImageError, ImageReader, Rgba};

#[derive(Debug)]
pub enum PixelArtError {
    Image(ImageError),
}

impl fmt::Display for PixelArtError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PixelArtError::Image(err) => write!(f, "image error: {err}"),
        }
    }
}

impl Error for PixelArtError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            PixelArtError::Image(err) => Some(err),
        }
    }
}

impl From<ImageError> for PixelArtError {
    fn from(value: ImageError) -> Self {
        PixelArtError::Image(value)
    }
}

type Result<T> = std::result::Result<T, PixelArtError>;

pub fn generate(image_bytes: &[u8], block_type: i32, spacing: f32, state: f32) -> Result<Structure> {
    let img = ImageReader::new(Cursor::new(image_bytes)).with_guessed_format().expect("cursor io never fails").decode()?;

    let mut s = Structure {
        roots: vec![Root::default()],
        ..Default::default()
    };

    for (x, y, Rgba::<u8>([r, g, b, a])) in img.pixels() {
        s.blocks.push(Block {
            position: Vec3::new(x as f32 * spacing - (img.width() / 2) as f32, -(y as f32) * spacing, 0.0),
            color: Some(Rgba32::new(r, g, b, a)),
            state,
            ..Block::new(block_type, 0)
        });
    }

    Ok(s)
}
