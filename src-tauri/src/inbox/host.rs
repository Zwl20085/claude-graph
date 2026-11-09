//! This machine's name as written to `agent.host`, and the runner's run ids.

use chrono::NaiveDateTime;

/// Longest host token.
pub const HOST_MAX: usize = 63;
pub const UNKNOWN_HOST: &str = "unknown-host";

/// Letters, digits, `.`, `_`, `-`; other characters become `-`; at most
/// [`HOST_MAX`] chars; [`UNKNOWN_HOST`] when nothing is left.
pub fn sanitise_host(raw: &str) -> String {
    let token: String = raw
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .take(HOST_MAX)
        .collect();
    let token = token.trim_matches('-').to_string();
    if token.is_empty() {
        UNKNOWN_HOST.to_string()
    } else {
        token
    }
}

/// `COMPUTERNAME` (Windows), else `HOSTNAME`, else the `hostname` file.
pub fn this_host() -> String {
    let raw = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok())
        .unwrap_or_default();
    sanitise_host(&raw)
}

/// `20261109-073100-ab12`: local time plus 4 random hex digits.
pub fn new_inbox_run_id(now: NaiveDateTime) -> String {
    let rand = uuid::Uuid::new_v4().simple().to_string();
    format!("{}-{}", now.format("%Y%m%d-%H%M%S"), &rand[..4])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_tokens() {
        assert_eq!(sanitise_host("DESKTOP-ABC"), "DESKTOP-ABC");
        assert_eq!(sanitise_host(" my pc\n"), "my-pc");
        assert_eq!(sanitise_host("研究室"), UNKNOWN_HOST);
        assert_eq!(sanitise_host(&"x".repeat(100)).len(), HOST_MAX);
        assert!(!this_host().is_empty());
    }

    #[test]
    fn run_ids_follow_the_protocol_example() {
        let at = chrono::NaiveDate::from_ymd_opt(2026, 11, 9)
            .unwrap()
            .and_hms_opt(7, 31, 0)
            .unwrap();
        let id = new_inbox_run_id(at);
        assert!(id.starts_with("20261109-073100-") && id.len() == 20, "{id}");
    }
}
