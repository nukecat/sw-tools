use std::{error::Error, fmt};

#[derive(Debug)]
pub enum HexError {
    OddLength,
    InvalidHex(usize),
}

impl fmt::Display for HexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HexError::OddLength => write!(f, "odd length"),
            HexError::InvalidHex(pos) => write!(f, "invalid hes at {pos}"),
        }
    }
}

impl Error for HexError {}

type Result<T> = std::result::Result<T, HexError>;

pub fn decode(s: &str) -> Result<Vec<u8>> {
    if s.len() % 2 != 0 {
        return Err(HexError::OddLength);
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| HexError::InvalidHex(i)))
        .collect()
}
