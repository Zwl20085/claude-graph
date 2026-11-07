//! The graph folder on disk (docs/ARCHITECTURE.md §2): graph.json, node briefs,
//! the recent-graphs list, and atomic file writes.
//!
//! Every whole-file write goes through [`write_atomic`] (temp file in the same
//! folder, then rename), so a crash never leaves a half-written graph.json or
//! run.json. Run folders live in [`crate::run_store`].

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::contract::{AppError, AppResult, Graph, Settings, GRAPH_VERSION};
use crate::graph_util::validate;
use crate::prompt::{display_dir, render_brief, upstream_placeholders};

pub const GRAPH_FILE: &str = "graph.json";
const RECENT_MAX: usize = 20;
const RENAME_RETRIES: u32 = 5;

const README: &str = "# Claude Graph folder\n\n\
This folder is a Claude Graph: a directed graph of Claude Code tasks.\n\n\
- `graph.json` — nodes, edges and settings (written by the app; edit it in the app).\n\
- `nodes/<id>/brief.md` — the brief each node's agent receives.\n\
- `nodes/<id>/deliverables/` — files each node produced.\n\
- `runs/<run-id>/` — one folder per run: `run.json`, and per node `prompt.md`, \
`events.jsonl`, `result.md`, `summary.json`, `messages.jsonl`.\n\
- `GRAPH_LOG.md` — one line per node per run.\n";

fn io_err(path: &Path, e: std::io::Error) -> AppError {
    let code = if e.kind() == std::io::ErrorKind::NotFound {
        "not_found"
    } else {
        "io"
    };
    AppError::new(code, format!("{}: {e}", path.display()))
}

/// Write `bytes` to `path` via a temp file in the same folder and a rename.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> AppResult<()> {
    let parent = path
        .parent()
        .ok_or_else(|| AppError::new("io", format!("{} has no parent", path.display())))?;
    fs::create_dir_all(parent).map_err(|e| io_err(parent, e))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy())
        .unwrap_or_default();
    let tmp = parent.join(format!(".{name}.{}.tmp", uuid::Uuid::new_v4().simple()));
    let result = (|| {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        rename_with_retry(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.map_err(|e| io_err(path, e))
}

/// Windows refuses a rename while another handle without delete sharing is
/// open (an editor, an antivirus scan); retry briefly before giving up.
fn rename_with_retry(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut attempt = 0;
    loop {
        match fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) if attempt < RENAME_RETRIES => {
                log::debug!("rename {} retry {attempt}: {e}", to.display());
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(20 * u64::from(attempt)));
            }
            Err(e) => return Err(e),
        }
    }
}

pub fn write_json<T: serde::Serialize>(path: &Path, value: &T) -> AppResult<()> {
    let mut text = serde_json::to_string_pretty(value)?;
    text.push('\n');
    write_atomic(path, text.as_bytes())
}

pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> AppResult<T> {
    let text = fs::read_to_string(path).map_err(|e| io_err(path, e))?;
    serde_json::from_str(&text)
        .map_err(|e| AppError::new("json", format!("{}: {e}", path.display())))
}

/// Append one chunk (non-atomic by nature; used for logs and jsonl files).
pub fn append(path: &Path, text: &str) -> AppResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| io_err(parent, e))?;
    }
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| io_err(path, e))?;
    f.write_all(text.as_bytes()).map_err(|e| io_err(path, e))
}

/// Read a text file; `None` if it does not exist.
pub fn read_optional(path: &Path) -> AppResult<Option<String>> {
    match fs::read_to_string(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io_err(path, e)),
    }
}

pub fn node_dir(dir: &Path, node_id: &str) -> PathBuf {
    dir.join("nodes").join(node_id)
}

pub fn deliverables_dir(dir: &Path, node_id: &str) -> PathBuf {
    node_dir(dir, node_id).join("deliverables")
}

/// Read and validate `<dir>/graph.json`.
pub fn read_graph(dir: &Path) -> AppResult<Graph> {
    let path = dir.join(GRAPH_FILE);
    if !path.is_file() {
        return Err(AppError::new(
            "not_found",
            format!("no {GRAPH_FILE} in {}", dir.display()),
        ));
    }
    let graph: Graph = read_json(&path).map_err(|e| match e.code.as_str() {
        "json" => AppError::new("invalid_graph", e.message),
        _ => e,
    })?;
    validate(&graph)?;
    Ok(graph)
}

/// Validate, write graph.json, every `nodes/<id>/brief.md` and deliverables folder.
pub fn save_graph(dir: &Path, graph: &Graph) -> AppResult<()> {
    validate(graph)?;
    write_json(&dir.join(GRAPH_FILE), graph)?;
    let shown = display_dir(dir);
    for node in &graph.nodes {
        let deliverables = deliverables_dir(dir, &node.id);
        fs::create_dir_all(&deliverables).map_err(|e| io_err(&deliverables, e))?;
        let brief = render_brief(
            &shown,
            graph,
            node,
            &upstream_placeholders(&shown, graph, node),
        );
        write_atomic(&node_dir(dir, &node.id).join("brief.md"), brief.as_bytes())?;
    }
    Ok(())
}

/// New empty graph in `dir` (created if missing). Refuses to overwrite a graph.
pub fn create_graph(dir: &Path, name: &str) -> AppResult<Graph> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::new("invalid_graph", "graph name is empty"));
    }
    if dir.join(GRAPH_FILE).exists() {
        return Err(AppError::new(
            "io",
            format!(
                "{} already holds a {GRAPH_FILE}; open it instead",
                dir.display()
            ),
        ));
    }
    fs::create_dir_all(dir).map_err(|e| io_err(dir, e))?;
    let graph = Graph {
        version: GRAPH_VERSION,
        name: name.to_string(),
        settings: Settings::default(),
        nodes: Vec::new(),
        edges: Vec::new(),
    };
    save_graph(dir, &graph)?;
    let readme = dir.join("README.md");
    if !readme.exists() {
        write_atomic(&readme, README.as_bytes())?;
    }
    Ok(graph)
}

/// `<data dir>/claude-graph/recent.json`.
pub fn default_recent_file() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("claude-graph").join("recent.json"))
}

/// Recent graph folders, most recent first, keeping only those that still hold a graph.
pub fn load_recent(file: &Path) -> Vec<String> {
    let list: Vec<String> = read_json(file).unwrap_or_default();
    list.into_iter()
        .filter(|d| Path::new(d).join(GRAPH_FILE).is_file())
        .collect()
}

/// Move `dir` to the front of the recent list (max 20 entries).
pub fn push_recent(file: &Path, dir: &Path) -> AppResult<()> {
    let entry = std::path::absolute(dir)
        .unwrap_or_else(|_| dir.to_path_buf())
        .display()
        .to_string();
    let previous: Vec<String> = read_json(file).unwrap_or_default();
    let list: Vec<String> = std::iter::once(entry.clone())
        .chain(previous.into_iter().filter(|d| d != &entry))
        .take(RECENT_MAX)
        .collect();
    write_json(file, &list)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example() -> Graph {
        serde_json::from_str(include_str!("../../examples/hello-pipeline/graph.json")).unwrap()
    }

    #[test]
    fn save_and_open_round_trip_with_briefs() {
        let tmp = tempfile::tempdir().unwrap();
        let g = example();
        save_graph(tmp.path(), &g).unwrap();
        assert_eq!(read_graph(tmp.path()).unwrap(), g);
        for id in ["survey", "design", "report"] {
            let brief = fs::read_to_string(node_dir(tmp.path(), id).join("brief.md")).unwrap();
            assert!(brief.contains(&format!("node id: {id})")));
            assert!(deliverables_dir(tmp.path(), id).is_dir());
        }
        let leftovers: Vec<_> = fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temp files left behind");
    }

    #[test]
    fn invalid_graph_is_not_written() {
        let tmp = tempfile::tempdir().unwrap();
        let mut g = example();
        g.edges.push(crate::contract::Edge {
            id: "back".into(),
            from: "report".into(),
            to: "survey".into(),
        });
        assert_eq!(
            save_graph(tmp.path(), &g).unwrap_err().code,
            "invalid_graph"
        );
        assert!(!tmp.path().join(GRAPH_FILE).exists());
        fs::write(tmp.path().join(GRAPH_FILE), "{ not json").unwrap();
        assert_eq!(read_graph(tmp.path()).unwrap_err().code, "invalid_graph");
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(read_graph(empty.path()).unwrap_err().code, "not_found");
    }

    #[test]
    fn create_graph_writes_readme_and_refuses_overwrite() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("new-graph");
        let g = create_graph(&dir, "  demo ").unwrap();
        assert_eq!(g.name, "demo");
        assert!(dir.join("README.md").is_file());
        assert_eq!(read_graph(&dir).unwrap(), g);
        assert_eq!(create_graph(&dir, "again").unwrap_err().code, "io");
        assert_eq!(
            create_graph(&tmp.path().join("x"), " ").unwrap_err().code,
            "invalid_graph"
        );
    }

    #[test]
    fn recent_list_dedupes_caps_and_filters() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("app").join("recent.json");
        let dirs: Vec<PathBuf> = (0..25)
            .map(|i| {
                let d = tmp.path().join(format!("g{i}"));
                create_graph(&d, "g").unwrap();
                d
            })
            .collect();
        for d in &dirs {
            push_recent(&file, d).unwrap();
        }
        push_recent(&file, &dirs[3]).unwrap();
        let list = load_recent(&file);
        assert_eq!(list.len(), RECENT_MAX);
        assert!(list[0].ends_with("g3"));
        assert!(list[1].ends_with("g24"));
        assert_eq!(list.iter().filter(|d| d.ends_with("g3")).count(), 1);
        fs::remove_file(dirs[24].join(GRAPH_FILE)).unwrap();
        assert!(!load_recent(&file).iter().any(|d| d.ends_with("g24")));
        assert!(load_recent(&tmp.path().join("missing.json")).is_empty());
    }
}
