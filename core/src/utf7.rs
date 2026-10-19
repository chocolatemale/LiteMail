//! IMAP modified UTF-7 (RFC 3501 §5.1.3) for mailbox names.

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+,";

fn value(c: u8) -> Option<u32> {
    ALPHABET.iter().position(|&a| a == c).map(|p| p as u32)
}

/// Decodes a mailbox name. Malformed sequences are kept verbatim.
pub fn decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'&' {
            // Safe: we only split at ASCII boundaries.
            let start = i;
            while i < bytes.len() && bytes[i] != b'&' {
                i += 1;
            }
            out.push_str(&input[start..i]);
            continue;
        }
        let Some(end) = input[i + 1..].find('-').map(|p| p + i + 1) else {
            out.push_str(&input[i..]);
            break;
        };
        if end == i + 1 {
            out.push('&');
        } else if let Some(s) = decode_run(&bytes[i + 1..end]) {
            out.push_str(&s);
        } else {
            out.push_str(&input[i..=end]);
        }
        i = end + 1;
    }
    out
}

fn decode_run(run: &[u8]) -> Option<String> {
    let mut bits: u32 = 0;
    let mut nbits = 0;
    let mut units = Vec::new();
    for &c in run {
        bits = (bits << 6) | value(c)?;
        nbits += 6;
        if nbits >= 16 {
            nbits -= 16;
            units.push(((bits >> nbits) & 0xFFFF) as u16);
        }
    }
    String::from_utf16(&units).ok()
}

/// Encodes a mailbox name for use in IMAP commands.
pub fn encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut pending: Vec<u16> = Vec::new();
    for ch in input.chars() {
        if (' '..='~').contains(&ch) {
            flush(&mut pending, &mut out);
            if ch == '&' {
                out.push_str("&-");
            } else {
                out.push(ch);
            }
        } else {
            let mut buf = [0u16; 2];
            pending.extend_from_slice(ch.encode_utf16(&mut buf));
        }
    }
    flush(&mut pending, &mut out);
    out
}

fn flush(units: &mut Vec<u16>, out: &mut String) {
    if units.is_empty() {
        return;
    }
    let bytes: Vec<u8> = units.iter().flat_map(|u| u.to_be_bytes()).collect();
    out.push('&');
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        let chars = match chunk.len() {
            1 => 2,
            2 => 3,
            _ => 4,
        };
        for k in 0..chars {
            out.push(ALPHABET[((n >> (18 - 6 * k)) & 63) as usize] as char);
        }
    }
    out.push('-');
    units.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_rfc_examples() {
        assert_eq!(decode("~peter/mail/&U,BTFw-/&ZeVnLIqe-"), "~peter/mail/台北/日本語");
        assert_eq!(decode("&Jjo-!"), "☺!");
        assert_eq!(decode("Tom &- Jerry"), "Tom & Jerry");
        assert_eq!(decode("INBOX"), "INBOX");
    }

    #[test]
    fn decodes_chinese_folders() {
        assert_eq!(decode("&XfJT0ZAB-"), "已发送");
        assert_eq!(decode("&V4NXPpCuTvY-"), "垃圾邮件");
    }

    #[test]
    fn round_trips() {
        for s in ["已发送", "项目/2026 计划", "Tom & Jerry", "😀 emoji", "INBOX", "a&b&c"] {
            assert_eq!(decode(&encode(s)), s, "{s}");
        }
        assert_eq!(encode("台北"), "&U,BTFw-");
    }

    #[test]
    fn keeps_malformed_input() {
        assert_eq!(decode("broken&abc"), "broken&abc");
        assert_eq!(decode("bad&***-x"), "bad&***-x");
    }
}
