use super::*;

fn custom() -> HarnessProfile {
    HarnessProfile {
        id: "my-cli".into(),
        label: " Mine ".into(),
        kind: HarnessKind::Custom,
        command: Some(" node ".into()),
        idle_marker: Some(" > ".into()),
        dialog_markers: vec!["  ".into(), "Trust?".into()],
        quiet_ms: Some(1000),
        builtin: true,
        ..builtin("x", "x", HarnessKind::Custom)
    }
}

#[test]
fn builtins_are_the_four_profiles() {
    let b = builtins();
    let ids: Vec<&str> = b.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, [CLAUDE_ID, DEEPSEEK_ID, CODEX_ID, DSH_ID]);
    assert_eq!(b[3].kind, HarnessKind::DeepseekHarness);
    assert_eq!(b[1].label, "Claude Code via DeepSeek API");
    assert_eq!(b[1].env["ANTHROPIC_AUTH_TOKEN"], "${DEEPSEEK_API_KEY}");
    assert_eq!(b[1].env.len(), 7);
    assert_eq!(b[2].args, ["--no-alt-screen"]);
    assert!(b.iter().all(|p| p.builtin && validated(p).is_ok()));
}

#[test]
fn ids_and_env_keys() {
    let long_ok = "a".repeat(32);
    let long_bad = "a".repeat(33);
    for ok in ["claude", "a", "0x", "my-cli-2", long_ok.as_str()] {
        assert!(valid_id(ok), "{ok}");
    }
    for bad in ["", "-a", "A", "a_b", "a b", long_bad.as_str(), "é"] {
        assert!(!valid_id(bad), "{bad}");
    }
    assert!(valid_env_key("_A1") && valid_env_key("ANTHROPIC_MODEL"));
    assert!(!valid_env_key("1A") && !valid_env_key("A-B") && !valid_env_key(""));
}

#[test]
fn validation_normalizes_and_rejects() {
    let p = validated(&custom()).unwrap();
    assert_eq!(p.label, "Mine");
    assert_eq!(p.command.as_deref(), Some("node"));
    assert_eq!(p.idle_marker.as_deref(), Some(">"));
    assert_eq!(p.dialog_markers, ["Trust?"]);
    assert!(!p.builtin);
    let no_cmd = HarnessProfile {
        command: Some("  ".into()),
        ..custom()
    };
    assert!(validated(&no_cmd).unwrap_err().message.contains("command"));
    let unc = HarnessProfile {
        command: Some(r"\\host\share\x.exe".into()),
        ..custom()
    };
    assert_eq!(validated(&unc).unwrap_err().code, "invalid_input");
    let mut env = custom();
    env.env.insert("BAD-KEY".into(), "v".into());
    assert!(validated(&env).is_err());
    let mut secret = custom();
    secret.env.insert("K".into(), "sk-secret\u{0}".into());
    let err = validated(&secret).unwrap_err().message;
    assert!(!err.contains("sk-secret"), "{err}");
    let quiet = HarnessProfile {
        quiet_ms: Some(10),
        ..custom()
    };
    assert!(validated(&quiet).is_err());
    let model = HarnessProfile {
        model: Some("gpt 5".into()),
        ..custom()
    };
    assert!(validated(&model).is_err());
    let ds = HarnessProfile {
        model: Some("deepseek-flash[1m]".into()),
        ..custom()
    };
    assert!(validated(&ds).is_ok());
}

#[test]
fn non_custom_kinds_drop_custom_fields() {
    let p = HarnessProfile {
        kind: HarnessKind::Codex,
        command: None,
        stop_mode: StopMode::ClaudeHooks,
        ..custom()
    };
    let v = validated(&p).unwrap();
    assert_eq!(v.idle_marker, None);
    assert!(v.dialog_markers.is_empty());
    assert_eq!((v.stop_mode, v.quiet_ms), (StopMode::Quiet, None));
}

#[test]
fn serde_defaults_load_minimal_profiles() {
    let p: HarnessProfile =
        serde_json::from_str(r#"{"id":"x","kind":"custom","stop_mode":"codex_notify"}"#).unwrap();
    assert_eq!(p.stop_mode, StopMode::CodexNotify);
    assert!(p.args.is_empty() && p.env.is_empty() && !p.builtin);
    let json = serde_json::to_value(&builtins()[0]).unwrap();
    assert_eq!(json["kind"], "claude");
    assert_eq!(json["stop_mode"], "quiet");
    assert!(json["quiet_ms"].is_null());
}
