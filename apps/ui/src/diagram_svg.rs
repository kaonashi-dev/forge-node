//! A review diagram as SVG text: a sequence as columns and rows of arrows,
//! every other kind as breadth-first ranks drawn top-down with straight edges.
//! Colours come in from the theme; this module names none.

use std::collections::{HashMap, VecDeque};
use std::fmt::Write as _;

use domain::{Diagram, DiagramKind};

/// Hex colours from the theme roles.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct Palette {
    pub text: String,
    pub muted: String,
    pub line: String,
    pub fill: String,
    pub stroke: String,
}

const COLUMN: f32 = 170.0;
const HEADER: f32 = 30.0;
const ROW: f32 = 34.0;
const MARGIN: f32 = 16.0;
const BOX_W: f32 = 160.0;
const LINE_H: f32 = 15.0;
const RANK_GAP: f32 = 56.0;
const NODE_GAP: f32 = 24.0;
const FONT: &str = "font-family=\"-apple-system, Helvetica, sans-serif\" font-size=\"11\"";
const MAX_LABEL_CHARS: usize = 26;

fn escaped(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            ch if ch.is_control() => out.push(' '),
            ch => out.push(ch),
        }
    }
    out
}

fn short(text: &str) -> String {
    if text.chars().count() <= MAX_LABEL_CHARS {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(MAX_LABEL_CHARS - 1).collect();
    out.push('…');
    out
}

fn head(width: f32, height: f32, palette: &Palette) -> String {
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width:.0}\" height=\"{height:.0}\" viewBox=\"0 0 {width:.0} {height:.0}\">\
         <defs><marker id=\"arrow\" viewBox=\"0 0 10 10\" refX=\"10\" refY=\"5\" markerWidth=\"7\" markerHeight=\"7\" orient=\"auto-start-reverse\">\
         <path d=\"M 0 0 L 10 5 L 0 10 z\" fill=\"{}\"/></marker></defs>",
        palette.line
    )
}

/// SVG text and the size it draws at, in points.
pub struct Svg {
    pub text: String,
    pub width: f32,
    pub height: f32,
}

#[must_use]
pub fn svg(diagram: &Diagram, palette: &Palette) -> Svg {
    match diagram.kind {
        DiagramKind::Sequence => sequence(diagram, palette),
        _ => ranked(diagram, palette),
    }
}

fn sequence(diagram: &Diagram, palette: &Palette) -> Svg {
    let column: HashMap<&str, usize> = diagram
        .nodes
        .iter()
        .enumerate()
        .map(|(at, node)| (node.id.as_str(), at))
        .collect();
    let x = |at: usize| MARGIN + COLUMN * at as f32 + BOX_W / 2.0;
    let width = MARGIN * 2.0 + COLUMN * diagram.nodes.len().max(1) as f32;
    let height = MARGIN * 2.0 + HEADER + ROW * (diagram.edges.len() as f32 + 1.0);
    let mut out = head(width, height, palette);
    for (at, node) in diagram.nodes.iter().enumerate() {
        let left = MARGIN + COLUMN * at as f32;
        let _ = write!(
            out,
            "<rect x=\"{left:.1}\" y=\"{MARGIN:.1}\" width=\"{BOX_W:.1}\" height=\"{HEADER:.1}\" rx=\"4\" fill=\"{}\" stroke=\"{}\"/>\
             <text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"middle\" fill=\"{}\" {FONT}>{}</text>\
             <line x1=\"{:.1}\" y1=\"{:.1}\" x2=\"{:.1}\" y2=\"{:.1}\" stroke=\"{}\" stroke-dasharray=\"4 4\"/>",
            palette.fill,
            palette.stroke,
            x(at),
            MARGIN + HEADER / 2.0 + 4.0,
            palette.text,
            escaped(&short(&node.label)),
            x(at),
            MARGIN + HEADER,
            x(at),
            height - MARGIN,
            palette.muted,
        );
    }
    for (row, edge) in diagram.edges.iter().enumerate() {
        let (Some(&from), Some(&to)) =
            (column.get(edge.from.as_str()), column.get(edge.to.as_str()))
        else {
            continue;
        };
        let y = MARGIN + HEADER + ROW * (row as f32 + 1.0);
        let (x1, x2) = if from == to {
            (x(from), x(from) + 40.0)
        } else {
            (x(from), x(to))
        };
        let _ = write!(
            out,
            "<line x1=\"{x1:.1}\" y1=\"{y:.1}\" x2=\"{x2:.1}\" y2=\"{y:.1}\" stroke=\"{}\" marker-end=\"url(#arrow)\"/>\
             <text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"middle\" fill=\"{}\" {FONT}>{}</text>",
            palette.line,
            (x1 + x2) / 2.0,
            y - 5.0,
            palette.muted,
            escaped(&short(&edge.label)),
        );
    }
    out.push_str("</svg>");
    Svg {
        text: out,
        width,
        height,
    }
}

/// Breadth-first ranks from the nodes nothing points at; a node in a cycle
/// with no such start is ranked from the first node in the diagram's order.
fn ranks(diagram: &Diagram) -> Vec<usize> {
    let at: HashMap<&str, usize> = diagram
        .nodes
        .iter()
        .enumerate()
        .map(|(i, node)| (node.id.as_str(), i))
        .collect();
    let mut out: Vec<Option<usize>> = vec![None; diagram.nodes.len()];
    let mut incoming = vec![0usize; diagram.nodes.len()];
    let mut next: Vec<Vec<usize>> = vec![Vec::new(); diagram.nodes.len()];
    for edge in &diagram.edges {
        if let (Some(&from), Some(&to)) = (at.get(edge.from.as_str()), at.get(edge.to.as_str())) {
            incoming[to] += 1;
            next[from].push(to);
        }
    }
    let mut queue: VecDeque<usize> = (0..diagram.nodes.len())
        .filter(|i| incoming[*i] == 0)
        .collect();
    for start in &queue {
        out[*start] = Some(0);
    }
    loop {
        while let Some(node) = queue.pop_front() {
            let rank = out[node].unwrap_or(0);
            for &to in &next[node] {
                if out[to].is_none() {
                    out[to] = Some(rank + 1);
                    queue.push_back(to);
                }
            }
        }
        match out.iter().position(Option::is_none) {
            Some(unranked) => {
                out[unranked] = Some(0);
                queue.push_back(unranked);
            }
            None => break,
        }
    }
    out.into_iter().map(|rank| rank.unwrap_or(0)).collect()
}

fn ranked(diagram: &Diagram, palette: &Palette) -> Svg {
    let ranks = ranks(diagram);
    let depth = ranks.iter().copied().max().map_or(0, |max| max + 1);
    let mut rows: Vec<Vec<usize>> = vec![Vec::new(); depth];
    for (node, rank) in ranks.iter().enumerate() {
        rows[*rank].push(node);
    }
    let lines: Vec<Vec<&str>> = diagram
        .nodes
        .iter()
        .map(|node| node.label.lines().collect())
        .collect();
    let row_height: Vec<f32> = rows
        .iter()
        .map(|row| {
            row.iter()
                .map(|n| LINE_H * lines[*n].len().max(1) as f32 + 12.0)
                .fold(0.0, f32::max)
        })
        .collect();
    let widest = rows.iter().map(Vec::len).max().unwrap_or(1).max(1);
    let width = MARGIN * 2.0 + widest as f32 * (BOX_W + NODE_GAP) - NODE_GAP;
    let mut place: Vec<(f32, f32, f32)> = vec![(0.0, 0.0, 0.0); diagram.nodes.len()];
    let mut top = MARGIN;
    for (rank, row) in rows.iter().enumerate() {
        let span = row.len() as f32 * (BOX_W + NODE_GAP) - NODE_GAP;
        let mut left = (width - span) / 2.0;
        for &node in row {
            let height = LINE_H * lines[node].len().max(1) as f32 + 12.0;
            place[node] = (left, top, height);
            left += BOX_W + NODE_GAP;
        }
        top += row_height[rank] + RANK_GAP;
    }
    let height = (top - RANK_GAP + MARGIN).max(MARGIN * 2.0);
    let mut out = head(width, height, palette);
    let at: HashMap<&str, usize> = diagram
        .nodes
        .iter()
        .enumerate()
        .map(|(i, node)| (node.id.as_str(), i))
        .collect();
    for edge in &diagram.edges {
        let (Some(&from), Some(&to)) = (at.get(edge.from.as_str()), at.get(edge.to.as_str()))
        else {
            continue;
        };
        let (fx, fy, fh) = place[from];
        let (tx, ty, th) = place[to];
        let (x1, y1) = (fx + BOX_W / 2.0, if ty >= fy { fy + fh } else { fy });
        let (x2, y2) = (tx + BOX_W / 2.0, if ty >= fy { ty } else { ty + th });
        let _ = write!(
            out,
            "<line x1=\"{x1:.1}\" y1=\"{y1:.1}\" x2=\"{x2:.1}\" y2=\"{y2:.1}\" stroke=\"{}\" marker-end=\"url(#arrow)\"/>",
            palette.line
        );
        if !edge.label.is_empty() {
            let _ = write!(
                out,
                "<text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"middle\" fill=\"{}\" {FONT}>{}</text>",
                (x1 + x2) / 2.0 + 4.0,
                (y1 + y2) / 2.0,
                palette.muted,
                escaped(&short(&edge.label)),
            );
        }
    }
    for (node, (left, top, height)) in place.iter().enumerate() {
        let _ = write!(
            out,
            "<rect x=\"{left:.1}\" y=\"{top:.1}\" width=\"{BOX_W:.1}\" height=\"{height:.1}\" rx=\"4\" fill=\"{}\" stroke=\"{}\"/>",
            palette.fill, palette.stroke
        );
        for (line, text) in lines[node].iter().enumerate() {
            let color = if line == 0 {
                &palette.text
            } else {
                &palette.muted
            };
            let _ = write!(
                out,
                "<text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"middle\" fill=\"{color}\" {FONT}>{}</text>",
                left + BOX_W / 2.0,
                top + 6.0 + LINE_H * (line as f32 + 1.0) - 3.0,
                escaped(&short(text)),
            );
        }
    }
    out.push_str("</svg>");
    Svg {
        text: out,
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use domain::{DiagramEdge, DiagramNode};

    use super::*;

    fn palette() -> Palette {
        Palette {
            text: "#111111".into(),
            muted: "#666666".into(),
            line: "#888888".into(),
            fill: "#fafafa".into(),
            stroke: "#cccccc".into(),
        }
    }

    fn diagram(kind: DiagramKind) -> Diagram {
        let node = |id: &str, label: &str| DiagramNode {
            id: id.into(),
            label: label.into(),
        };
        let edge = |from: &str, to: &str, label: &str| DiagramEdge {
            from: from.into(),
            to: to.into(),
            label: label.into(),
        };
        Diagram {
            kind,
            title: "t".into(),
            nodes: vec![
                node("a", "main.rs"),
                node("b", "lib.rs <&>"),
                node("c", "users\nid\nemail"),
            ],
            edges: vec![edge("a", "b", "total"), edge("b", "c", "")],
        }
    }

    fn rasterizes(svg: &str) {
        let cache = rune::ImageCache::new(Arc::new(|| {}));
        let image = cache.svg(svg.as_bytes().to_vec(), 256.0, 1.0);
        let deadline = Instant::now() + Duration::from_secs(10);
        while !image.is_ready() && image.error().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(image.error().is_none(), "{:?}", image.error());
        assert!(image.size().is_some());
    }

    #[test]
    fn a_sequence_is_columns_and_rows_of_arrows() {
        let drawn = svg(&diagram(DiagramKind::Sequence), &palette());
        assert!(drawn.width > 0.0 && drawn.height > 0.0);
        let svg = drawn.text;
        assert!(svg.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\""));
        assert_eq!(
            svg.matches("stroke-dasharray").count(),
            3,
            "one lifeline per participant"
        );
        assert_eq!(svg.matches("marker-end").count(), 2);
        assert!(svg.contains("lib.rs &lt;&amp;&gt;"));
        assert!(!svg.contains("#000"), "only the palette's colours");
        rasterizes(&svg);
    }

    #[test]
    fn other_kinds_rank_breadth_first_top_down() {
        let flow = diagram(DiagramKind::Flow);
        assert_eq!(ranks(&flow), [0, 1, 2]);
        let svg = svg(&flow, &palette()).text;
        assert_eq!(svg.matches("<rect").count(), 3);
        assert!(
            svg.contains(">email</text>"),
            "an entity's fields are lines"
        );
        rasterizes(&svg);
        let mut cycle = diagram(DiagramKind::Class);
        cycle.edges.push(DiagramEdge {
            from: "c".into(),
            to: "a".into(),
            label: String::new(),
        });
        assert_eq!(ranks(&cycle), [0, 1, 2]);
    }

    #[test]
    fn the_same_diagram_draws_the_same_svg() {
        let a = svg(&diagram(DiagramKind::Er), &palette()).text;
        assert_eq!(a, svg(&diagram(DiagramKind::Er), &palette()).text);
    }
}
