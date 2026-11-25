//! The commit graph of a history list: which lane (column) each commit sits in and the lines that join it
//! to its parents, for drawing next to the list one row per commit.
//!
//! Commits come newest first, children before their parents (`git log --topo-order`). Every lane waits
//! for one commit (the parent of the commit above it in that lane); a commit takes the first lane waiting
//! for it, the other lanes waiting for it end there (branches that started from it), its first parent
//! continues its lane and its other parents (merges) get a lane of their own unless one waits for them
//! already. Lanes keep their column (a freed one is reused), so lines that pass a row stay straight.

/// A line through half a row: from lane `from` at one edge to lane `to` at the other (the commit's lane
/// at the row's middle for the lines that start or end at its dot).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edge {
    pub from: usize,
    pub to: usize,
    /// The lane's color index (cycled through the theme's graph colors).
    pub color: usize,
    /// The row of the commit the line comes down from (whose parent it leads to).
    pub child: usize,
}

/// How a commit's row is drawn.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GraphRow {
    /// The commit's lane.
    pub col: usize,
    pub color: usize,
    /// Lines from the top of the row: down to the commit's dot (`to == col`) or past it (`from == to`).
    pub top: Vec<Edge>,
    /// Lines to the bottom of the row: from the commit's dot to its parents' lanes, or past it.
    pub bottom: Vec<Edge>,
    /// Lanes the row uses.
    pub width: usize,
}

impl GraphRow {
    /// The lanes going on below the commit (for rows drawn under it, like its expanded files).
    pub fn below(&self) -> Vec<Edge> {
        let mut out: Vec<Edge> = Vec::new();
        for e in &self.bottom {
            if !out.iter().any(|o| o.to == e.to) {
                out.push(Edge { from: e.to, ..*e });
            }
        }
        out
    }
}

struct Lane {
    /// The commit the lane leads to.
    waits_for: String,
    color: usize,
    child: usize,
}

/// Lays out the graph of `commits`: (id, parent ids), newest first in topological order.
pub fn layout<'a>(commits: impl IntoIterator<Item = (&'a str, &'a [String])>) -> Vec<GraphRow> {
    let mut lanes: Vec<Option<Lane>> = Vec::new();
    let mut next_color = 0usize;
    let mut new_color = || {
        let c = next_color;
        next_color += 1;
        c
    };
    let mut rows = Vec::new();
    for (row, (id, parents)) in commits.into_iter().enumerate() {
        let waiting: Vec<usize> = lanes.iter().enumerate().filter(|(_, l)| l.as_ref().is_some_and(|l| l.waits_for == id)).map(|(i, _)| i).collect();
        let col = match waiting.first() {
            Some(&c) => c,
            None => free_slot(&mut lanes, None),
        };
        let color = match waiting.first() {
            Some(&c) => lanes[c].as_ref().map_or(0, |l| l.color),
            None => new_color(),
        };
        let top: Vec<Edge> = lanes
            .iter()
            .enumerate()
            .filter_map(|(i, l)| l.as_ref().map(|l| Edge { from: i, to: if l.waits_for == id { col } else { i }, color: l.color, child: l.child }))
            .collect();
        for &i in &waiting {
            lanes[i] = None;
        }
        // The parents: the first continues the commit's lane, the others branch off to their own.
        let mut bottom: Vec<Edge> = Vec::new();
        for (k, parent) in parents.iter().enumerate() {
            let existing = lanes.iter().position(|l| l.as_ref().is_some_and(|l| &l.waits_for == parent));
            match existing {
                // The first parent is what a lane further right leads to as well (a branch that started from
                // it): this lane goes on and that one comes over into it, so the main line keeps its
                // column instead of wandering right.
                Some(j) if k == 0 && col < j && lanes[col].is_none() => {
                    let other = lanes[j].take().expect("existing lane");
                    lanes[col] = Some(Lane { waits_for: parent.clone(), color, child: row });
                    bottom.push(Edge { from: col, to: col, color, child: row });
                    bottom.push(Edge { from: j, to: col, color: other.color, child: other.child });
                }
                Some(j) => {
                    let lane_color = lanes[j].as_ref().map_or(color, |l| l.color);
                    bottom.push(Edge { from: col, to: j, color: if k == 0 { color } else { lane_color }, child: row });
                }
                None => {
                    let (j, c) = if k == 0 && lanes.get(col).is_none_or(Option::is_none) { (col, color) } else { (free_slot(&mut lanes, Some(col)), new_color()) };
                    if j >= lanes.len() {
                        lanes.resize_with(j + 1, || None);
                    }
                    lanes[j] = Some(Lane { waits_for: parent.clone(), color: c, child: row });
                    bottom.push(Edge { from: col, to: j, color: c, child: row });
                }
            }
        }
        // The lanes passing by.
        for (i, l) in lanes.iter().enumerate() {
            if let Some(l) = l
                && !bottom.iter().any(|e| e.to == i && e.child == row)
            {
                bottom.push(Edge { from: i, to: i, color: l.color, child: l.child });
            }
        }
        while lanes.last().is_some_and(Option::is_none) {
            lanes.pop();
        }
        let width = top.iter().chain(&bottom).map(|e| e.from.max(e.to) + 1).max().unwrap_or(0).max(col + 1);
        rows.push(GraphRow { col, color, top, bottom, width });
    }
    rows
}

/// The first free lane (other than `skip`), else a new one.
fn free_slot(lanes: &mut Vec<Option<Lane>>, skip: Option<usize>) -> usize {
    match lanes.iter().enumerate().position(|(i, l)| l.is_none() && Some(i) != skip) {
        Some(i) => i,
        None => {
            lanes.push(None);
            lanes.len() - 1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph(commits: &[(&str, &[&str])]) -> Vec<GraphRow> {
        let owned: Vec<(String, Vec<String>)> = commits.iter().map(|(id, ps)| (id.to_string(), ps.iter().map(|p| p.to_string()).collect())).collect();
        layout(owned.iter().map(|(id, ps)| (id.as_str(), ps.as_slice())))
    }

    #[test]
    fn a_straight_line() {
        let rows = graph(&[("c", &["b"]), ("b", &["a"]), ("a", &[])]);
        assert!(rows.iter().all(|r| r.col == 0 && r.width == 1));
        assert_eq!(rows[1].top, [Edge { from: 0, to: 0, color: 0, child: 0 }]);
        assert_eq!(rows[1].bottom, [Edge { from: 0, to: 0, color: 0, child: 1 }]);
        assert!(rows[2].bottom.is_empty(), "the first commit ends the line");
    }

    #[test]
    fn a_merge_and_its_branch() {
        // m merges f (feature) into b; f branched off a.
        //   m       0
        //   |\
        //   b |     0
        //   | f     1
        //   |/
        //   a       0
        let rows = graph(&[("m", &["b", "f"]), ("b", &["a"]), ("f", &["a"]), ("a", &[])]);
        assert_eq!(rows.iter().map(|r| r.col).collect::<Vec<_>>(), [0, 0, 1, 0]);
        assert_eq!(rows[0].bottom, [Edge { from: 0, to: 0, color: 0, child: 0 }, Edge { from: 0, to: 1, color: 1, child: 0 }]);
        assert_eq!(rows[2].col, 1);
        assert_eq!(rows[2].color, 1, "the branch keeps its color");
        // f's line joins a's lane (which b's lane already waits for): it curves into lane 0, in f's color.
        assert!(rows[2].bottom.contains(&Edge { from: 1, to: 0, color: 1, child: 2 }), "{:?}", rows[2].bottom);
        assert_eq!(rows[3].top.iter().filter(|e| e.to == 0).count(), 1, "a single line reaches a: {:?}", rows[3].top);
        assert!(rows.iter().all(|r| r.width <= 2));
    }

    #[test]
    fn the_main_line_keeps_its_lane() {
        // m merges e (a branch listed first) into t; both started from p. Under t, p's line is in lane 0.
        let rows = graph(&[("m", &["t", "e"]), ("e", &["p"]), ("t", &["p"]), ("p", &[])]);
        assert_eq!(rows.iter().map(|r| r.col).collect::<Vec<_>>(), [0, 1, 0, 0]);
        assert!(rows[2].bottom.contains(&Edge { from: 1, to: 0, color: 1, child: 1 }), "e's line comes over: {:?}", rows[2].bottom);
        assert_eq!(rows[3].top.len(), 1);
        assert_eq!(rows[3].width, 1);
    }

    #[test]
    fn diverged_branches_and_freed_lanes() {
        // Local x and remote y both on top of a; then a straight line.
        let rows = graph(&[("x", &["a"]), ("y", &["a"]), ("a", &["z"]), ("z", &[])]);
        assert_eq!((rows[0].col, rows[1].col), (0, 1));
        // y's line joins the lane already leading to a right below y (as `git log --graph` draws it).
        assert!(rows[1].bottom.contains(&Edge { from: 1, to: 0, color: 1, child: 1 }), "{:?}", rows[1].bottom);
        assert_eq!(rows[2].top.iter().filter(|e| e.to == rows[2].col).count(), 1);
        assert_eq!(rows[2].col, 0);
        assert_eq!(rows[3].width, 1, "lane 1 is free again");
        // Rows under a commit (its files) continue the lanes below it.
        assert_eq!(rows[0].below().iter().map(|e| e.from).collect::<Vec<_>>(), [0]);
        assert_eq!(rows[1].below().iter().map(|e| e.from).collect::<Vec<_>>(), [0]);
        // Two lanes side by side while both wait: x's and y's.
        assert_eq!(rows[1].top.len(), 1, "only x's lane comes down to y's row");
        assert_eq!(rows[1].width, 2);
    }
}
