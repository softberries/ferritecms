//! Lowercase hex, the one encoding every content hash, fingerprint and ETag uses.

/// `bytes` as lowercase hex, two digits per byte.
pub fn to_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(char::from(DIGITS[usize::from(b >> 4)]));
        out.push(char::from(DIGITS[usize::from(b & 0x0f)]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lowercase_two_digits_per_byte() {
        assert_eq!(to_hex(&[]), "");
        assert_eq!(to_hex(&[0x00, 0x0f, 0xa0, 0xff]), "000fa0ff");
        let all: Vec<u8> = (0..=255).collect();
        let want: String = all.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(to_hex(&all), want);
    }
}
