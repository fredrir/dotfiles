use serde_json::Value;
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::fs;
use std::io;
use std::path::PathBuf;

#[derive(Debug)]
pub enum ContractError {
    InvalidName(String),
    Io(io::Error),
    Json(serde_json::Error),
}

impl Display for ContractError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidName(name) => write!(formatter, "invalid contract name: {name}"),
            Self::Io(error) => Display::fmt(error, formatter),
            Self::Json(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for ContractError {}

impl From<io::Error> for ContractError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for ContractError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

pub fn contract_directory() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../../contracts/dotfile/v1")
}

pub fn load_contract(name: &str) -> Result<Value, ContractError> {
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(ContractError::InvalidName(name.to_owned()));
    }
    let bytes = fs::read(contract_directory().join(format!("{name}.json")))?;
    Ok(serde_json::from_slice(&bytes)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_a_named_contract() {
        let versions = load_contract("versions").unwrap();
        assert_eq!(versions["tuple"]["source"], "1");
    }

    #[test]
    fn rejects_path_syntax() {
        assert!(matches!(
            load_contract("../versions"),
            Err(ContractError::InvalidName(_))
        ));
    }
}
