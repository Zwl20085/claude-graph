//! A registered run or assistant turn ([`super::Approvals::register`]): makes
//! the inline `--mcp-config` of its turns and, when dropped, denies what it
//! left pending and forgets its "allow for this node" answers.

use std::net::SocketAddr;
use std::path::PathBuf;

use serde_json::json;

use super::hub::Approvals;
use super::{ENV_ADDR, ENV_NODE_ID, ENV_REG, ENV_RUN_ID, ENV_TOKEN, FLAG_MCP_APPROVE, SERVER_NAME};

/// A registered run or assistant turn; makes the CLI flags of its turns.
pub struct Registration {
    pub(super) approvals: Approvals,
    pub(super) id: String,
    pub(super) run_id: String,
    pub(super) addr: SocketAddr,
    pub(super) exe: PathBuf,
}

impl Registration {
    /// The inline `--mcp-config` JSON for one node's turn. Holds the token:
    /// never log it (see [`super::args::redact`]).
    pub fn mcp_config(&self, node_id: &str) -> String {
        let mut env = serde_json::Map::new();
        env.insert(ENV_ADDR.into(), json!(self.addr.to_string()));
        env.insert(ENV_TOKEN.into(), json!(self.approvals.inner.token));
        env.insert(ENV_RUN_ID.into(), json!(self.run_id));
        env.insert(ENV_NODE_ID.into(), json!(node_id));
        env.insert(ENV_REG.into(), json!(self.id));
        let server = json!({
            "command": self.exe.display().to_string(),
            "args": [FLAG_MCP_APPROVE],
            "env": env,
        });
        json!({ "mcpServers": { SERVER_NAME: server } }).to_string()
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        self.approvals.close(&self.id);
    }
}
