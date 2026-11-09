//! Body hash, docs/AGENT-PROTOCOL.md section 4.

use sha2::{Digest, Sha256};

/// Prefix of a stored hash value.
pub const HASH_PREFIX: &str = "sha256:";

/// `normalise(body)`: drop a leading U+FEFF, turn `\r\n` and lone `\r` into
/// `\n`, strip trailing spaces and tabs from every line, drop leading and
/// trailing empty lines; joined with `\n` and no final newline.
pub fn normalise(body: &str) -> String {
    let body = body.strip_prefix('\u{feff}').unwrap_or(body);
    let unified = body.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = unified
        .split('\n')
        .map(|l| l.trim_end_matches([' ', '\t']))
        .collect();
    let first = lines.iter().position(|l| !l.is_empty());
    let last = lines.iter().rposition(|l| !l.is_empty());
    match (first, last) {
        (Some(a), Some(b)) => lines[a..=b].join("\n"),
        _ => String::new(),
    }
}

/// Lowercase hex SHA-256 of `normalise(body)` (no prefix).
pub fn body_hex(body: &str) -> String {
    Sha256::digest(normalise(body).as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// `"sha256:" + hex`, the value stored in `agent.hash`.
pub fn body_hash(body: &str) -> String {
    format!("{HASH_PREFIX}{}", body_hex(body))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The five rows of the protocol's test vector table (row 3 has two inputs).
    #[test]
    fn protocol_test_vectors() {
        let rows: [(&str, &str); 6] = [
            (
                "Find three recent papers on hairpin AC loss.\r\n\r\n",
                "a7b5da43b6fbfa12a04b1d93b1c7eb5b224f672554632f78c5bf68c6f5a2386f",
            ),
            (
                "\u{feff}\n  line one  \r\n\tline two\t\n\n\n",
                "7ceac30c0680c7b43ef2cc5971ddff12cd7584a56cf6fa615f865ffb6f97adca",
            ),
            (
                "",
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            ),
            (
                "\n \n",
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            ),
            (
                "调研 ⭐\n",
                "2115d91d771e0eafc845500cc950c60545e93cd145b2f4b80434fd36a6bf9f95",
            ),
            (
                "Prompt\n\n<!-- agent:begin 2026-11-09T07:31:00 question -->\n## ❓ Questions\n1. Which year range?\n<!-- agent:end -->\n\n2020 onwards\n",
                "262828d169635feb78f04a5a63dde71f681a21c534598a9b19556af76446b9ac",
            ),
        ];
        for (body, hex) in rows {
            assert_eq!(body_hex(body), hex, "body {body:?}");
            assert_eq!(body_hash(body), format!("sha256:{hex}"));
        }
    }

    #[test]
    fn normalise_keeps_inner_blank_lines_and_leading_spaces() {
        assert_eq!(normalise("\r\n  a \r\r\n\tb\r"), "  a\n\n\tb");
        assert_eq!(normalise("x\u{feff}"), "x\u{feff}");
    }
}
