//! ワークスペースとレシピの ID に共通する、文字列の形。
//!
//! どちらも 128 ビットの乱数を、16 進数の小文字 32 文字で表す。

use std::fmt::Write as _;

/// ID の文字数。
pub const LEN: usize = 32;

/// 16 バイトの乱数を、16 進数の小文字 32 文字にする。
pub fn hex_from_bytes(bytes: [u8; 16]) -> String {
    let mut text = String::with_capacity(LEN);
    for byte in bytes {
        // String への書き込みは失敗しないので、結果は捨てる。
        let _ = write!(text, "{byte:02x}");
    }
    text
}

/// 文字列が ID の形（16 進数の小文字 32 文字）かを返す。大文字の 16 進数は受け付けない。
pub fn is_valid(value: &str) -> bool {
    value.len() == LEN
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::{hex_from_bytes, is_valid};

    #[test]
    fn hex_from_bytes_makes_lowercase_hex() {
        let text = hex_from_bytes([
            0x00, 0x01, 0x0a, 0xff, 0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80, 0x90, 0xa0,
            0xb0, 0xc0,
        ]);
        assert_eq!(text, "00010aff102030405060708090a0b0c0");
    }

    #[test]
    fn is_valid_accepts_lowercase_hex_of_32_chars() {
        assert!(is_valid("0123456789abcdef0123456789abcdef"));
    }

    #[test]
    fn is_valid_rejects_wrong_length_or_characters() {
        for text in [
            "",
            "0123456789abcdef0123456789abcde",   // 31 文字
            "0123456789abcdef0123456789abcdef0", // 33 文字
            "0123456789ABCDEF0123456789ABCDEF",  // 大文字
            "0123456789abcdef0123456789abcdeg",  // 16 進数でない文字
            "0123456789abcdef-123456789abcdef",
        ] {
            assert!(!is_valid(text), "{text:?}");
        }
    }
}
