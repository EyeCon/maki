//! Hook for rendering native tool outputs into styled snapshot lines.
//!
//! The renderer lives in maki-ui (it owns themes and code rendering), but the
//! batch plugin needs rich child output while composing its parent buffer in
//! Lua. The UI registers the renderer once at startup; headless hosts simply
//! leave it unset and plugins fall back to plain text.

use std::sync::OnceLock;

use maki_agent::{SnapshotLine, ToolInput, ToolOutput};

pub struct ToolRenderRequest<'a> {
    pub input: Option<&'a ToolInput>,
    pub output: &'a ToolOutput,
    pub output_limit: usize,
    pub expanded: bool,
    pub highlight: bool,
}

pub struct RenderedTool {
    pub lines: Vec<SnapshotLine>,
    pub truncated: bool,
    /// `false` when the output itself is plain text (e.g. code_execution's
    /// stdout): the caller must still display the text body separately.
    pub covers_output: bool,
    pub annotation: Option<String>,
}

pub type ToolRenderer = Box<dyn Fn(&ToolRenderRequest) -> Option<RenderedTool> + Send + Sync>;

static RENDERER: OnceLock<ToolRenderer> = OnceLock::new();

pub fn set_tool_renderer(renderer: ToolRenderer) {
    let _ = RENDERER.set(renderer);
}

pub(crate) fn tool_renderer() -> Option<&'static ToolRenderer> {
    RENDERER.get()
}
