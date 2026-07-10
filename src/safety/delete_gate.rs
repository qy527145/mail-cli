use crate::error::{Error, Result};

pub const ENV_VAR: &str = "MAIL_CLI_DELETE_ENABLED";

/// Enforce the two-gate delete authorization.
///
/// Both must be satisfied:
///   1. env `MAIL_CLI_DELETE_ENABLED=true` (operator-controlled, per-session)
///   2. per-call `--user-explicitly-requested-deletion` flag (caller-controlled, per-message)
///
/// The intent is that neither the operator nor the caller can unilaterally delete;
/// deletion requires an out-of-band operator opt-in AND a fresh user acknowledgment.
pub fn check(explicit_flag: bool) -> Result<()> {
    if !explicit_flag {
        return Err(Error::Input(
            "delete requires --user-explicitly-requested-deletion".into(),
        ));
    }
    let env = std::env::var(ENV_VAR).unwrap_or_default();
    if env != "true" {
        return Err(Error::Input(format!(
            "delete requires env {ENV_VAR}=true (currently: {env:?})"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // SAFETY: these tests mutate a process-global env var — force serial execution.
    fn with_env<T>(value: Option<&str>, f: impl FnOnce() -> T) -> T {
        use std::sync::Mutex;
        static LOCK: Mutex<()> = Mutex::new(());
        let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
        // SAFETY: single-threaded under LOCK for the duration of `f`.
        unsafe {
            match value {
                Some(v) => std::env::set_var(ENV_VAR, v),
                None => std::env::remove_var(ENV_VAR),
            }
        }
        let out = f();
        unsafe {
            std::env::remove_var(ENV_VAR);
        }
        out
    }

    #[test]
    fn flag_alone_is_not_enough() {
        with_env(None, || {
            assert!(check(true).is_err());
        });
    }

    #[test]
    fn env_alone_is_not_enough() {
        with_env(Some("true"), || {
            assert!(check(false).is_err());
        });
    }

    #[test]
    fn both_gates_pass() {
        with_env(Some("true"), || {
            assert!(check(true).is_ok());
        });
    }

    #[test]
    fn env_must_be_exactly_true() {
        with_env(Some("1"), || {
            assert!(check(true).is_err());
        });
        with_env(Some("yes"), || {
            assert!(check(true).is_err());
        });
    }
}
