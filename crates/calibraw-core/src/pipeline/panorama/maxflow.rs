//! Minimum s-t cut by the Boykov–Kolmogorov max-flow algorithm ("An Experimental
//! Comparison of Min-Cut/Max-Flow Algorithms for Energy Minimization in Vision", PAMI
//! 2004). Ported from OpenCV's `GCGraph` (`modules/imgproc/src/gcgraph.hpp`), with
//! indices in place of its vertex pointers and a queue with an in-queue flag in place of
//! its intrusive active list.
//!
//! Copyright (C) 2000, Intel Corporation, all rights reserved. Used under the Intel
//! License Agreement for the Open Source Computer Vision Library (3-clause BSD),
//! reproduced in `THIRD_PARTY_NOTICES.md`.

use std::collections::VecDeque;

/// A vertex's parent: an edge index, or one of these.
const NO_PARENT: i32 = 0;
const TERMINAL: i32 = -1;
const ORPHAN: i32 = -2;

#[derive(Clone, Copy, Default)]
struct Vertex {
    /// First outgoing edge, 0 for none.
    first: usize,
    parent: i32,
    /// Time stamp and distance to the terminal, for the tree restoration heuristic.
    ts: u32,
    dist: u32,
    /// Residual terminal capacity: positive towards the source, negative towards the
    /// sink.
    weight: f32,
    /// Which tree the vertex belongs to: false source, true sink.
    sink: bool,
    queued: bool,
}

#[derive(Clone, Copy)]
struct Edge {
    destination: usize,
    next: usize,
    weight: f32,
}

pub(super) struct Graph {
    vertices: Vec<Vertex>,
    /// Edges in pairs: `e` and `e ^ 1` are each other's reverse. Entries 0 and 1 are
    /// unused, so 0 can mean "none".
    edges: Vec<Edge>,
}

impl Graph {
    pub(super) fn new(vertices: usize, edges: usize) -> Self {
        let mut graph = Self {
            vertices: vec![Vertex::default(); vertices],
            edges: Vec::with_capacity(2 * edges + 2),
        };
        let unused = Edge {
            destination: 0,
            next: 0,
            weight: 0.0,
        };
        graph.edges.extend([unused, unused]);
        graph
    }

    /// Adds the edge `i → j` of capacity `forward` and `j → i` of capacity `backward`.
    pub(super) fn add_edges(&mut self, i: usize, j: usize, forward: f32, backward: f32) {
        debug_assert!(i != j && forward >= 0.0 && backward >= 0.0);
        let index = self.edges.len();
        self.edges.push(Edge {
            destination: j,
            next: self.vertices[i].first,
            weight: forward,
        });
        self.vertices[i].first = index;
        self.edges.push(Edge {
            destination: i,
            next: self.vertices[j].first,
            weight: backward,
        });
        self.vertices[j].first = index + 1;
    }

    /// Adds capacities from the source and to the sink of vertex `i`.
    pub(super) fn add_terminal_weights(&mut self, i: usize, source: f32, sink: f32) {
        let (mut source, mut sink) = (source, sink);
        let existing = self.vertices[i].weight;
        if existing > 0.0 {
            source += existing;
        } else {
            sink -= existing;
        }
        self.vertices[i].weight = source - sink;
    }

    /// Whether vertex `i` ends on the source side of the minimum cut, after
    /// [`Self::max_flow`].
    pub(super) fn in_source_segment(&self, i: usize) -> bool {
        !self.vertices[i].sink
    }

    pub(super) fn max_flow(&mut self) {
        let mut active: VecDeque<usize> = VecDeque::new();
        let mut orphans: Vec<usize> = Vec::new();
        let mut current_ts = 0u32;
        for (i, v) in self.vertices.iter_mut().enumerate() {
            v.ts = 0;
            if v.weight != 0.0 {
                active.push_back(i);
                v.queued = true;
                v.dist = 1;
                v.parent = TERMINAL;
                v.sink = v.weight < 0.0;
            } else {
                v.parent = NO_PARENT;
            }
        }
        loop {
            // Grow the source and sink trees until an edge connects them.
            let mut connecting = 0usize;
            while let Some(&v) = active.front() {
                if self.vertices[v].parent != NO_PARENT {
                    let side = usize::from(self.vertices[v].sink);
                    let mut e = self.vertices[v].first;
                    while e != 0 {
                        if self.edges[e ^ side].weight != 0.0 {
                            let u = self.edges[e].destination;
                            if self.vertices[u].parent == NO_PARENT {
                                let (ts, dist) = (self.vertices[v].ts, self.vertices[v].dist);
                                let vertex = &mut self.vertices[u];
                                vertex.sink = side == 1;
                                vertex.parent = (e ^ 1) as i32;
                                vertex.ts = ts;
                                vertex.dist = dist + 1;
                                if !vertex.queued {
                                    vertex.queued = true;
                                    active.push_back(u);
                                }
                            } else if usize::from(self.vertices[u].sink) != side {
                                connecting = e ^ side;
                                break;
                            } else if self.vertices[u].dist > self.vertices[v].dist + 1
                                && self.vertices[u].ts <= self.vertices[v].ts
                            {
                                let (ts, dist) = (self.vertices[v].ts, self.vertices[v].dist);
                                let vertex = &mut self.vertices[u];
                                vertex.parent = (e ^ 1) as i32;
                                vertex.ts = ts;
                                vertex.dist = dist + 1;
                            }
                        }
                        e = self.edges[e].next;
                    }
                    if connecting > 0 {
                        break;
                    }
                }
                active.pop_front();
                self.vertices[v].queued = false;
            }
            if connecting == 0 {
                break;
            }
            // The bottleneck along the path through the connecting edge.
            let mut bottleneck = self.edges[connecting].weight;
            for k in [1usize, 0] {
                let mut v = self.edges[connecting ^ k].destination;
                loop {
                    let parent = self.vertices[v].parent;
                    if parent < 0 {
                        break;
                    }
                    bottleneck = bottleneck.min(self.edges[parent as usize ^ k].weight);
                    v = self.edges[parent as usize].destination;
                }
                bottleneck = bottleneck.min(self.vertices[v].weight.abs());
            }
            debug_assert!(bottleneck > 0.0);
            // Push the bottleneck along the path and collect the saturated vertices.
            self.edges[connecting].weight -= bottleneck;
            self.edges[connecting ^ 1].weight += bottleneck;
            for k in [1usize, 0] {
                let mut v = self.edges[connecting ^ k].destination;
                loop {
                    let parent = self.vertices[v].parent;
                    if parent < 0 {
                        break;
                    }
                    let parent = parent as usize;
                    self.edges[parent ^ (k ^ 1)].weight += bottleneck;
                    self.edges[parent ^ k].weight -= bottleneck;
                    if self.edges[parent ^ k].weight == 0.0 {
                        orphans.push(v);
                        self.vertices[v].parent = ORPHAN;
                    }
                    v = self.edges[parent].destination;
                }
                let sign = if k == 1 { -1.0 } else { 1.0 };
                self.vertices[v].weight += bottleneck * sign;
                if self.vertices[v].weight == 0.0 {
                    orphans.push(v);
                    self.vertices[v].parent = ORPHAN;
                }
            }
            // Restore the trees: find new parents for the orphans.
            current_ts += 1;
            while let Some(orphan) = orphans.pop() {
                self.adopt(orphan, current_ts, &mut orphans, &mut active);
            }
        }
    }

    /// Gives `orphan` a new parent in its tree, or frees it and orphans its children.
    fn adopt(
        &mut self,
        orphan: usize,
        current_ts: u32,
        orphans: &mut Vec<usize>,
        active: &mut VecDeque<usize>,
    ) {
        let side = usize::from(self.vertices[orphan].sink);
        let mut best_edge = 0usize;
        let mut best_dist = u32::MAX;
        let mut e = self.vertices[orphan].first;
        while e != 0 {
            let next = self.edges[e].next;
            if self.edges[e ^ (side ^ 1)].weight == 0.0 {
                e = next;
                continue;
            }
            let start = self.edges[e].destination;
            if usize::from(self.vertices[start].sink) != side
                || self.vertices[start].parent == NO_PARENT
            {
                e = next;
                continue;
            }
            // Distance from `start` to its tree's terminal.
            let mut u = start;
            let mut d = 0u32;
            loop {
                if self.vertices[u].ts == current_ts {
                    d = d.saturating_add(self.vertices[u].dist);
                    break;
                }
                let parent = self.vertices[u].parent;
                d = d.saturating_add(1);
                if parent < 0 {
                    if parent == ORPHAN {
                        d = u32::MAX - 1;
                    } else {
                        self.vertices[u].ts = current_ts;
                        self.vertices[u].dist = 1;
                    }
                    break;
                }
                u = self.edges[parent as usize].destination;
            }
            d = d.saturating_add(1);
            if d < u32::MAX {
                if d < best_dist {
                    best_dist = d;
                    best_edge = e;
                }
                // Stamp the path's distances so later searches stop early.
                let mut u = start;
                while self.vertices[u].ts != current_ts {
                    self.vertices[u].ts = current_ts;
                    d -= 1;
                    self.vertices[u].dist = d;
                    let parent = self.vertices[u].parent;
                    if parent <= 0 {
                        break;
                    }
                    u = self.edges[parent as usize].destination;
                }
            }
            e = next;
        }
        if best_edge > 0 {
            let vertex = &mut self.vertices[orphan];
            vertex.parent = best_edge as i32;
            vertex.ts = current_ts;
            vertex.dist = best_dist;
            return;
        }
        // No parent: the orphan leaves its tree, its children become orphans and its
        // neighbours in the tree become active again.
        self.vertices[orphan].ts = 0;
        self.vertices[orphan].parent = NO_PARENT;
        let mut e = self.vertices[orphan].first;
        while e != 0 {
            let u = self.edges[e].destination;
            let parent = self.vertices[u].parent;
            if usize::from(self.vertices[u].sink) == side && parent != NO_PARENT {
                if self.edges[e ^ (side ^ 1)].weight != 0.0 && !self.vertices[u].queued {
                    self.vertices[u].queued = true;
                    active.push_back(u);
                }
                if parent > 0 && self.edges[parent as usize].destination == orphan {
                    orphans.push(u);
                    self.vertices[u].parent = ORPHAN;
                }
            }
            e = self.edges[e].next;
        }
    }
}
