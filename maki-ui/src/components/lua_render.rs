//! Bridges native tool rendering to Lua plugins (registered via
//! `maki_lua::set_tool_renderer`). The batch plugin uses it to embed richly
//! rendered child outputs (code, diffs, grep results) into its parent buffer.

use maki_agent::types::InlineStyle;
use maki_agent::{SnapshotLine, SnapshotSpan, SpanStyle, ToolOutput};
use maki_lua::{RenderedTool, ToolRenderRequest};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;

use super::code_view::{self, RenderLimits, SectionFlags};
use super::tool_display::tool_output_annotation;
use crate::highlight;

pub fn register_tool_renderer() {
    maki_lua::set_tool_renderer(Box::new(render_for_lua));
}

fn render_for_lua(req: &ToolRenderRequest) -> Option<RenderedTool> {
    let expanded = SectionFlags {
        script: req.expanded,
        output: req.expanded,
    };
    let limits = RenderLimits::new(expanded, req.output_limit);
    let do_highlight = req.highlight && highlight::is_ready();
    let content = code_view::render_tool_content(req.input, Some(req.output), do_highlight, limits);
    if content.lines.is_empty() {
        return None;
    }
    Some(RenderedTool {
        lines: content.lines.iter().map(line_to_snapshot).collect(),
        truncated: content.truncation.any(),
        covers_output: covers_output(req.output),
        annotation: tool_output_annotation(req.output),
    })
}

fn covers_output(output: &ToolOutput) -> bool {
    matches!(
        output,
        ToolOutput::ReadCode { .. }
            | ToolOutput::WriteCode { .. }
            | ToolOutput::Diff { .. }
            | ToolOutput::GrepResult { .. }
            | ToolOutput::Instructions { .. }
    )
}

fn line_to_snapshot(line: &Line<'_>) -> SnapshotLine {
    SnapshotLine {
        spans: line
            .spans
            .iter()
            .map(|s| SnapshotSpan {
                text: s.content.to_string(),
                style: style_to_span_style(s.style),
            })
            .collect(),
    }
}

fn style_to_span_style(style: Style) -> SpanStyle {
    let mut inline = InlineStyle::default();
    if let Some(Color::Rgb(r, g, b)) = style.fg {
        inline.fg = Some((r, g, b));
    }
    if let Some(Color::Rgb(r, g, b)) = style.bg {
        inline.bg = Some((r, g, b));
    }
    let m = style.add_modifier;
    inline.bold = m.contains(Modifier::BOLD);
    inline.italic = m.contains(Modifier::ITALIC);
    inline.underline = m.contains(Modifier::UNDERLINED);
    inline.dim = m.contains(Modifier::DIM);
    inline.strikethrough = m.contains(Modifier::CROSSED_OUT);
    inline.reversed = m.contains(Modifier::REVERSED);
    if inline == InlineStyle::default() {
        SpanStyle::Default
    } else {
        SpanStyle::Inline(inline)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::text::Span;
    use test_case::test_case;

    #[test]
    fn default_style_maps_to_default_span_style() {
        assert_eq!(style_to_span_style(Style::default()), SpanStyle::Default);
    }

    #[test]
    fn rgb_and_modifiers_map_to_inline() {
        let style = Style::default()
            .fg(Color::Rgb(1, 2, 3))
            .bg(Color::Rgb(4, 5, 6))
            .add_modifier(Modifier::BOLD | Modifier::DIM);
        let SpanStyle::Inline(inline) = style_to_span_style(style) else {
            panic!("expected inline style");
        };
        assert_eq!(inline.fg, Some((1, 2, 3)));
        assert_eq!(inline.bg, Some((4, 5, 6)));
        assert!(inline.bold);
        assert!(inline.dim);
        assert!(!inline.italic);
    }

    #[test]
    fn non_rgb_colors_are_dropped() {
        let style = Style::default().fg(Color::Red);
        assert_eq!(style_to_span_style(style), SpanStyle::Default);
    }

    #[test]
    fn line_to_snapshot_preserves_text_and_span_count() {
        let line = Line::from(vec![
            Span::raw("a"),
            Span::styled("b", Style::default().fg(Color::Rgb(9, 9, 9))),
        ]);
        let snap = line_to_snapshot(&line);
        assert_eq!(snap.spans.len(), 2);
        assert_eq!(snap.spans[0].text, "a");
        assert_eq!(snap.spans[0].style, SpanStyle::Default);
        assert_eq!(snap.spans[1].text, "b");
        assert!(matches!(snap.spans[1].style, SpanStyle::Inline(_)));
    }

    #[test_case(ToolOutput::Plain("x".into()), false ; "plain_not_covered")]
    #[test_case(ToolOutput::Markdown("x".into()), false ; "markdown_not_covered")]
    #[test_case(ToolOutput::ReadCode { path: "a.rs".into(), start_line: 1, lines: vec!["x".into()], total_lines: 1, instructions: None }, true ; "read_code_covered")]
    #[test_case(ToolOutput::Diff { path: "a.rs".into(), before: "a".into(), after: "b".into(), summary: String::new() }, true ; "diff_covered")]
    fn covers_output_by_variant(output: ToolOutput, expected: bool) {
        assert_eq!(covers_output(&output), expected);
    }

    #[test]
    fn render_for_lua_returns_none_for_plain_output() {
        let req = ToolRenderRequest {
            input: None,
            output: &ToolOutput::Plain("just text".into()),
            output_limit: 5,
            expanded: false,
            highlight: true,
        };
        assert!(render_for_lua(&req).is_none());
    }

    #[test]
    fn render_for_lua_renders_read_code_with_line_numbers() {
        let output = ToolOutput::ReadCode {
            path: "a.rs".into(),
            start_line: 1,
            lines: vec!["fn main() {}".into()],
            total_lines: 1,
            instructions: None,
        };
        let req = ToolRenderRequest {
            input: None,
            output: &output,
            output_limit: 5,
            expanded: false,
            highlight: false,
        };
        let rendered = render_for_lua(&req).expect("read code must render");
        assert!(rendered.covers_output);
        assert!(!rendered.truncated);
        let text: String = rendered.lines[0]
            .spans
            .iter()
            .map(|s| s.text.as_str())
            .collect();
        assert!(text.contains("fn main() {}"));
        assert!(text.starts_with("1 "));
    }

    #[test]
    fn render_for_lua_truncates_past_output_limit() {
        let lines: Vec<String> = (0..20).map(|i| format!("line {i}")).collect();
        let output = ToolOutput::ReadCode {
            path: "a.rs".into(),
            start_line: 1,
            lines,
            total_lines: 20,
            instructions: None,
        };
        let req = ToolRenderRequest {
            input: None,
            output: &output,
            output_limit: 5,
            expanded: false,
            highlight: false,
        };
        let rendered = render_for_lua(&req).expect("read code must render");
        assert!(rendered.truncated);
        assert_eq!(rendered.lines.len(), 6, "5 code lines + truncation notice");

        let expanded = ToolRenderRequest {
            expanded: true,
            ..req
        };
        let rendered = render_for_lua(&expanded).expect("expanded must render");
        assert!(!rendered.truncated);
        assert_eq!(rendered.lines.len(), 20);
    }
}
