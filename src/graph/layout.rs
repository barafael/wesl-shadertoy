//! Layered (Sugiyama-style) layout of a dataflow graph, left to right.
//!
//! 1. Cycles (loop-carried `var` updates) are broken by a depth-first
//!    search: back edges are laid out reversed and drawn as such.
//! 2. Layers: a node sits one column left of its furthest-right consumer
//!    (longest path to a sink), so the return values end up in the last
//!    column and inputs sit just before their first use.
//! 3. Edges spanning several columns get a dummy node per column crossed,
//!    so they route around boxes instead of through them.
//! 4. A few barycenter sweeps reorder each column to cut crossings,
//!    keeping the best order seen.
//! 5. Columns are as wide as their widest box; within a column boxes are
//!    pulled towards the mean height of their neighbors, then pushed apart
//!    so none overlap.

use bevy_egui::egui::{Pos2, Rect, Vec2, pos2, vec2};

/// Horizontal gap between columns.
const COLUMN_GAP: f32 = 56.0;

/// Vertical gap between boxes in a column.
const ROW_GAP: f32 = 14.0;

/// Height an edge passing through a column takes up there.
const DUMMY_HEIGHT: f32 = 6.0;

/// Barycenter passes (each one down and one up).
const SWEEPS: usize = 6;

/// Where everything goes, in graph space (points at zoom 1, origin top-left).
#[derive(Clone, Debug, Default)]
pub struct Layout {
    /// One rect per node.
    pub rects: Vec<Rect>,
    /// One route per input edge, from the source's right edge to the
    /// target's left edge through the columns in between.
    pub routes: Vec<Route>,
    pub size: Vec2,
}

#[derive(Clone, Debug, Default)]
pub struct Route {
    pub points: Vec<Pos2>,
    /// Closes a cycle: runs right to left.
    pub back: bool,
}

/// Lay out `sizes.len()` boxes connected by `edges` (`(from, to)`).
pub fn layout(sizes: &[Vec2], edges: &[(usize, usize)]) -> Layout {
    let n = sizes.len();
    let back = back_edges(n, edges);
    // The acyclic version: back edges reversed, self-loops dropped.
    let forward: Vec<(usize, usize)> = edges
        .iter()
        .zip(&back)
        .filter(|((a, b), _)| a != b)
        .map(|(&(a, b), &back)| if back { (b, a) } else { (a, b) })
        .collect();
    let layers = assign_layers(n, &forward);
    let columns = layers.iter().copied().max().map_or(0, |m| m + 1);

    // Nodes of the proper graph: the real ones, then one dummy per column
    // a long edge crosses. `chains[i]` lists edge i's nodes in column order.
    let mut layer_of = layers.clone();
    let mut height: Vec<f32> = sizes.iter().map(|s| s.y).collect();
    let mut chains = Vec::with_capacity(forward.len());
    let mut links = Vec::new();
    for &(a, b) in &forward {
        let mut chain = vec![a];
        for layer in layers[a] + 1..layers[b] {
            layer_of.push(layer);
            height.push(DUMMY_HEIGHT);
            chain.push(layer_of.len() - 1);
        }
        chain.push(b);
        links.extend(chain.windows(2).map(|w| (w[0], w[1])));
        chains.push(chain);
    }
    let total = layer_of.len();
    let mut preds = vec![Vec::new(); total];
    let mut succs = vec![Vec::new(); total];
    for &(a, b) in &links {
        succs[a].push(b);
        preds[b].push(a);
    }

    let order = order_columns(columns, &layer_of, &preds, &succs);

    // x: column by column, boxes centered in their column.
    let mut column_width = vec![0.0f32; columns];
    for (i, size) in sizes.iter().enumerate() {
        column_width[layers[i]] = column_width[layers[i]].max(size.x);
    }
    let mut column_x = Vec::with_capacity(columns);
    let mut x = 0.0;
    for w in &column_width {
        column_x.push(x);
        x += w + COLUMN_GAP;
    }
    let width = (x - COLUMN_GAP).max(0.0);
    let center_x = |i: usize| column_x[layer_of[i]] + column_width[layer_of[i]] / 2.0;

    let y = place_rows(&order, &height, &preds, &succs);

    let rects: Vec<Rect> = (0..n)
        .map(|i| Rect::from_center_size(pos2(center_x(i), y[i] + height[i] / 2.0), sizes[i]))
        .collect();
    let mut chains = chains.into_iter();
    let routes = edges
        .iter()
        .zip(&back)
        .map(|(&(a, b), &back)| {
            let out = |i: usize| pos2(rects[i].right(), rects[i].center().y);
            let into = |i: usize| pos2(rects[i].left(), rects[i].center().y);
            if a == b {
                return Route {
                    points: vec![out(a), into(b)],
                    back: true,
                };
            }
            let chain = chains.next().expect("one chain per non-loop edge");
            let middle = chain[1..chain.len() - 1]
                .iter()
                .map(|&d| pos2(center_x(d), y[d] + DUMMY_HEIGHT / 2.0));
            let mut points: Vec<Pos2> = std::iter::once(out(chain[0]))
                .chain(middle)
                .chain(std::iter::once(into(chain[chain.len() - 1])))
                .collect();
            if back {
                // Laid out reversed: run from the real source's right edge
                // back to the real target's left edge.
                points.reverse();
                let last = points.len() - 1;
                points[0] = out(a);
                points[last] = into(b);
            }
            Route { points, back }
        })
        .collect();
    let bottom = (0..total).map(|i| y[i] + height[i]).fold(0.0, f32::max);
    Layout {
        rects,
        routes,
        size: vec2(width, bottom),
    }
}

/// Which edges close a cycle, found by depth-first search in node order.
fn back_edges(n: usize, edges: &[(usize, usize)]) -> Vec<bool> {
    let mut out = vec![Vec::new(); n];
    for (i, &(a, _)) in edges.iter().enumerate() {
        out[a].push(i);
    }
    // 0: unvisited, 1: on the stack, 2: done.
    let mut state = vec![0u8; n];
    let mut back = vec![false; edges.len()];
    for root in 0..n {
        if state[root] != 0 {
            continue;
        }
        // Iterative DFS: (node, next outgoing edge to try).
        let mut stack = vec![(root, 0)];
        state[root] = 1;
        while let Some(&mut (node, ref mut next)) = stack.last_mut() {
            if let Some(&e) = out[node].get(*next) {
                *next += 1;
                let to = edges[e].1;
                match state[to] {
                    0 => {
                        state[to] = 1;
                        stack.push((to, 0));
                    }
                    1 => back[e] = true,
                    _ => {}
                }
            } else {
                state[node] = 2;
                stack.pop();
            }
        }
    }
    back
}

/// Column per node: the longest path to a sink, counted from the right.
/// Isolated nodes go in the first column.
fn assign_layers(n: usize, edges: &[(usize, usize)]) -> Vec<usize> {
    let mut succs = vec![Vec::new(); n];
    let mut connected = vec![false; n];
    for &(a, b) in edges {
        succs[a].push(b);
        connected[a] = true;
        connected[b] = true;
    }
    // Depth to a sink, by memoized DFS (the graph is acyclic here).
    let mut depth: Vec<Option<usize>> = vec![None; n];
    for root in 0..n {
        let mut stack = vec![root];
        while let Some(&node) = stack.last() {
            if depth[node].is_some() {
                stack.pop();
                continue;
            }
            let pending: Vec<usize> = succs[node]
                .iter()
                .copied()
                .filter(|&s| depth[s].is_none())
                .collect();
            if pending.is_empty() {
                let d = succs[node]
                    .iter()
                    .map(|&s| depth[s].unwrap_or(0) + 1)
                    .max()
                    .unwrap_or(0);
                depth[node] = Some(d);
                stack.pop();
            } else {
                stack.extend(pending);
            }
        }
    }
    let max = depth.iter().flatten().copied().max().unwrap_or(0);
    (0..n)
        .map(|i| {
            if connected[i] {
                max - depth[i].unwrap_or(0)
            } else {
                0
            }
        })
        .collect()
}

/// The order of the nodes in each column, after barycenter sweeps.
fn order_columns(
    columns: usize,
    layer_of: &[usize],
    preds: &[Vec<usize>],
    succs: &[Vec<usize>],
) -> Vec<Vec<usize>> {
    let mut order = vec![Vec::new(); columns];
    for (i, &l) in layer_of.iter().enumerate() {
        order[l].push(i);
    }
    let mut best = order.clone();
    let mut best_crossings = crossings(&order, succs);
    for sweep in 0..SWEEPS * 2 {
        let down = sweep % 2 == 0;
        let mut position = vec![0.0f32; layer_of.len()];
        for column in &order {
            for (p, &i) in column.iter().enumerate() {
                position[i] = p as f32;
            }
        }
        let range: Vec<usize> = if down {
            (1..columns).collect()
        } else {
            (0..columns.saturating_sub(1)).rev().collect()
        };
        for l in range {
            let neighbors = if down { preds } else { succs };
            let mut keyed: Vec<(f32, usize)> = order[l]
                .iter()
                .map(|&i| {
                    let ns = &neighbors[i];
                    let key = if ns.is_empty() {
                        position[i]
                    } else {
                        ns.iter().map(|&j| position[j]).sum::<f32>() / ns.len() as f32
                    };
                    (key, i)
                })
                .collect();
            keyed.sort_by(|a, b| a.0.total_cmp(&b.0));
            order[l] = keyed.into_iter().map(|(_, i)| i).collect();
            for (p, &i) in order[l].iter().enumerate() {
                position[i] = p as f32;
            }
        }
        let c = crossings(&order, succs);
        if c < best_crossings {
            best_crossings = c;
            best = order.clone();
        }
    }
    best
}

/// Edge crossings between adjacent columns.
pub fn crossings(order: &[Vec<usize>], succs: &[Vec<usize>]) -> usize {
    let mut position = vec![0usize; succs.len()];
    for column in order {
        for (p, &i) in column.iter().enumerate() {
            position[i] = p;
        }
    }
    let mut total = 0;
    for column in order {
        let edges: Vec<(usize, usize)> = column
            .iter()
            .flat_map(|&a| succs[a].iter().map(move |&b| (a, b)))
            .map(|(a, b)| (position[a], position[b]))
            .collect();
        for (i, &(a1, b1)) in edges.iter().enumerate() {
            for &(a2, b2) in &edges[i + 1..] {
                if (a1 < a2 && b1 > b2) || (a1 > a2 && b1 < b2) {
                    total += 1;
                }
            }
        }
    }
    total
}

/// Top y of every (proper-graph) node: stacked in column order, then a
/// few passes pull each box towards its neighbors' mean height without
/// letting boxes overlap or change order.
fn place_rows(
    order: &[Vec<usize>],
    height: &[f32],
    preds: &[Vec<usize>],
    succs: &[Vec<usize>],
) -> Vec<f32> {
    let mut y = vec![0.0f32; height.len()];
    for column in order {
        let mut top = 0.0;
        for &i in column {
            y[i] = top;
            top += height[i] + ROW_GAP;
        }
    }
    let center = |y: &[f32], i: usize| y[i] + height[i] / 2.0;
    for pass in 0..8 {
        let down = pass % 2 == 0;
        let columns: Vec<&Vec<usize>> = if down {
            order.iter().collect()
        } else {
            order.iter().rev().collect()
        };
        for column in columns {
            let neighbors = if down { preds } else { succs };
            // Wanted top of each box.
            let mut want: Vec<f32> = column
                .iter()
                .map(|&i| {
                    let ns = &neighbors[i];
                    if ns.is_empty() {
                        y[i]
                    } else {
                        let mean = ns.iter().map(|&j| center(&y, j)).sum::<f32>() / ns.len() as f32;
                        mean - height[i] / 2.0
                    }
                })
                .collect();
            // Push down past the box above, then up past the box below,
            // which leaves every gap at least ROW_GAP.
            for k in 1..column.len() {
                let above = want[k - 1] + height[column[k - 1]] + ROW_GAP;
                want[k] = want[k].max(above);
            }
            for k in (0..column.len().saturating_sub(1)).rev() {
                let below = want[k + 1] - height[column[k]] - ROW_GAP;
                want[k] = want[k].min(below);
            }
            for (k, &i) in column.iter().enumerate() {
                y[i] = want[k];
            }
        }
    }
    // The passes drift connected boxes down; nodes without neighbors
    // stay put, so they are left out here and stacked below the rest.
    let alone = |i: usize| preds[i].is_empty() && succs[i].is_empty();
    let top = (0..y.len())
        .filter(|&i| !alone(i))
        .map(|i| y[i])
        .fold(f32::INFINITY, f32::min);
    if top.is_finite() {
        for v in &mut y {
            *v -= top;
        }
    }
    for column in order {
        let mut bottom = column
            .iter()
            .filter(|&&i| !alone(i))
            .map(|&i| y[i] + height[i] + ROW_GAP)
            .fold(0.0, f32::max);
        for &i in column.iter().filter(|&&i| alone(i)) {
            y[i] = bottom;
            bottom += height[i] + ROW_GAP;
        }
    }
    y
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Column of each node, from the box centers.
    fn columns(l: &Layout) -> Vec<usize> {
        let mut xs: Vec<f32> = l.rects.iter().map(|r| r.center().x).collect();
        xs.sort_by(f32::total_cmp);
        xs.dedup();
        l.rects
            .iter()
            .map(|r| xs.iter().position(|&x| x == r.center().x).unwrap())
            .collect()
    }

    fn check(sizes: &[Vec2], edges: &[(usize, usize)]) -> Layout {
        let l = layout(sizes, edges);
        assert_eq!(l.rects.len(), sizes.len());
        assert_eq!(l.routes.len(), edges.len());
        for (i, a) in l.rects.iter().enumerate() {
            assert!((a.size() - sizes[i]).length() < 1e-3);
            assert!(a.min.x >= -1e-3 && a.min.y >= -1e-3, "{a:?}");
            assert!(a.max.x <= l.size.x + 1e-3 && a.max.y <= l.size.y + 1e-3);
            for b in &l.rects[i + 1..] {
                assert!(!a.intersects(*b), "{a:?} overlaps {b:?}");
            }
        }
        for (route, &(a, b)) in l.routes.iter().zip(edges) {
            if !route.back {
                // Dependencies flow left to right.
                assert!(l.rects[a].max.x < l.rects[b].min.x, "{a} -> {b}");
                assert!(route.points.windows(2).all(|w| w[0].x < w[1].x));
            }
        }
        l
    }

    #[test]
    fn chain_and_diamond() {
        let s = vec2(80.0, 30.0);
        // 0 -> 1 -> 3, 0 -> 2 -> 3, and 0 -> 3 directly (spans two columns).
        let l = check(&[s; 4], &[(0, 1), (0, 2), (1, 3), (2, 3), (0, 3)]);
        assert_eq!(columns(&l), [0, 1, 1, 2]);
        // The long edge passes through a dummy in the middle column.
        assert_eq!(l.routes[4].points.len(), 3);
    }

    #[test]
    fn inputs_sit_next_to_their_use() {
        let s = vec2(60.0, 20.0);
        // 0 -> 1 -> 2 -> 3, and 4 feeds 3 only: it goes in column 2.
        let l = check(&[s; 5], &[(0, 1), (1, 2), (2, 3), (4, 3)]);
        assert_eq!(columns(&l), [0, 1, 2, 3, 2]);
    }

    #[test]
    fn cycles_and_self_loops() {
        let s = vec2(50.0, 20.0);
        // A var updated in a loop: 0 -> 1 -> 2 -> 1, plus 1 -> 1.
        let l = check(&[s; 4], &[(0, 1), (1, 2), (2, 1), (1, 1), (1, 3)]);
        assert_eq!(l.routes.iter().filter(|r| r.back).count(), 2);
    }

    #[test]
    fn barycenter_removes_avoidable_crossings() {
        let s = vec2(40.0, 20.0);
        // Two independent chains, interleaved in the initial order.
        let edges = [(0, 3), (1, 2)];
        let l = check(&[s; 4], &edges);
        let (a, b) = (l.rects[0].center().y, l.rects[1].center().y);
        let (c, d) = (l.rects[3].center().y, l.rects[2].center().y);
        assert_eq!(a < b, c < d, "the two edges cross");
    }

    #[test]
    fn varied_sizes_never_overlap() {
        let sizes: Vec<Vec2> = (0..30)
            .map(|i| vec2(40.0 + (i * 37 % 120) as f32, 20.0 + (i * 13 % 70) as f32))
            .collect();
        let edges: Vec<(usize, usize)> = (1..30)
            .flat_map(|i| [(i * 7 % i, i), ((i * 3 + 1) % i, i)])
            .chain([(29, 4), (12, 12)])
            .collect();
        check(&sizes, &edges);
    }

    /// A node without edges does not hold the others' coordinates hostage
    /// (they once drifted thousands of units below it).
    #[test]
    fn isolated_nodes_go_below() {
        let s = vec2(50.0, 20.0);
        let edges: Vec<(usize, usize)> = (1..12)
            .map(|i| (i - 1, i))
            .chain([(0, 5), (2, 9)])
            .collect();
        let mut sizes = vec![s; 12];
        sizes.push(s);
        let l = check(&sizes, &edges);
        assert!(l.size.y < 200.0, "{:?}", l.size);
        assert!(l.rects[12].min.y > l.rects[0].min.y);
    }
}
