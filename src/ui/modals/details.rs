//! Details of one operation, including its full `$currentOp` document.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};

use super::{DISMISS_HINT, modal_block, open, render_scrolled};
use crate::app::LayoutCache;
use crate::model::Operation;
use crate::theme::Palette;
use crate::ui::layout::{centered, percent};
use crate::ui::style;
use crate::ui::text::{pretty_json, sanitize_owned, to_u16, wrap_chars};
use crate::ui::widgets::with_hint;

const COMMAND_TITLE: &str = "Command Details:";
const DOCUMENT_TITLE: &str = "Full $currentOp document:";

pub fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    op: &Operation,
    scroll: &mut u16,
    p: &Palette,
    layout: &mut LayoutCache,
) {
    let rect = centered(area, percent(area.width, 80), percent(area.height, 80));
    let block = with_hint(
        modal_block("Operation Details", p),
        DISMISS_HINT,
        rect.width,
    );
    let inner = open(frame, rect, block, layout);
    if inner.is_empty() {
        return;
    }
    let width = usize::from(inner.width);
    let lines: Vec<Line<'static>> = styled_lines(details_lines(op), p)
        .iter()
        .flat_map(|spans| wrap_chars(spans, width))
        .collect();
    let shown = render_scrolled(frame, (rect, inner), lines, usize::from(*scroll), p);
    *scroll = to_u16(shown);
}

/// Text of the details view: the main fields, then the command and the
/// whole `$currentOp` document as pretty-printed relaxed extended JSON.
pub fn details_lines(op: &Operation) -> Vec<String> {
    let present = |value: &Option<String>| value.clone().filter(|v| !v.is_empty());
    let mut lines = vec![
        format!("Operation ID: {}", op.opid),
        format!("Type: {}", op.op),
        format!(
            "Namespace: {}",
            if op.ns.is_empty() { "N/A" } else { &op.ns }
        ),
        format!("Running Time: {}s", op.secs_running),
        format!("Client: {}", op.client_display()),
    ];
    if let Some(shard) = present(&op.shard) {
        lines.push(format!("Shard: {shard}"));
    }
    if let Some(host) = present(&op.host) {
        lines.push(match op.node_role {
            Some(role) => format!("Host: {host} ({})", role.label()),
            None => format!("Host: {host}"),
        });
    }
    if let Some(app_name) = present(&op.app_name) {
        lines.push(format!("App Name: {app_name}"));
    }
    if op.effective_users.iter().any(|u| !u.is_empty()) {
        lines.push(format!("Effective Users: {}", op.users_display()));
    }
    if let Some(plan) = present(&op.plan_summary) {
        lines.push(format!("Plan Summary: {plan}"));
    }
    if op.kill_pending {
        lines.push("Kill Pending: yes".to_owned());
    }
    if let Ok(command) = op.raw.get_document("command") {
        lines.push(String::new());
        lines.push(COMMAND_TITLE.to_owned());
        lines.extend(pretty_json(command).lines().map(str::to_owned));
    }
    lines.push(String::new());
    lines.push(DOCUMENT_TITLE.to_owned());
    lines.extend(pretty_json(&op.raw).lines().map(str::to_owned));
    lines
}

/// Field labels and section titles highlighted.
fn styled_lines(lines: Vec<String>, p: &Palette) -> Vec<Vec<Span<'static>>> {
    let mut in_fields = true;
    lines
        .into_iter()
        .map(|line| {
            let line = sanitize_owned(line);
            if line.is_empty() {
                in_fields = false;
            }
            if line == COMMAND_TITLE || line == DOCUMENT_TITLE {
                return vec![Span::styled(line, style::title(p))];
            }
            match line.split_once(": ") {
                Some((label, value)) if in_fields => vec![
                    Span::styled(format!("{label}:"), style::key(p)),
                    Span::raw(format!(" {value}")),
                ],
                _ => vec![Span::raw(line)],
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use mongodb::bson::doc;

    use super::*;
    use crate::model::{NodeRole, OpId};
    use crate::testutil::{operation_running, sample_operation};
    use crate::theme;

    #[test]
    fn main_fields_then_pretty_json() {
        let op = operation_running(7, 12);
        let lines = details_lines(&op);
        assert_eq!(
            lines[..6],
            [
                "Operation ID: 7",
                "Type: query",
                "Namespace: app.users",
                "Running Time: 12s",
                "Client: 10.0.0.1:5000",
                "Host: db1:27017",
            ]
        );
        assert_eq!(lines[6], "Effective Users: alice");
        assert_eq!(lines[7], "");
        assert_eq!(lines[8], COMMAND_TITLE);
        assert_eq!(lines[9], "{");
        assert_eq!(lines[10], "  \"find\": \"users\",");
        assert!(lines.contains(&"    \"age\": 3".to_owned()), "{lines:#?}");
        let document = lines.iter().position(|l| l == DOCUMENT_TITLE).unwrap();
        assert_eq!(lines[document - 1], "");
        assert_eq!(lines[document + 1], "{");
        assert_eq!(lines[document + 2], "  \"command\": {");
        assert_eq!(lines.last().unwrap(), "}");
    }

    #[test]
    fn optional_fields_when_present() {
        let mut op = sample_operation(OpId::Str("shard01:42".into()));
        op.ns.clear();
        op.shard = Some("shard01".into());
        op.node_role = Some(NodeRole::Secondary);
        op.app_name = Some("reporting".into());
        op.plan_summary = Some("IXSCAN { age: 1 }".into());
        op.kill_pending = true;
        op.effective_users.clear();
        op.raw = doc! { "opid": "shard01:42" };
        let lines = details_lines(&op);
        assert_eq!(
            lines,
            [
                "Operation ID: shard01:42",
                "Type: query",
                "Namespace: N/A",
                "Running Time: 3s",
                "Client: 10.0.0.1:5000",
                "Shard: shard01",
                "Host: db1:27017 (secondary)",
                "App Name: reporting",
                "Plan Summary: IXSCAN { age: 1 }",
                "Kill Pending: yes",
                "",
                DOCUMENT_TITLE,
                "{",
                "  \"opid\": \"shard01:42\"",
                "}",
            ]
        );
    }

    #[test]
    fn labels_and_titles_are_styled() {
        let p = theme::default_theme().palette(true);
        let styled = styled_lines(details_lines(&operation_running(1, 1)), &p);
        assert_eq!(styled[0][0].content, "Operation ID:");
        assert_eq!(styled[0][0].style, style::key(&p));
        assert_eq!(styled[0][1].content, " 1");
        let title = styled
            .iter()
            .find(|spans| spans[0].content == COMMAND_TITLE)
            .unwrap();
        assert_eq!(title[0].style, style::title(&p));
        // JSON lines are not split at ": ".
        let json = styled
            .iter()
            .find(|spans| spans[0].content.contains("\"find\""))
            .unwrap();
        assert_eq!(json.len(), 1);
    }
}
