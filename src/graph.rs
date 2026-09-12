use crate::git::Commit;
use std::hash::{DefaultHasher, Hash, Hasher};

pub const PALETTE: [(u8, u8, u8); 6] = [
    (94, 234, 212),
    (167, 139, 250),
    (251, 191, 106),
    (96, 165, 250),
    (244, 114, 182),
    (163, 230, 153),
];

#[derive(Clone, Debug)]
pub struct Lane {
    pub oid: String,
    pub color: usize,
}

#[derive(Clone, Debug)]
pub struct Edge {
    pub from: usize,
    pub to: usize,
    pub color: usize,
}

#[derive(Clone, Debug)]
pub struct Row {
    pub column: usize,
    pub color: usize,
    pub above: Vec<Option<Lane>>,
    pub below: Vec<Option<Lane>>,
    pub edges: Vec<Edge>,
}

#[derive(Clone, Debug, Default)]
pub struct Graph {
    pub rows: Vec<Row>,
    pub width: usize,
    pub fingerprint: u64,
}

// Reserved columns track pending parent OIDs, rather than branch labels.
// Holes are reused without shifting other lanes; first parents inherit color.
impl Graph {
    pub fn build(commits: &[Commit]) -> Self {
        let mut graph = Self::default();
        let mut lanes: Vec<Option<Lane>> = Vec::new();
        let mut next_color = 0;
        let mut fingerprint = DefaultHasher::new();
        for commit in commits {
            commit.oid.hash(&mut fingerprint);
            commit.parents.hash(&mut fingerprint);
            let existing = lanes
                .iter()
                .position(|l| l.as_ref().is_some_and(|l| l.oid == commit.oid));
            let column = existing.unwrap_or_else(|| vacant(&mut lanes));
            let color = if let Some(lane) = &lanes[column] {
                lane.color
            } else {
                let color = next_color;
                next_color += 1;
                color
            };
            let above = lanes.clone();
            lanes[column] = None;
            let mut edges: Vec<_> = above
                .iter()
                .enumerate()
                .filter_map(|(i, lane)| {
                    lane.as_ref().filter(|_| i != column).map(|l| Edge {
                        from: i,
                        to: i,
                        color: l.color,
                    })
                })
                .collect();
            for (i, parent) in commit.parents.iter().enumerate() {
                // Duplicate parent IDs must not introduce duplicate lanes/edges.
                if commit.parents[..i].contains(parent) {
                    continue;
                }
                let destination = lanes
                    .iter()
                    .position(|l| l.as_ref().is_some_and(|l| &l.oid == parent));
                let to = destination.unwrap_or_else(|| {
                    let to = if i == 0 && lanes[column].is_none() {
                        column
                    } else {
                        vacant(&mut lanes)
                    };
                    let branch_color = if i == 0 {
                        color
                    } else {
                        let c = next_color;
                        next_color += 1;
                        c
                    };
                    lanes[to] = Some(Lane {
                        oid: parent.clone(),
                        color: branch_color,
                    });
                    to
                });
                edges.push(Edge {
                    from: column,
                    to,
                    color: lanes[to].as_ref().unwrap().color,
                });
            }
            graph.width = graph.width.max(lanes.len()).max(column + 1);
            graph.rows.push(Row {
                column,
                color,
                above,
                below: lanes.clone(),
                edges,
            });
            while lanes.last().is_some_and(Option::is_none) {
                lanes.pop();
            }
        }
        graph.fingerprint = fingerprint.finish();
        graph
    }
}

fn vacant(lanes: &mut Vec<Option<Lane>>) -> usize {
    if let Some(i) = lanes.iter().position(Option::is_none) {
        i
    } else {
        lanes.push(None);
        lanes.len() - 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn c(oid: &str, parents: &[&str]) -> Commit {
        Commit {
            oid: oid.into(),
            parents: parents.iter().map(|s| s.to_string()).collect(),
            author: String::new(),
            date: String::new(),
            refs: String::new(),
            subject: String::new(),
        }
    }
    #[test]
    fn diamond_preserves_both_paths_and_closes_at_shared_parent() {
        let g = Graph::build(&[
            c("m", &["a", "b"]),
            c("b", &["r"]),
            c("a", &["r"]),
            c("r", &[]),
        ]);
        assert_eq!(g.width, 2);
        assert_eq!(g.rows[0].edges.len(), 2);
        assert_eq!(
            g.rows[2]
                .edges
                .iter()
                .filter(|e| e.from == g.rows[2].column)
                .count(),
            1
        );
        assert!(g.rows[3].below.iter().all(Option::is_none));
        for rows in g.rows.windows(2) {
            let below: Vec<_> = rows[0]
                .below
                .iter()
                .map(|l| l.as_ref().map(|l| &l.oid))
                .collect();
            let above: Vec<_> = rows[1]
                .above
                .iter()
                .map(|l| l.as_ref().map(|l| &l.oid))
                .collect();
            assert_eq!(
                below
                    .iter()
                    .rposition(Option::is_some)
                    .map(|n| &below[..=n]),
                above
                    .iter()
                    .rposition(Option::is_some)
                    .map(|n| &above[..=n])
            );
        }
    }
    #[test]
    fn octopus_and_disconnected_history_are_supported() {
        let g = Graph::build(&[
            c("m", &["a", "b", "c"]),
            c("x", &[]),
            c("a", &["r"]),
            c("b", &["r"]),
            c("c", &["r"]),
            c("r", &[]),
        ]);
        assert_eq!(g.rows[0].edges.len(), 3);
        assert_eq!(g.rows[1].column, 3);
        assert!(g.rows.last().unwrap().below.iter().all(Option::is_none));
    }
    #[test]
    fn truncated_history_keeps_pending_parents() {
        let g = Graph::build(&[c("tip", &["outside-page"])]);
        assert_eq!(g.rows[0].below[0].as_ref().unwrap().oid, "outside-page");
    }
}
