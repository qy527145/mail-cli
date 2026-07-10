use crate::error::{Error, Result};

/// Enforce the account's send allowlist.
///
/// Rules:
/// - Empty allowlist → all recipients rejected (fail-closed default).
/// - Exact address match (case-insensitive): `"a@b.com"` allows `"a@b.com"`.
/// - Wildcard domain: `"*@example.com"` allows any local part at `example.com`.
///
/// Returns [`Error::Input`] on any mismatch, mapped to exit code 3.
pub fn check(recipients: &[String], allowlist: &[String]) -> Result<()> {
    if allowlist.is_empty() {
        return Err(Error::Input(format!(
            "send allowlist is empty; add addresses to `send_allowlist` in your account config \
             (or use --dry-run). Rejected recipients: {recipients:?}"
        )));
    }
    let disallowed: Vec<&String> = recipients
        .iter()
        .filter(|r| !matches_any(r, allowlist))
        .collect();
    if !disallowed.is_empty() {
        return Err(Error::Input(format!(
            "recipients not in send_allowlist: {disallowed:?}"
        )));
    }
    Ok(())
}

fn matches_any(addr: &str, allowlist: &[String]) -> bool {
    allowlist
        .iter()
        .any(|pattern| matches_pattern(addr, pattern))
}

fn matches_pattern(addr: &str, pattern: &str) -> bool {
    if let Some(domain) = pattern.strip_prefix("*@") {
        addr.rsplit('@')
            .next()
            .is_some_and(|d| d.eq_ignore_ascii_case(domain))
    } else {
        addr.eq_ignore_ascii_case(pattern)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_allowlist_rejects_everything() {
        let e = check(&["a@b".to_string()], &[]).unwrap_err();
        assert!(matches!(e, Error::Input(_)));
    }

    #[test]
    fn exact_match_case_insensitive() {
        assert!(check(&["A@B.com".to_string()], &["a@b.com".to_string()]).is_ok());
    }

    #[test]
    fn wildcard_domain() {
        assert!(check(&["x@example.com".to_string()], &["*@example.com".to_string()]).is_ok());
        assert!(check(&["x@evil.com".to_string()], &["*@example.com".to_string()]).is_err());
    }

    #[test]
    fn mixed_rejects_disallowed() {
        assert!(
            check(
                &["ok@a.com".to_string(), "bad@b.com".to_string()],
                &["ok@a.com".to_string()],
            )
            .is_err()
        );
    }
}
