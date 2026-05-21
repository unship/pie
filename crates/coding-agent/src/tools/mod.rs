//! Built-in tools. Modeled on `packages/coding-agent/src/core/tools/` (the TS implementation):
//! same names, same parameter shapes, simpler bodies. Each tool implements
//! [`pie_agent_core::AgentTool`].

pub mod bash;
pub mod edit;
pub mod find;
pub mod git;
pub mod grep;
pub mod ls;
pub mod memory;
pub mod read;
pub mod truncate;
pub mod web_fetch;
pub mod write;

use std::sync::Arc;

use pie_agent_core::AgentTool;

/// Default tool set the coding agent ships with. Order matches the TS `createCodingTools()`
/// + the read-only quartet (`grep`/`find`/`ls`) the TS exposes via `createAllTools()`.
pub fn default_tools(memory_dir: std::path::PathBuf) -> Vec<Arc<dyn AgentTool>> {
    vec![
        Arc::new(read::ReadTool),
        Arc::new(write::WriteTool),
        Arc::new(edit::EditTool),
        Arc::new(bash::BashTool),
        Arc::new(ls::LsTool),
        Arc::new(grep::GrepTool),
        Arc::new(find::FindTool),
        Arc::new(web_fetch::WebFetchTool),
        Arc::new(git::GitTool),
        Arc::new(memory::MemoryTool::new(memory_dir)),
    ]
}
