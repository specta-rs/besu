//! Base64 for the wire format's binary values.
//!
//! JSON has no byte string, and an array of numbers is four to five times larger than the
//! same bytes in base64. The alphabet and padding are RFC 4648's standard ones, which is what
//! every language's decoder accepts by default.

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Encode `bytes` as standard base64 with padding.
pub(crate) fn encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = u32::from(chunk[0]);
        let second = chunk.get(1).copied().map_or(0, u32::from);
        let third = chunk.get(2).copied().map_or(0, u32::from);
        let value = (first << 16) | (second << 8) | third;

        out.push(char::from(ALPHABET[((value >> 18) & 0x3f) as usize]));
        out.push(char::from(ALPHABET[((value >> 12) & 0x3f) as usize]));
        if chunk.len() > 1 {
            out.push(char::from(ALPHABET[((value >> 6) & 0x3f) as usize]));
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(char::from(ALPHABET[(value & 0x3f) as usize]));
        } else {
            out.push('=');
        }
    }
    out
}

/// Decode standard base64 with padding.
///
/// The error is a static description rather than the offending text, because every caller
/// turns it into a decode error that already names the column.
pub(crate) fn decode(text: &str) -> Result<Vec<u8>, &'static str> {
    let bytes = text.as_bytes();
    if bytes.len() % 4 != 0 {
        return Err("base64 text must be a multiple of four characters");
    }

    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    let chunks = bytes.len() / 4;

    for (position, chunk) in bytes.chunks(4).enumerate() {
        // Padding is only ever at the end of the last group, and never in the first two
        // characters of a group.
        let padding = match (chunk[2], chunk[3]) {
            (b'=', b'=') => 2,
            (_, b'=') => 1,
            _ => 0,
        };
        if padding > 0 && position + 1 != chunks {
            return Err("base64 padding may only appear at the end of the text");
        }
        if chunk[0] == b'=' || chunk[1] == b'=' {
            return Err("base64 padding may only appear in the last two characters of a group");
        }

        let mut value = 0_u32;
        for &byte in &chunk[..4 - padding] {
            let digit =
                digit(byte).ok_or("the base64 text contains a character outside the alphabet")?;
            value = (value << 6) | digit;
        }
        value <<= 6 * padding;

        out.push((value >> 16) as u8);
        if padding < 2 {
            out.push((value >> 8) as u8);
        }
        if padding < 1 {
            out.push(value as u8);
        }
    }

    Ok(out)
}

fn digit(byte: u8) -> Option<u32> {
    match byte {
        b'A'..=b'Z' => Some(u32::from(byte - b'A')),
        b'a'..=b'z' => Some(u32::from(byte - b'a') + 26),
        b'0'..=b'9' => Some(u32::from(byte - b'0') + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{decode, encode};

    #[test]
    fn round_trips_every_remainder() {
        for length in 0..32 {
            let bytes = (0..length)
                .map(|i| (i * 37) as u8 % 251)
                .collect::<Vec<_>>();
            let text = encode(&bytes);
            assert_eq!(text.len() % 4, 0);
            assert_eq!(decode(&text), Ok(bytes), "length {length}");
        }
    }

    #[test]
    fn spells_the_rfc_examples() {
        assert_eq!(encode(b""), "");
        assert_eq!(encode(b"f"), "Zg==");
        assert_eq!(encode(b"fo"), "Zm8=");
        assert_eq!(encode(b"foo"), "Zm9v");
        assert_eq!(encode(b"foob"), "Zm9vYg==");
        assert_eq!(encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn rejects_malformed_text() {
        assert!(decode("Zg=").is_err());
        assert!(decode("Z===").is_err());
        assert!(decode("Zg==Zg==").is_err());
        assert!(decode("Z!==").is_err());
    }
}
