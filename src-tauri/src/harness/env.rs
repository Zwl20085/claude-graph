//! `${VAR}` substitution in profile env values, from the app's own process
//! environment, at spawn time. Expanded values are never stored or logged;
//! errors name the variable, never a value.

use crate::contract::{AppError, AppResult, HarnessProfile};
use crate::harness::profile::valid_env_key;

/// Environment changes for a process: `Some` sets, `None` removes.
pub type EnvChanges = Vec<(String, Option<String>)>;

/// `value` with every `${NAME}` replaced by `lookup(NAME)`. A `$` not
/// followed by `{NAME}` is kept as is. An unset or empty variable is an
/// error naming it (`Err(NAME)`).
pub fn expand(value: &str, lookup: &dyn Fn(&str) -> Option<String>) -> Result<String, String> {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(i) = rest.find("${") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 2..];
        match after.find('}') {
            Some(end) if valid_env_key(&after[..end]) => {
                let name = &after[..end];
                match lookup(name).filter(|v| !v.is_empty()) {
                    Some(v) => out.push_str(&v),
                    None => return Err(name.to_string()),
                }
                rest = &after[end + 1..];
            }
            _ => {
                out.push_str("${");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    Ok(out)
}

/// The message of a session that cannot start because `var` is missing.
pub fn missing_message(profile: &HarnessProfile, key: &str, var: &str) -> String {
    format!(
        "harness {}: environment variable {var} (used by {key}) is not set in the app's \
         environment; set it and restart Claude Graph",
        profile.id
    )
}

/// The profile's env, expanded, as `(key, Some(value))` pairs.
pub fn expand_profile(
    profile: &HarnessProfile,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> AppResult<EnvChanges> {
    profile
        .env
        .iter()
        .map(|(k, v)| match expand(v, lookup) {
            Ok(value) => Ok((k.clone(), Some(value))),
            Err(var) => Err(AppError::new(
                "invalid_input",
                missing_message(profile, k, &var),
            )),
        })
        .collect()
}

/// [`expand_profile`] against this process's environment.
pub fn process_env(profile: &HarnessProfile) -> AppResult<EnvChanges> {
    expand_profile(profile, &|name| std::env::var(name).ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::profile::builtins;

    fn look(name: &str) -> Option<String> {
        match name {
            "KEY" => Some("sk-1".into()),
            "EMPTY" => Some(String::new()),
            _ => None,
        }
    }

    #[test]
    fn substitutes_and_keeps_other_dollars() {
        assert_eq!(expand("a${KEY}b${KEY}", &look).unwrap(), "ask-1bsk-1");
        assert_eq!(
            expand("$KEY ${ } ${1A} ${KEY", &look).unwrap(),
            "$KEY ${ } ${1A} ${KEY"
        );
        assert_eq!(expand("plain", &look).unwrap(), "plain");
        assert_eq!(expand("${MISSING_VAR}", &look).unwrap_err(), "MISSING_VAR");
        assert_eq!(expand("x${EMPTY}", &look).unwrap_err(), "EMPTY");
    }

    #[test]
    fn profile_errors_name_the_variable_not_values() {
        let deepseek = &builtins()[1];
        let err = expand_profile(deepseek, &look).unwrap_err();
        assert!(err.message.contains("DEEPSEEK_API_KEY"), "{}", err.message);
        assert!(err.message.contains("ANTHROPIC_AUTH_TOKEN"));
        let ok =
            expand_profile(deepseek, &|n| (n == "DEEPSEEK_API_KEY").then(|| "k".into())).unwrap();
        assert!(ok.contains(&("ANTHROPIC_AUTH_TOKEN".into(), Some("k".into()))));
        assert_eq!(ok.len(), 7);
    }
}
