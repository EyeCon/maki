use std::fmt::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use flume::Sender;
use maki_providers::{AgentError, ContentBlock, Message, Role, StopReason, TokenUsage};
use maki_tool_macro::{ArgEnum, Args};
use serde::{Deserialize, Serialize};

pub const NO_FILES_FOUND: &str = "No files found";
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrepFileEntry {
    pub path: String,
    pub groups: Vec<GrepMatchGroup>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrepMatchGroup {
    pub lines: Vec<GrepLine>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrepLine {
    pub line_nr: usize,
    pub text: String,
    pub is_match: bool,
}

impl GrepLine {
    pub fn matched(line_nr: usize, text: impl Into<String>) -> Self {
        Self {
            line_nr,
            text: text.into(),
            is_match: true,
        }
    }

    pub fn context(line_nr: usize, text: impl Into<String>) -> Self {
        Self {
            line_nr,
            text: text.into(),
            is_match: false,
        }
    }
}

impl GrepMatchGroup {
    pub fn single(line_nr: usize, text: impl Into<String>) -> Self {
        Self {
            lines: vec![GrepLine::matched(line_nr, text)],
        }
    }

    pub fn match_count(&self) -> usize {
        self.lines.iter().filter(|l| l.is_match).count()
    }
}

impl GrepFileEntry {
    pub fn match_count(&self) -> usize {
        self.groups.iter().map(|g| g.match_count()).sum()
    }
}

#[derive(Args, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TodoItem {
    #[param(description = "Task description")]
    pub content: String,
    pub status: TodoStatus,
    pub priority: TodoPriority,
}

#[derive(ArgEnum, Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
    Cancelled,
}

impl TodoStatus {
    pub fn marker(self) -> &'static str {
        match self {
            Self::Completed => "[✓]",
            Self::InProgress => "[•]",
            Self::Pending => "[ ]",
            Self::Cancelled => "[x]",
        }
    }
}

#[derive(ArgEnum, Debug, Clone, Copy, Serialize, Deserialize, PartialEq, strum::Display)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum TodoPriority {
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ToolInput {
    Code { language: String, code: String },
    Script { language: String, code: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstructionBlock {
    pub path: String,
    pub content: String,
}

fn append_instructions(out: &mut String, blocks: &[InstructionBlock]) {
    for block in blocks {
        out.push_str("\n\n---\nInstructions from: ");
        out.push_str(&block.path);
        out.push('\n');
        out.push_str(&block.content);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ToolOutput {
    Plain(String),
    Markdown(String),
    ReadCode {
        path: String,
        start_line: usize,
        lines: Vec<String>,
        #[serde(default)]
        total_lines: usize,
        #[serde(default)]
        instructions: Option<Vec<InstructionBlock>>,
    },
    ReadDir {
        text: String,
        #[serde(default)]
        instructions: Option<Vec<InstructionBlock>>,
    },
    Diff {
        path: String,
        before: String,
        after: String,
        summary: String,
    },
    TodoList(Vec<TodoItem>),
    WriteCode {
        path: String,
        byte_count: usize,
        lines: Vec<String>,
    },

    GrepResult {
        entries: Vec<GrepFileEntry>,
    },
    Instructions {
        blocks: Vec<InstructionBlock>,
    },
}

/// Saturating arithmetic so callers can't overflow with any combination of inputs.
fn lines_remaining_after(total: usize, start_line: usize, shown: usize) -> usize {
    let end = start_line.saturating_add(shown).saturating_sub(1);
    total.saturating_sub(end)
}

impl ToolOutput {
    pub fn written_path(&self) -> Option<&str> {
        match self {
            Self::WriteCode { path, .. } | Self::Diff { path, .. } => Some(path),
            _ => None,
        }
    }

    pub fn instructions(&self) -> Option<&[InstructionBlock]> {
        match self {
            Self::ReadCode { instructions, .. } | Self::ReadDir { instructions, .. } => {
                instructions.as_deref()
            }
            _ => None,
        }
    }

    pub fn owned_instructions(&self) -> Option<Vec<InstructionBlock>> {
        self.instructions()
            .filter(|b| !b.is_empty())
            .map(|b| b.to_vec())
    }

    pub fn is_markdown(&self) -> bool {
        matches!(self, Self::Markdown(_))
    }

    pub fn structured_display_text(&self) -> Option<String> {
        match self {
            Self::Diff { .. }
            | Self::ReadCode { .. }
            | Self::ReadDir { .. }
            | Self::WriteCode { .. }
            | Self::GrepResult { .. }
            | Self::TodoList(_) => Some(self.as_display_text()),
            _ => None,
        }
    }

    pub fn is_empty_result(&self) -> bool {
        match self {
            Self::GrepResult { entries } => entries.is_empty(),
            Self::ReadDir { text, .. } => text.is_empty(),
            Self::Plain(text) | Self::Markdown(text) => text.is_empty(),

            _ => false,
        }
    }

    pub fn as_text(&self) -> String {
        match self {
            Self::Diff { summary, .. } => summary.clone(),
            Self::TodoList(_) => "ok".into(),
            Self::ReadCode { instructions, .. } | Self::ReadDir { instructions, .. } => {
                let mut out = self.as_display_text();
                if let Some(blocks) = instructions {
                    append_instructions(&mut out, blocks);
                }
                out
            }
            _ => self.as_display_text(),
        }
    }

    pub fn as_display_text(&self) -> String {
        match self {
            Self::Plain(s) | Self::Markdown(s) => s.clone(),

            Self::ReadDir { text, .. } => text.clone(),
            Self::ReadCode {
                start_line,
                lines,
                total_lines,
                ..
            } => {
                let mut out: String = lines
                    .iter()
                    .enumerate()
                    .map(|(i, line)| format!("{}: {line}", start_line + i))
                    .collect::<Vec<_>>()
                    .join("\n");
                let remaining = lines_remaining_after(*total_lines, *start_line, lines.len());
                if remaining > 0 {
                    out.push_str(&format!(
                        "\n\n...\n\nTruncated lines: {}-{}. Use offset={} to read further.",
                        start_line + lines.len(),
                        total_lines,
                        start_line + lines.len(),
                    ));
                }
                out
            }
            Self::Diff {
                path,
                before,
                after,
                summary,
            } => crate::diff::unified_text(
                before,
                after,
                summary,
                &crate::tools::relative_path(path),
            ),
            Self::TodoList(items) => {
                if items.is_empty() {
                    return "No todos.".into();
                }
                items
                    .iter()
                    .map(|t| format!("{} ({}) {}", t.status.marker(), t.priority, t.content))
                    .collect::<Vec<_>>()
                    .join("\n")
            }
            Self::WriteCode {
                path, byte_count, ..
            } => {
                let display = crate::tools::relative_path(path);
                format!("wrote {byte_count} bytes to {display}")
            }
            Self::GrepResult { entries } => {
                let mut out = String::new();
                for (i, entry) in entries.iter().enumerate() {
                    if i > 0 {
                        out.push('\n');
                    }
                    out.push_str(&entry.path);
                    out.push(':');
                    let has_context = entry.groups.iter().any(|g| g.lines.len() > 1);
                    for (gi, group) in entry.groups.iter().enumerate() {
                        if gi > 0 && has_context {
                            out.push_str("\n  --");
                        }
                        for line in &group.lines {
                            let sep = if line.is_match { ":" } else { " " };
                            let _ = write!(out, "\n  {}{sep} {}", line.line_nr, line.text);
                        }
                    }
                }
                out
            }
            Self::Instructions { blocks } => {
                let mut out = String::new();
                append_instructions(&mut out, blocks);
                out
            }
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolStartEvent {
    pub id: String,
    pub tool: Arc<str>,
    pub summary: String,
    pub render_header: Option<BufferSnapshot>,
    pub annotation: Option<String>,
    pub input: Option<ToolInput>,
    pub raw_input: Option<serde_json::Value>,
    pub output: Option<ToolOutput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub full_view: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolDoneEvent {
    pub id: String,
    pub tool: Arc<str>,
    pub output: ToolOutput,
    pub is_error: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
}

const UNKNOWN_TOOL: &str = "unknown";

impl ToolDoneEvent {
    pub fn error(id: String, message: impl Into<String>) -> Self {
        Self {
            id,
            tool: Arc::from(UNKNOWN_TOOL),
            output: ToolOutput::Plain(message.into()),
            is_error: true,
            parent_id: None,
        }
    }

    pub fn written_path(&self) -> Option<&str> {
        if self.is_error {
            return None;
        }
        self.output.written_path()
    }

    pub fn wrote_to(&self, plan_path: &Path) -> bool {
        self.written_path()
            .is_some_and(|wp| Path::new(wp) == plan_path)
    }
}

pub fn tool_results(results: Vec<ToolDoneEvent>) -> Message {
    Message {
        role: Role::User,
        content: results
            .into_iter()
            .map(|r| ContentBlock::ToolResult {
                tool_use_id: r.id,
                content: r.output.as_text(),
                is_error: r.is_error,
            })
            .collect(),
        ..Default::default()
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    TextDelta {
        text: String,
    },
    ThinkingDelta {
        text: String,
    },
    ToolPending {
        id: String,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_id: Option<String>,
    },
    ToolStart(Box<ToolStartEvent>),
    /// `content` is the **full accumulated output** so far, not a delta.
    /// Producers must accumulate into a growing buffer and send the whole thing each flush.
    ToolOutput {
        id: String,
        content: String,
    },
    ToolDone(Box<ToolDoneEvent>),
    TurnComplete(Box<TurnCompleteEvent>),
    ToolResultsSubmitted {
        message: Box<Message>,
    },
    QueueItemConsumed {
        text: String,
        image_count: usize,
    },
    Done {
        usage: TokenUsage,
        num_turns: u32,
        stop_reason: Option<StopReason>,
    },
    AutoCompacting,
    Retry {
        attempt: u32,
        message: String,
        delay_ms: u64,
    },
    Error {
        message: String,
    },
    PermissionRequest {
        id: String,
        tool: String,
        scopes: Vec<String>,
    },
    AuthRequired,
    SubagentHistory {
        tool_use_id: String,
        messages: Vec<Message>,
    },
    ToolSnapshot {
        id: String,
        snapshot: BufferSnapshot,
        /// Which theme baked these colors. `None` for live output.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        theme_gen: Option<u64>,
    },
    ToolHeaderSnapshot {
        id: String,
        snapshot: BufferSnapshot,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        theme_gen: Option<u64>,
    },
    LiveToolBuf {
        id: String,
        body: Arc<SharedBuf>,
    },
}

#[derive(Clone)]
enum BufEntry {
    Line(SnapshotLine),
    Embed {
        buf: Arc<SharedBuf>,
        indent: String,
        first_indent: Option<String>,
    },
}

#[derive(Default)]
struct BufInner {
    entries: Vec<BufEntry>,
    /// Flattened lines keyed by the aggregate version they were built from.
    cache: Option<(u64, Arc<Vec<SnapshotLine>>)>,
}

/// Buffer for streaming tool output to the UI. Holds owned lines and
/// live embeds of other buffers (by reference): an embedded buffer's
/// mutations are visible through every buffer that embeds it, so a child's
/// content has a single source of truth no matter where it is rendered.
///
/// Change detection is a monotonic [`version`](Self::version) aggregated
/// over embeds. Readers keep their own last-seen version cursor, so any
/// number of readers can poll independently.
pub struct SharedBuf {
    inner: Mutex<BufInner>,
    version: AtomicU64,
}

impl SharedBuf {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(BufInner::default()),
            version: AtomicU64::new(0),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BufInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn bump(&self, by: u64) {
        self.version.fetch_add(by, Ordering::Release);
    }

    /// Monotonic aggregate version: own mutations plus every embedded
    /// buffer's. Changes whenever flattened content may have changed.
    pub fn version(&self) -> u64 {
        let mut v = self.version.load(Ordering::Acquire);
        let inner = self.lock();
        for entry in &inner.entries {
            if let BufEntry::Embed { buf, .. } = entry {
                v = v.wrapping_add(buf.version());
            }
        }
        v
    }

    pub fn append(&self, line: SnapshotLine) {
        self.lock().push(BufEntry::Line(line));
        self.bump(1);
    }

    pub fn extend(&self, lines: &[SnapshotLine]) {
        let mut inner = self.lock();
        for line in lines {
            inner.push(BufEntry::Line(line.clone()));
        }
        drop(inner);
        self.bump(1);
    }

    pub fn set_lines(&self, lines: Vec<SnapshotLine>) {
        let mut inner = self.lock();
        // Absorb the removed embeds' versions so the aggregate stays
        // monotonic: readers must never see a version they already saw.
        let absorbed: u64 = inner
            .entries
            .iter()
            .map(|e| match e {
                BufEntry::Embed { buf, .. } => buf.version(),
                BufEntry::Line(_) => 0,
            })
            .fold(0, u64::wrapping_add);
        inner.entries = lines.into_iter().map(BufEntry::Line).collect();
        inner.cache = None;
        drop(inner);
        self.bump(absorbed.wrapping_add(1));
    }

    /// Embeds `other` by reference: its current and future content shows up
    /// in this buffer's flattened lines, each line prefixed with `indent`
    /// (`first_indent` for the embed's first line, when given).
    ///
    /// Returns `false` when embedding would create a cycle.
    pub fn embed(
        self: &Arc<Self>,
        other: &Arc<SharedBuf>,
        indent: String,
        first_indent: Option<String>,
    ) -> bool {
        if Arc::ptr_eq(self, other) || other.reaches(self) {
            return false;
        }
        self.lock().push(BufEntry::Embed {
            buf: Arc::clone(other),
            indent,
            first_indent,
        });
        self.bump(1);
        true
    }

    fn reaches(&self, target: &Arc<SharedBuf>) -> bool {
        let inner = self.lock();
        inner.entries.iter().any(|e| match e {
            BufEntry::Embed { buf, .. } => {
                std::ptr::eq(Arc::as_ptr(buf), Arc::as_ptr(target)) || buf.reaches(target)
            }
            BufEntry::Line(_) => false,
        })
    }

    /// Atomically replaces all entries with those of `other`, absorbing
    /// removed embed versions for monotonicity. Returns `false` when the
    /// swap would create a cycle (any embed in `other` reaches `self`).
    pub fn assign(self: &Arc<Self>, other: &SharedBuf) -> bool {
        let new_entries = {
            let src = other.lock();
            src.entries.clone()
        };
        for entry in &new_entries {
            if let BufEntry::Embed { buf, .. } = entry
                && (Arc::ptr_eq(self, buf) || buf.reaches(self))
            {
                return false;
            }
        }
        let mut inner = self.lock();
        let absorbed: u64 = inner
            .entries
            .iter()
            .map(|e| match e {
                BufEntry::Embed { buf, .. } => buf.version(),
                BufEntry::Line(_) => 0,
            })
            .fold(0, u64::wrapping_add);
        inner.entries = new_entries;
        inner.cache = None;
        drop(inner);
        self.bump(absorbed.wrapping_add(1));
        true
    }

    /// Flattened line count, embeds included.
    pub fn len(&self) -> usize {
        let inner = self.lock();
        inner
            .entries
            .iter()
            .map(|e| match e {
                BufEntry::Line(_) => 1,
                BufEntry::Embed { buf, .. } => buf.len(),
            })
            .sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Flattened lines with embed indents applied. Cached per aggregate
    /// version, so repeated reads of an unchanged buffer are O(1).
    pub fn read(&self) -> Arc<Vec<SnapshotLine>> {
        let version = self.version();
        let mut inner = self.lock();
        if let Some((v, lines)) = &inner.cache
            && *v == version
        {
            return Arc::clone(lines);
        }
        drop(inner);
        let mut out = Vec::new();
        self.flatten_into(&mut out, "", "");
        let lines = Arc::new(out);
        inner = self.lock();
        inner.cache = Some((version, Arc::clone(&lines)));
        lines
    }

    fn flatten_into(&self, out: &mut Vec<SnapshotLine>, first_prefix: &str, rest_prefix: &str) {
        let inner = self.lock();
        let mut first = true;
        for entry in &inner.entries {
            let prefix = if first { first_prefix } else { rest_prefix };
            match entry {
                BufEntry::Line(line) => {
                    out.push(prefix_line(line, prefix));
                    first = false;
                }
                BufEntry::Embed {
                    buf,
                    indent,
                    first_indent,
                } => {
                    let fp = format!("{prefix}{}", first_indent.as_deref().unwrap_or(indent));
                    let rp = format!("{rest_prefix}{indent}");
                    let before = out.len();
                    buf.flatten_into(out, &fp, &rp);
                    if out.len() > before {
                        first = false;
                    }
                }
            }
        }
    }

    /// Resolves a flattened row to the chain of buffers containing it, from
    /// this buffer (row as given) down to the innermost embed (row local to
    /// it). Used to route clicks to the handler closest to the content.
    pub fn locate(self: &Arc<Self>, row: usize) -> Vec<(Arc<SharedBuf>, usize)> {
        let mut path = vec![(Arc::clone(self), row)];
        let (mut cur, mut row) = (Arc::clone(self), row);
        while let Some((child, local)) = cur.child_at(row) {
            path.push((Arc::clone(&child), local));
            (cur, row) = (child, local);
        }
        path
    }

    fn child_at(&self, row: usize) -> Option<(Arc<SharedBuf>, usize)> {
        let inner = self.lock();
        let mut offset = 0;
        for entry in &inner.entries {
            if row < offset {
                return None;
            }
            offset += match entry {
                BufEntry::Line(_) => 1,
                BufEntry::Embed { buf, .. } => {
                    let count = buf.len();
                    if row < offset + count {
                        return Some((Arc::clone(buf), row - offset));
                    }
                    count
                }
            };
        }
        None
    }

    /// Forces readers to see a change even if nothing was written. Used to
    /// flush click-handler results to pollers when the handler was a no-op.
    pub fn mark_dirty(&self) {
        self.bump(1);
    }

    pub fn take(&self) -> BufferSnapshot {
        BufferSnapshot::from_arc(self.read())
    }
}

impl BufInner {
    fn push(&mut self, entry: BufEntry) {
        self.entries.push(entry);
        self.cache = None;
    }
}

fn prefix_line(line: &SnapshotLine, prefix: &str) -> SnapshotLine {
    if prefix.is_empty() {
        return line.clone();
    }
    let mut spans = Vec::with_capacity(line.spans.len() + 1);
    spans.push(SnapshotSpan {
        text: prefix.to_owned(),
        style: SpanStyle::default(),
    });
    spans.extend_from_slice(&line.spans);
    SnapshotLine { spans }
}

impl Default for SharedBuf {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for SharedBuf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedBuf").finish_non_exhaustive()
    }
}

impl Serialize for SharedBuf {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_unit()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BufferSnapshot {
    pub lines: Arc<Vec<SnapshotLine>>,
}

impl BufferSnapshot {
    pub fn from_arc(lines: Arc<Vec<SnapshotLine>>) -> Self {
        Self { lines }
    }

    pub fn first_line_text(&self) -> String {
        self.lines
            .first()
            .map(|l| l.spans.iter().map(|s| s.text.as_str()).collect())
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SnapshotLine {
    pub spans: Vec<SnapshotSpan>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SnapshotSpan {
    pub text: String,
    pub style: SpanStyle,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub enum SpanStyle {
    #[default]
    Default,
    Named(String),
    Inline(InlineStyle),
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct InlineStyle {
    pub fg: Option<(u8, u8, u8)>,
    pub bg: Option<(u8, u8, u8)>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub dim: bool,
    pub strikethrough: bool,
    pub reversed: bool,
}

#[derive(Debug, Serialize)]
pub struct TurnCompleteEvent {
    pub message: Message,
    pub usage: TokenUsage,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_size: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SubagentInfo {
    pub parent_tool_use_id: String,
    #[serde(rename = "parent_name")]
    pub name: String,
    #[serde(rename = "parent_prompt", skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(rename = "parent_model", skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip)]
    pub answer_tx: Option<flume::Sender<String>>,
}

#[derive(Debug, Clone)]
pub struct EventSender {
    tx: Sender<Envelope>,
    run_id: u64,
}

impl EventSender {
    pub fn new(tx: Sender<Envelope>, run_id: u64) -> Self {
        Self { tx, run_id }
    }

    pub fn send(&self, event: impl Into<AgentEvent>) -> Result<(), AgentError> {
        self.tx
            .try_send(Envelope {
                event: event.into(),
                subagent: None,
                run_id: self.run_id,
            })
            .map_err(|_| AgentError::Channel)
    }

    pub fn send_envelope(&self, envelope: Envelope) -> Result<(), AgentError> {
        self.tx.try_send(envelope).map_err(|_| AgentError::Channel)
    }

    pub fn try_send(&self, event: impl Into<AgentEvent>) {
        let _ = self.tx.try_send(Envelope {
            event: event.into(),
            subagent: None,
            run_id: self.run_id,
        });
    }

    pub fn run_id(&self) -> u64 {
        self.run_id
    }

    pub fn raw_tx(&self) -> &Sender<Envelope> {
        &self.tx
    }
}

#[derive(Debug, Serialize)]
pub struct Envelope {
    #[serde(flatten)]
    pub event: AgentEvent,
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub subagent: Option<SubagentInfo>,
    pub run_id: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;

    #[test]
    fn as_display_text_diff_renders_unified_text() {
        let output = ToolOutput::Diff {
            path: "src/main.rs".into(),
            before: "keep\nold\n".into(),
            after: "keep\nnew\n".into(),
            summary: "Updated value".into(),
        };
        let display = output.as_display_text();
        assert!(display.starts_with("Updated value"));
        assert!(display.contains("--- src/main.rs"));
        assert!(display.contains("+++ src/main.rs"));
        assert!(display.contains("  keep"));
        assert!(display.contains("- old"));
        assert!(display.contains("+ new"));
        assert_eq!(output.as_text(), "Updated value");
    }

    #[test]
    fn as_display_text_todolist_formats_all_statuses() {
        let output = ToolOutput::TodoList(vec![
            TodoItem {
                content: "done".into(),
                status: TodoStatus::Completed,
                priority: TodoPriority::High,
            },
            TodoItem {
                content: "wip".into(),
                status: TodoStatus::InProgress,
                priority: TodoPriority::Medium,
            },
            TodoItem {
                content: "todo".into(),
                status: TodoStatus::Pending,
                priority: TodoPriority::Low,
            },
            TodoItem {
                content: "nope".into(),
                status: TodoStatus::Cancelled,
                priority: TodoPriority::Low,
            },
        ]);
        let display = output.as_display_text();
        assert!(display.contains("[✓] (high) done"));
        assert!(display.contains("[•] (medium) wip"));
        assert!(display.contains("[ ] (low) todo"));
        assert!(display.contains("[x] (low) nope"));
        assert_eq!(output.as_text(), "ok");
    }

    #[test]
    fn as_text_grep_result_multi_file() {
        let output = ToolOutput::GrepResult {
            entries: vec![
                GrepFileEntry {
                    path: "src/a.rs".into(),
                    groups: vec![
                        GrepMatchGroup::single(3, "fn foo()"),
                        GrepMatchGroup::single(10, "fn bar()"),
                    ],
                },
                GrepFileEntry {
                    path: "src/b.rs".into(),
                    groups: vec![GrepMatchGroup::single(1, "use crate")],
                },
            ],
        };
        let text = output.as_text();
        assert!(text.contains("src/a.rs"));
        assert!(text.contains("3: fn foo()"));
        assert!(text.contains("10: fn bar()"));
        assert!(text.contains("src/b.rs"));
        assert!(text.contains("1: use crate"));
    }

    #[test]
    fn as_text_grep_result_with_context() {
        let output = ToolOutput::GrepResult {
            entries: vec![GrepFileEntry {
                path: "src/a.rs".into(),
                groups: vec![
                    GrepMatchGroup {
                        lines: vec![
                            GrepLine::context(2, "let x = 1;"),
                            GrepLine::matched(3, "fn foo()"),
                            GrepLine::context(4, "let y = 2;"),
                        ],
                    },
                    GrepMatchGroup::single(20, "fn bar()"),
                ],
            }],
        };
        let text = output.as_text();
        assert!(text.contains("2  let x = 1;"), "context before: {text}");
        assert!(text.contains("3: fn foo()"), "match line: {text}");
        assert!(text.contains("4  let y = 2;"), "context after: {text}");
        assert!(text.contains("--"), "group separator: {text}");
        assert!(text.contains("20: fn bar()"), "second group: {text}");
    }

    #[test_case(ToolOutput::WriteCode { path: "src/lib.rs".into(), byte_count: 10, lines: vec![] }, Some("src/lib.rs") ; "write_code")]
    #[test_case(ToolOutput::Diff { path: "src/lib.rs".into(), before: String::new(), after: String::new(), summary: String::new() }, Some("src/lib.rs") ; "diff")]
    #[test_case(ToolOutput::Plain("ok".into()), None ; "non_write_variant")]
    fn output_written_path(output: ToolOutput, expected: Option<&str>) {
        assert_eq!(output.written_path(), expected);
    }

    #[test]
    fn tool_results_builds_message_with_tool_result_blocks() {
        let msg = tool_results(vec![
            ToolDoneEvent {
                id: "t1".into(),
                tool: Arc::from("bash"),
                output: ToolOutput::Plain("ok".into()),
                is_error: false,
                parent_id: None,
            },
            ToolDoneEvent {
                id: "t2".into(),
                tool: Arc::from("read"),
                output: ToolOutput::Plain("fail".into()),
                is_error: true,
                parent_id: None,
            },
        ]);
        assert!(matches!(msg.role, Role::User));
        assert_eq!(msg.content.len(), 2);
        assert!(
            matches!(&msg.content[0], ContentBlock::ToolResult { tool_use_id, is_error, .. } if tool_use_id == "t1" && !is_error)
        );
        assert!(
            matches!(&msg.content[1], ContentBlock::ToolResult { tool_use_id, is_error, .. } if tool_use_id == "t2" && *is_error)
        );
    }

    #[test_case(
        10,
        vec!["fn foo()".into(), "fn bar()".into()],
        Some(vec![InstructionBlock { path: "AGENTS.md".into(), content: "do stuff".into() }]),
        "10: fn foo()\n11: fn bar()\n\n...\n\nTruncated lines: 12-100. Use offset=12 to read further."
        ; "with_instructions"
    )]
    #[test_case(
        1,
        vec!["line1".into()],
        None,
        "1: line1\n\n...\n\nTruncated lines: 2-100. Use offset=2 to read further."
        ; "without_instructions"
    )]
    fn read_code_display_text(
        start_line: usize,
        lines: Vec<String>,
        instructions: Option<Vec<InstructionBlock>>,
        expected: &str,
    ) {
        let output = ToolOutput::ReadCode {
            path: "a.rs".into(),
            start_line,
            lines,
            total_lines: 100,
            instructions,
        };
        assert_eq!(output.as_display_text(), expected);
    }

    #[test]
    fn read_code_as_text_includes_instructions() {
        let output = ToolOutput::ReadCode {
            path: "a.rs".into(),
            start_line: 1,
            lines: vec!["fn main()".into()],
            total_lines: 1,
            instructions: Some(vec![InstructionBlock {
                path: "AGENTS.md".into(),
                content: "do stuff".into(),
            }]),
        };
        let text = output.as_text();
        assert!(text.contains("1: fn main()"));
        assert!(text.contains("Instructions from: AGENTS.md"));
        assert!(text.contains("do stuff"));
    }

    #[test]
    fn wrote_to_checks_path_and_error_flag() {
        let ok_event = ToolDoneEvent {
            id: "id".into(),
            tool: Arc::from("write"),
            output: ToolOutput::WriteCode {
                path: "/plans/slug.md".into(),
                byte_count: 10,
                lines: vec![],
            },
            is_error: false,
            parent_id: None,
        };
        assert!(ok_event.wrote_to(Path::new("/plans/slug.md")));
        assert!(!ok_event.wrote_to(Path::new("/plans/other.md")));

        let err_event = ToolDoneEvent {
            is_error: true,
            ..ok_event
        };
        assert!(!err_event.wrote_to(Path::new("/plans/slug.md")));
    }

    #[test]
    fn read_code_backward_compat_deserialization() {
        let json = r#"{"ReadCode":{"path":"a.rs","start_line":1,"lines":["x"]}}"#;
        let output: ToolOutput = serde_json::from_str(json).unwrap();
        match output {
            ToolOutput::ReadCode {
                total_lines,
                instructions,
                ..
            } => {
                assert_eq!(total_lines, 0);
                assert!(instructions.is_none());
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test_case(100, 10, 2, 89 ; "middle_of_file")]
    #[test_case(100, 1, 1, 99  ; "first_line_only")]
    #[test_case(5, 1, 5, 0     ; "all_lines_shown")]
    #[test_case(5, 1, 2, 3     ; "partial_from_start")]
    #[test_case(5, 3, 3, 0     ; "partial_to_end")]
    #[test_case(0, 1, 1, 0     ; "backward_compat_total_zero")]
    #[test_case(0, 1, 0, 0     ; "empty_lines_total_zero")]
    #[test_case(10, 10, 1, 0   ; "last_line")]
    fn lines_remaining(total: usize, start: usize, shown: usize, expected: usize) {
        assert_eq!(lines_remaining_after(total, start, shown), expected);
    }

    fn line(text: &str) -> SnapshotLine {
        SnapshotLine {
            spans: vec![SnapshotSpan {
                text: text.into(),
                style: SpanStyle::Default,
            }],
        }
    }

    #[test]
    fn shared_buf_lifecycle() {
        let buf = SharedBuf::new();

        assert!(buf.is_empty());
        assert_eq!(buf.version(), 0);

        for i in 0..3 {
            buf.append(line(&format!("l{i}")));
        }
        assert_eq!(buf.len(), 3);

        let v = buf.version();
        assert_ne!(v, 0, "version changes after appends");
        let snap = buf.read();
        assert_eq!(snap.len(), 3);
        assert_eq!(snap[0].spans[0].text, "l0");
        assert_eq!(buf.version(), v, "reading does not change the version");

        buf.append(line("l3"));
        assert_ne!(buf.version(), v);
        assert_eq!(buf.take().lines.len(), 4);
    }

    #[test]
    fn shared_buf_arc_snapshot_isolation() {
        let buf = SharedBuf::new();
        buf.append(line("a"));
        buf.append(line("b"));
        let snap = buf.read();
        buf.append(line("c"));
        assert_eq!(snap.len(), 2, "held Arc must not see new appends");
        assert_eq!(buf.read().len(), 3);
    }

    #[test]
    fn shared_buf_poisoned_mutex_recovery() {
        let buf = Arc::new(SharedBuf::new());
        let buf2 = Arc::clone(&buf);
        let h = std::thread::spawn(move || {
            let _guard = buf2.inner.lock().unwrap();
            panic!("intentional poison");
        });
        let _ = h.join();
        buf.append(SnapshotLine { spans: vec![] });
    }

    #[test]
    fn buffer_snapshot_first_line_text() {
        let empty = BufferSnapshot {
            lines: Arc::new(vec![]),
        };
        assert_eq!(empty.first_line_text(), "");

        let multi = BufferSnapshot {
            lines: Arc::new(vec![SnapshotLine {
                spans: vec![
                    SnapshotSpan {
                        text: "hello ".into(),
                        style: SpanStyle::Default,
                    },
                    SnapshotSpan {
                        text: "world".into(),
                        style: SpanStyle::Named("bold".into()),
                    },
                ],
            }]),
        };
        assert_eq!(multi.first_line_text(), "hello world");
    }

    #[test_case(SpanStyle::Default ; "default")]
    #[test_case(SpanStyle::Named("comment".into()) ; "named")]
    #[test_case(SpanStyle::Inline(InlineStyle {
        fg: Some((255, 0, 0)),
        bg: None,
        bold: true,
        italic: false,
        underline: true,
        dim: false,
        strikethrough: false,
        reversed: true,
    }) ; "inline")]
    fn snapshot_span_serde_roundtrip(style: SpanStyle) {
        let span = SnapshotSpan {
            text: "test".into(),
            style,
        };
        let json = serde_json::to_string(&span).unwrap();
        let parsed: SnapshotSpan = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, span);
    }

    #[test_case("", true  ; "plain_output_is_empty_for_empty_string")]
    #[test_case("a.rs\nb.rs", false ; "plain_output_not_empty_for_content")]
    fn plain_output_is_empty(text: &str, expected: bool) {
        assert_eq!(ToolOutput::Plain(text.into()).is_empty_result(), expected);
    }

    #[test]
    fn agent_event_tool_snapshot_theme_gen_backwards_compat() {
        const OMIT_MSG: &str = "theme_gen: None must not appear in serialized JSON";
        const COMPAT_MSG: &str = "missing theme_gen must deserialize as None (backwards compat)";

        let event = AgentEvent::ToolSnapshot {
            id: "t1".into(),
            snapshot: BufferSnapshot {
                lines: Arc::new(vec![]),
            },
            theme_gen: None,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(!json.contains("theme_gen"), "{OMIT_MSG}");

        #[derive(Deserialize)]
        struct ToolSnapshotFields {
            #[allow(dead_code)]
            id: String,
            #[serde(default)]
            theme_gen: Option<u64>,
        }
        let json_without = r#"{"id":"t1"}"#;
        let parsed: ToolSnapshotFields = serde_json::from_str(json_without).unwrap();
        assert_eq!(parsed.theme_gen, None, "{COMPAT_MSG}");
    }

    #[test]
    fn embed_reflects_later_child_writes() {
        let parent = Arc::new(SharedBuf::new());
        let child = Arc::new(SharedBuf::new());
        parent.append(line("header"));
        assert!(parent.embed(&child, "  ".into(), None));

        let v = parent.version();
        child.append(line("body"));
        assert_ne!(parent.version(), v, "child write must bump parent version");

        let snap = parent.read();
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[1].spans[0].text, "  ");
        assert_eq!(snap[1].spans[1].text, "body");
    }

    #[test]
    fn embed_first_indent_applies_to_first_line_only() {
        let parent = Arc::new(SharedBuf::new());
        let child = Arc::new(SharedBuf::new());
        child.append(line("first"));
        child.append(line("rest"));
        assert!(parent.embed(&child, "    ".into(), Some("  ".into())));

        let snap = parent.read();
        assert_eq!(snap[0].spans[0].text, "  ");
        assert_eq!(snap[1].spans[0].text, "    ");
    }

    #[test]
    fn embed_indents_compose_across_nesting() {
        let root = Arc::new(SharedBuf::new());
        let slot = Arc::new(SharedBuf::new());
        let body = Arc::new(SharedBuf::new());
        body.append(line("deep"));
        assert!(slot.embed(&body, "..".into(), None));
        assert!(root.embed(&slot, "--".into(), None));

        let snap = root.read();
        assert_eq!(snap[0].spans[0].text, "--..");
        assert_eq!(snap[0].spans[1].text, "deep");
    }

    #[test]
    fn embed_rejects_cycles() {
        let a = Arc::new(SharedBuf::new());
        let b = Arc::new(SharedBuf::new());
        assert!(a.embed(&b, String::new(), None));
        assert!(!b.embed(&a, String::new(), None), "direct cycle");
        assert!(!a.embed(&a, String::new(), None), "self cycle");
    }

    #[test]
    fn set_lines_keeps_version_monotonic_after_dropping_embeds() {
        let parent = Arc::new(SharedBuf::new());
        let child = Arc::new(SharedBuf::new());
        for _ in 0..10 {
            child.append(line("x"));
        }
        assert!(parent.embed(&child, String::new(), None));
        let before = parent.version();
        parent.set_lines(vec![line("flat")]);
        assert!(
            parent.version() > before,
            "dropping a versioned embed must not rewind the aggregate"
        );
    }

    #[test]
    fn locate_resolves_innermost_buffer_and_local_row() {
        let root = Arc::new(SharedBuf::new());
        let slot = Arc::new(SharedBuf::new());
        let body = Arc::new(SharedBuf::new());
        body.append(line("b0"));
        body.append(line("b1"));
        slot.append(line("slot header"));
        assert!(slot.embed(&body, "  ".into(), None));
        root.append(line("root header"));
        assert!(root.embed(&slot, String::new(), None));

        // row 3 = root[0], slot[0]=header, body[0], body[1]
        let path = root.locate(3);
        assert_eq!(path.len(), 3);
        assert_eq!(path[1].1, 2, "row local to slot");
        assert_eq!(path[2].1, 1, "row local to body");
        assert!(Arc::ptr_eq(&path[2].0, &body));

        let header_path = root.locate(0);
        assert_eq!(header_path.len(), 1, "own line resolves to self only");
    }

    #[test]
    fn len_counts_embedded_lines() {
        let parent = Arc::new(SharedBuf::new());
        let child = Arc::new(SharedBuf::new());
        child.append(line("a"));
        child.append(line("b"));
        parent.append(line("h"));
        assert!(parent.embed(&child, String::new(), None));
        assert_eq!(parent.len(), 3);
    }

    #[test]
    fn assign_replaces_entries_atomically() {
        let dst = Arc::new(SharedBuf::new());
        dst.append(line("old"));
        let v_before = dst.version();

        let src = SharedBuf::new();
        src.append(line("new1"));
        src.append(line("new2"));

        assert!(dst.assign(&src));
        assert!(dst.version() > v_before);
        let lines = dst.read();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].spans[0].text, "new1");
    }

    #[test]
    fn assign_preserves_embeds() {
        let dst = Arc::new(SharedBuf::new());
        let child = Arc::new(SharedBuf::new());
        child.append(line("child"));

        let tmp = Arc::new(SharedBuf::new());
        tmp.append(line("header"));
        assert!(tmp.embed(&child, String::new(), None));

        assert!(dst.assign(&tmp));
        let lines = dst.read();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1].spans[0].text, "child");
    }

    #[test]
    fn assign_rejects_cycles() {
        let a = Arc::new(SharedBuf::new());
        a.append(line("a"));

        let src = Arc::new(SharedBuf::new());
        assert!(src.embed(&a, String::new(), None));

        assert!(!a.assign(&src), "assigning embed of self must be rejected");
    }

    #[test]
    fn assign_version_stays_monotonic() {
        let dst = Arc::new(SharedBuf::new());
        let child = Arc::new(SharedBuf::new());
        for _ in 0..10 {
            child.append(line("x"));
        }
        assert!(dst.embed(&child, String::new(), None));
        let before = dst.version();

        let src = SharedBuf::new();
        src.append(line("flat"));
        assert!(dst.assign(&src));
        assert!(
            dst.version() > before,
            "dropping a versioned embed via assign must not rewind"
        );
    }
}
