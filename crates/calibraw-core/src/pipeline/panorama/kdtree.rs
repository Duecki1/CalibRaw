//! Approximate nearest neighbours among descriptors: a k-d tree searched best bin first
//! (Beis & Lowe, "Shape indexing using approximate nearest-neighbour search in
//! high-dimensional spaces", 1997), as cpfind and FLANN search SIFT descriptors. Each
//! node splits the dimension of largest variance at its mean, as FLANN does.

use super::features::{Descriptor, DESCRIPTOR_LENGTH};
use std::cmp::Reverse;
use std::collections::BinaryHeap;

/// Points per leaf.
const LEAF_SIZE: usize = 8;
/// Points sampled to choose a node's split dimension.
const SPLIT_SAMPLE: usize = 128;

enum Node {
    Leaf {
        start: usize,
        end: usize,
    },
    Split {
        dimension: usize,
        value: f32,
        below: usize,
        above: usize,
    },
}

pub(super) struct KdTree<'a> {
    points: &'a [Descriptor],
    /// Point indices, grouped by leaf.
    order: Vec<usize>,
    nodes: Vec<Node>,
}

/// The two nearest points of a query: indices and squared distances.
#[derive(Clone, Copy, Debug)]
pub(super) struct Nearest {
    pub(super) index: usize,
    pub(super) distance: f32,
    pub(super) second: f32,
}

impl<'a> KdTree<'a> {
    pub(super) fn new(points: &'a [Descriptor]) -> Self {
        let mut tree = Self {
            points,
            order: (0..points.len()).collect(),
            nodes: Vec::new(),
        };
        if !points.is_empty() {
            tree.build(0, points.len());
        }
        tree
    }

    /// Builds the subtree over `order[start..end]` and returns its node index.
    fn build(&mut self, start: usize, end: usize) -> usize {
        if end - start <= LEAF_SIZE {
            self.nodes.push(Node::Leaf { start, end });
            return self.nodes.len() - 1;
        }
        let step = ((end - start) / SPLIT_SAMPLE).max(1);
        let sample: Vec<usize> = self.order[start..end]
            .iter()
            .step_by(step)
            .copied()
            .collect();
        let count = sample.len() as f32;
        let mut mean = [0.0f32; DESCRIPTOR_LENGTH];
        for &i in &sample {
            for (m, v) in mean.iter_mut().zip(&self.points[i]) {
                *m += v / count;
            }
        }
        let mut variance = [0.0f32; DESCRIPTOR_LENGTH];
        for &i in &sample {
            for ((s, v), m) in variance.iter_mut().zip(&self.points[i]).zip(&mean) {
                *s += (v - m) * (v - m);
            }
        }
        let dimension = (0..DESCRIPTOR_LENGTH)
            .max_by(|&a, &b| variance[a].total_cmp(&variance[b]))
            .unwrap_or(0);
        let value = mean[dimension];
        let points = self.points;
        let slice = &mut self.order[start..end];
        let mut split = 0;
        for i in 0..slice.len() {
            if points[slice[i]][dimension] < value {
                slice.swap(i, split);
                split += 1;
            }
        }
        // Identical values cannot be split by the mean: halve instead.
        let split = if split == 0 || split == slice.len() {
            slice.len() / 2
        } else {
            split
        };
        let node = self.nodes.len();
        self.nodes.push(Node::Leaf { start, end });
        let below = self.build(start, start + split);
        let above = self.build(start + split, end);
        self.nodes[node] = Node::Split {
            dimension,
            value,
            below,
            above,
        };
        node
    }

    /// The two nearest points to `query`, examining at most `checks` points; `None`
    /// when the tree has fewer than two points.
    pub(super) fn nearest_two(&self, query: &Descriptor, checks: usize) -> Option<Nearest> {
        if self.points.len() < 2 {
            return None;
        }
        let mut best = (usize::MAX, f32::INFINITY);
        let mut second = f32::INFINITY;
        let mut examined = 0;
        // Branches left behind, nearest lower bound first.
        let mut queue = BinaryHeap::new();
        queue.push(Reverse((Bound(0.0), 0usize)));
        while let Some(Reverse((Bound(bound), mut node))) = queue.pop() {
            if bound >= second || examined >= checks {
                break;
            }
            loop {
                match self.nodes[node] {
                    Node::Leaf { start, end } => {
                        for &i in &self.order[start..end] {
                            let distance = squared_distance(query, &self.points[i], second);
                            if distance < best.1 {
                                second = best.1;
                                best = (i, distance);
                            } else if distance < second {
                                second = distance;
                            }
                        }
                        examined += end - start;
                        break;
                    }
                    Node::Split {
                        dimension,
                        value,
                        below,
                        above,
                    } => {
                        let difference = query[dimension] - value;
                        let (near, far) = if difference < 0.0 {
                            (below, above)
                        } else {
                            (above, below)
                        };
                        let far_bound = bound + difference * difference;
                        if far_bound < second {
                            queue.push(Reverse((Bound(far_bound), far)));
                        }
                        node = near;
                    }
                }
            }
        }
        (best.0 != usize::MAX && second.is_finite()).then_some(Nearest {
            index: best.0,
            distance: best.1,
            second,
        })
    }
}

/// Squared Euclidean distance, stopping early once it exceeds `limit`.
fn squared_distance(a: &Descriptor, b: &Descriptor, limit: f32) -> f32 {
    let mut sum = 0.0;
    for (chunk_a, chunk_b) in a.chunks_exact(16).zip(b.chunks_exact(16)) {
        sum += chunk_a
            .iter()
            .zip(chunk_b)
            .map(|(x, y)| (x - y) * (x - y))
            .sum::<f32>();
        if sum > limit {
            return sum;
        }
    }
    sum
}

/// A lower bound ordered for the search queue.
#[derive(Clone, Copy, PartialEq)]
struct Bound(f32);

impl Eq for Bound {}

impl PartialOrd for Bound {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Bound {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.total_cmp(&other.0)
    }
}
