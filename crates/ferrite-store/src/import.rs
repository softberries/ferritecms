//! Backend parity checks and the token codec of the import tables (`0009_import`,
//! `0010_import_jobs`).

use ferrite_domain::{ImportMapRow, ImportRun, to_hex};

use crate::{StoreError, validate_json, validate_text};

/// Validates `ImportRepo::create_run` input (NUL in its texts or JSON).
pub fn check_import_run(run: &ImportRun) -> Result<(), StoreError> {
    validate_text("import source", &run.source)?;
    if let Some(step) = &run.step {
        validate_text("import step", step)?;
    }
    if let Some(hash) = &run.report_hash {
        validate_text("import report hash", hash)?;
    }
    validate_json(&run.stats)
}

/// Validates `ImportRepo::map_put` input (NUL in its texts).
pub fn check_import_map(row: &ImportMapRow) -> Result<(), StoreError> {
    validate_text("import source", &row.source)?;
    validate_text("import hash", &row.hash)
}

/// The stored (lowercase hex) form of a run token.
pub fn token_to_db(token: &[u8; 16]) -> String {
    to_hex(token)
}

/// Decodes a stored run token; anything but 32 lowercase hex digits is corrupt.
pub fn token_from_db(text: &str) -> Result<[u8; 16], StoreError> {
    let corrupt = || StoreError::Database(format!("corrupt import token: {text:?}"));
    let digits = text.as_bytes();
    if digits.len() != 32 {
        return Err(corrupt());
    }
    let nibble = |b: u8| match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        _ => None,
    };
    let mut token = [0u8; 16];
    let (pairs, _) = digits.as_chunks::<2>();
    for (out, pair) in token.iter_mut().zip(pairs) {
        let (Some(hi), Some(lo)) = (nibble(pair[0]), nibble(pair[1])) else {
            return Err(corrupt());
        };
        *out = hi << 4 | lo;
    }
    Ok(token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_roundtrips_and_rejects_garbage() {
        let token = [
            0x00, 0x0f, 0xa0, 0xff, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12,
        ];
        assert_eq!(token_from_db(&token_to_db(&token)).unwrap(), token);
        assert!(token_from_db("").is_err());
        assert!(token_from_db(&"A".repeat(32)).is_err());
        assert!(token_from_db(&"g".repeat(32)).is_err());
    }
}
