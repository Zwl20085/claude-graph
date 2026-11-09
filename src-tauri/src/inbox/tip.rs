//! A parsed tip: its frontmatter as an ordered YAML mapping, the fields the
//! protocol reads, and the body (docs/AGENT-PROTOCOL.md sections 1 to 3).

use serde_yaml::{Mapping, Value};

use super::split::split;

pub const KIND_AGENT: &str = "agent";
pub const AGENT_KEY: &str = "agent";
/// The only protocol version this runner handles.
pub const PROTOCOL_V: u64 = 1;

/// `agent.state` values the runner writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentState {
    Running,
    Done,
    Question,
    Failed,
}

impl AgentState {
    pub fn as_str(self) -> &'static str {
        match self {
            AgentState::Running => "running",
            AgentState::Done => "done",
            AgentState::Question => "question",
            AgentState::Failed => "failed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "running" => Some(AgentState::Running),
            "done" => Some(AgentState::Done),
            "question" => Some(AgentState::Question),
            "failed" => Some(AgentState::Failed),
            _ => None,
        }
    }
}

/// Why a file could not be read as a tip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TipError {
    NotUtf8,
    /// No `---` frontmatter.
    NoFrontmatter,
    /// The frontmatter is not YAML, or not a mapping.
    BadYaml(String),
    /// `kind: agent` but `agent` is not a mapping.
    BadAgentMap,
}

impl TipError {
    pub fn describe(&self) -> String {
        match self {
            TipError::NotUtf8 => "not UTF-8 text".into(),
            TipError::NoFrontmatter => "no --- frontmatter".into(),
            TipError::BadYaml(e) => format!("frontmatter is not a YAML mapping: {e}"),
            TipError::BadAgentMap => "`agent` is not a mapping".into(),
        }
    }
}

/// `agent.v` as the protocol reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Version {
    Absent,
    /// The number 1 (`1` or `1.0`).
    One,
    /// A number above 1: the board shows no state, the runner skips it.
    Newer,
    /// A string, a number below 1, anything else: the runner skips it.
    Other,
}

/// The `agent` map as read (raw values kept for unknown sub-keys).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AgentMap {
    pub raw: Mapping,
}

fn text_of(v: Option<&Value>) -> Option<String> {
    match v? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

impl AgentMap {
    fn get(&self, key: &str) -> Option<&Value> {
        self.raw.get(key)
    }

    /// `agent.v` by the rule shared with the board: only a YAML number counts;
    /// a number above 1 (`2`, `2.0`) is a newer protocol; strings never are.
    pub fn version(&self) -> Version {
        match self.get("v") {
            None => Version::Absent,
            Some(Value::Number(n)) => match n.as_f64() {
                Some(x) if x == PROTOCOL_V as f64 => Version::One,
                Some(x) if x > PROTOCOL_V as f64 => Version::Newer,
                _ => Version::Other,
            },
            Some(_) => Version::Other,
        }
    }

    /// Raw `agent.state` text (`None` when absent).
    pub fn state_text(&self) -> Option<String> {
        text_of(self.get("state"))
    }

    pub fn hash(&self) -> Option<String> {
        text_of(self.get("hash"))
    }

    pub fn host(&self) -> Option<String> {
        text_of(self.get("host"))
    }

    pub fn run(&self) -> Option<String> {
        text_of(self.get("run"))
    }

    pub fn at(&self) -> Option<String> {
        text_of(self.get("at"))
    }
}

/// One tip file, parsed.
#[derive(Debug, Clone, PartialEq)]
pub struct Tip {
    /// File stem.
    pub id: String,
    pub frontmatter: Mapping,
    pub title: String,
    pub kind: Option<String>,
    pub status: Option<String>,
    /// `None` when there is no `agent` key.
    pub agent: Option<AgentMap>,
    pub body: String,
}

impl Tip {
    pub fn is_agent(&self) -> bool {
        self.kind.as_deref() == Some(KIND_AGENT)
    }
}

fn kind_of(map: &Mapping) -> Option<String> {
    text_of(map.get("kind"))
}

/// The frontmatter mapping of `text` (an empty frontmatter is an empty map).
pub fn frontmatter_of(text: &str) -> Result<Mapping, TipError> {
    let parts = split(text).ok_or(TipError::NoFrontmatter)?;
    parse_mapping(parts.frontmatter)
}

pub fn parse_mapping(yaml: &str) -> Result<Mapping, TipError> {
    if yaml.trim().is_empty() {
        return Ok(Mapping::new());
    }
    match serde_yaml::from_str::<Value>(yaml) {
        Ok(Value::Mapping(m)) => Ok(m),
        Ok(Value::Null) => Ok(Mapping::new()),
        Ok(_) => Err(TipError::BadYaml("not a mapping".into())),
        Err(e) => Err(TipError::BadYaml(e.to_string())),
    }
}

/// `kind` only: what the runner needs to decide whether to look further.
/// Non-agent tips are never read past their frontmatter.
pub fn peek_kind(bytes: &[u8]) -> Result<Option<String>, TipError> {
    let text = std::str::from_utf8(bytes).map_err(|_| TipError::NotUtf8)?;
    Ok(kind_of(&frontmatter_of(text)?))
}

/// Parse a whole tip (`id` is the file stem).
pub fn parse(id: &str, text: &str) -> Result<Tip, TipError> {
    let parts = split(text).ok_or(TipError::NoFrontmatter)?;
    let map = parse_mapping(parts.frontmatter)?;
    let agent = match map.get(AGENT_KEY) {
        None => None,
        Some(Value::Mapping(m)) => Some(AgentMap { raw: m.clone() }),
        Some(Value::Null) => Some(AgentMap::default()),
        Some(_) => return Err(TipError::BadAgentMap),
    };
    Ok(Tip {
        id: id.to_string(),
        title: text_of(map.get("title")).unwrap_or_default(),
        kind: kind_of(&map),
        status: text_of(map.get("status")),
        agent,
        body: parts.body.to_string(),
        frontmatter: map,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fields_and_agent_map() {
        let t = parse(
            "survey",
            "---\ntitle: Survey\nkind: agent\nstatus: open\nagent:\n  v: 1\n  state: done\n  hash: sha256:ab\n  extra: [1, 2]\n---\nFind papers.\n",
        )
        .unwrap();
        assert!(t.is_agent());
        assert_eq!(
            (t.title.as_str(), t.status.as_deref()),
            ("Survey", Some("open"))
        );
        let a = t.agent.unwrap();
        assert_eq!(a.version(), Version::One);
        assert_eq!(a.state_text().as_deref(), Some("done"));
        assert_eq!(a.hash().as_deref(), Some("sha256:ab"));
        assert_eq!(t.body, "Find papers.\n");
    }

    #[test]
    fn errors_are_reported() {
        assert_eq!(peek_kind(&[0xff, 0xfe]).unwrap_err(), TipError::NotUtf8);
        assert_eq!(parse("x", "no fm").unwrap_err(), TipError::NoFrontmatter);
        assert!(matches!(
            parse("x", "---\n: : :\n  - [\n---\n").unwrap_err(),
            TipError::BadYaml(_)
        ));
        assert!(matches!(
            parse("x", "---\n- a\n---\n").unwrap_err(),
            TipError::BadYaml(_)
        ));
        assert_eq!(
            parse("x", "---\nkind: agent\nagent: 3\n---\n").unwrap_err(),
            TipError::BadAgentMap
        );
        assert_eq!(
            peek_kind(b"---\nkind: task\n---\nbody").unwrap().as_deref(),
            Some("task")
        );
        assert_eq!(peek_kind(b"---\n---\n").unwrap(), None);
    }

    #[test]
    fn version_rule_is_shared_with_the_board() {
        let v = |yaml: &str| {
            parse(
                "x",
                &format!("---\nkind: agent\nagent:\n  v: {yaml}\n---\n"),
            )
            .unwrap()
            .agent
            .unwrap()
            .version()
        };
        assert_eq!(v("1"), Version::One);
        assert_eq!(v("1.0"), Version::One);
        assert_eq!(v("2"), Version::Newer);
        assert_eq!(v("2.0"), Version::Newer);
        assert_eq!(v("1.5"), Version::Newer);
        assert_eq!(v("'2'"), Version::Other);
        assert_eq!(v("'1'"), Version::Other);
        assert_eq!(v("two"), Version::Other);
        assert_eq!(v("0"), Version::Other);
        assert_eq!(v("~"), Version::Other);
        let none = parse("x", "---\nkind: agent\nagent:\n  state: done\n---\n").unwrap();
        assert_eq!(none.agent.unwrap().version(), Version::Absent);
    }
}
