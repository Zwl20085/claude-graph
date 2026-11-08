//! Unit tests of [`super`] (the live-session registry and its checks).

use super::*;
use crate::contract::SessionStatus;

fn info(sid: &str, nodes: &[&str]) -> TerminalSession {
    TerminalSession {
        sid: sid.into(),
        title: "T".into(),
        node_ids: nodes.iter().map(|n| n.to_string()).collect(),
        status: SessionStatus::Starting,
        pid: None,
        alive: false,
        current_node: None,
        harness: "claude".into(),
        pending_prompt: None,
    }
}

struct Quiet;
impl Emitter for Quiet {
    fn run_state(&self, _: &crate::contract::RunState) {}
    fn node_event(&self, _: &crate::contract::NodeEvent) {}
    fn assistant_update(&self, _: &crate::contract::AssistantState) {}
}

#[test]
fn sids_and_sizes_are_validated() {
    for ok in ["s1", "s42", "s99999"] {
        assert!(check_sid(ok).is_ok(), "{ok}");
    }
    for bad in ["", "s", "1", "S1", "s1a", "s-1", "s123456", "../s1"] {
        assert_eq!(check_sid(bad).unwrap_err().code, "invalid_input", "{bad}");
    }
    let t = Terminals::default();
    t.insert(SessionShared::new(
        "R1",
        Path::new("/g"),
        info("s1", &["a"]),
    ));
    assert_eq!(
        pty_resize(&t, "R1", "s1", 5, 40).unwrap_err().code,
        "invalid_input"
    );
    assert_eq!(
        pty_resize(&t, "R1", "s1", 80, 1000).unwrap_err().code,
        "invalid_input"
    );
    assert_eq!(
        pty_resize(&t, "R1", "s1", 80, 24).unwrap_err().code,
        "not_found"
    );
    let big = "x".repeat(WRITE_MAX + 1);
    assert_eq!(
        pty_write(&t, "R1", "s1", &big).unwrap_err().code,
        "invalid_input"
    );
    assert_eq!(
        pty_write(&t, "../x", "s1", "a").unwrap_err().code,
        "not_found"
    );
    assert_eq!(
        pty_write(&t, "R1", "s2", "a").unwrap_err().code,
        "not_found"
    );
    assert!(pty_write(&t, "R1", "s1", "").is_ok());
    assert_eq!(pty_replay(&t, "R1", "s1").unwrap().seq, 0);
}

#[test]
fn registry_finds_sessions_and_kills() {
    let t = Terminals::default();
    let s = SessionShared::new("R1", Path::new("/g"), info("s1", &["a", "b"]));
    let mut kill = s.kill_watch();
    t.insert(s.clone());
    assert!(t.has_run("R1"));
    assert_eq!(t.session_of_node("R1", "b").unwrap().info().sid, "s1");
    assert!(t.session_of_node("R1", "c").is_none());
    assert_eq!(t.live_count(), 0);
    s.task_started();
    assert_eq!(t.live_count(), 1);
    assert_eq!(t.kill_run("R1"), 1);
    assert!(*kill.borrow_and_update());
    s.task_ended();
    assert_eq!(t.live_count(), 0);
    let quiet: Arc<dyn Emitter> = Arc::new(Quiet);
    assert_eq!(
        t.type_message("R1", "a", "hi", quiet).unwrap_err().message,
        "Session closed; rerun to talk"
    );
    let w = s.window();
    assert_eq!(w.label, "term-R1-s1");
    assert_eq!(w.url, "index.html#/terminal/R1/s1?dir=%2Fg");
    assert_eq!(url_encode("C:/a b/ü"), "C%3A%2Fa%20b%2F%C3%BC");
    assert_eq!(list_sessions(&t, None, "R1").unwrap().len(), 1);
    assert_eq!(list_sessions(&t, None, "R2").unwrap_err().code, "not_found");
    assert_eq!(w.title, "T — session window");
}
