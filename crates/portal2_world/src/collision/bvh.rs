//! Bounding volume hierarchy over static collision volumes.

use bevy::math::Vec3;

#[derive(Clone, Copy, Debug)]
struct Node {
    mins: Vec3,
    maxs: Vec3,
    /// Leaf: first index into `items`; interior: index of the left child
    /// (the right child follows it).
    first: u32,
    /// Number of items for leaves, 0 for interior nodes.
    count: u32,
}

#[derive(Clone, Debug, Default)]
pub struct Bvh {
    nodes: Vec<Node>,
    items: Vec<u32>,
}

const LEAF_SIZE: usize = 4;

impl Bvh {
    pub fn build(bounds: &[(Vec3, Vec3)]) -> Self {
        let mut bvh = Bvh { nodes: Vec::with_capacity(bounds.len() * 2), items: (0..bounds.len() as u32).collect() };
        if bounds.is_empty() {
            return bvh;
        }
        bvh.nodes.push(Node { mins: Vec3::ZERO, maxs: Vec3::ZERO, first: 0, count: 0 });
        bvh.subdivide(0, 0, bounds.len(), bounds);
        bvh
    }

    fn subdivide(&mut self, node: usize, start: usize, end: usize, bounds: &[(Vec3, Vec3)]) {
        let mut mins = Vec3::splat(f32::MAX);
        let mut maxs = Vec3::splat(f32::MIN);
        let mut cmin = Vec3::splat(f32::MAX);
        let mut cmax = Vec3::splat(f32::MIN);
        for &i in &self.items[start..end] {
            let (a, b) = bounds[i as usize];
            mins = mins.min(a);
            maxs = maxs.max(b);
            let c = (a + b) * 0.5;
            cmin = cmin.min(c);
            cmax = cmax.max(c);
        }
        self.nodes[node].mins = mins;
        self.nodes[node].maxs = maxs;
        let count = end - start;
        let extent = cmax - cmin;
        if count <= LEAF_SIZE || extent.max_element() < 1e-3 {
            self.nodes[node].first = start as u32;
            self.nodes[node].count = count as u32;
            return;
        }
        let axis = if extent.x >= extent.y && extent.x >= extent.z {
            0
        } else if extent.y >= extent.z {
            1
        } else {
            2
        };
        let slice = &mut self.items[start..end];
        let mid = count / 2;
        slice.select_nth_unstable_by(mid, |&a, &b| {
            let ca = bounds[a as usize].0[axis] + bounds[a as usize].1[axis];
            let cb = bounds[b as usize].0[axis] + bounds[b as usize].1[axis];
            ca.total_cmp(&cb)
        });
        let left = self.nodes.len();
        self.nodes.push(Node { mins: Vec3::ZERO, maxs: Vec3::ZERO, first: 0, count: 0 });
        self.nodes.push(Node { mins: Vec3::ZERO, maxs: Vec3::ZERO, first: 0, count: 0 });
        self.nodes[node].first = left as u32;
        self.nodes[node].count = 0;
        self.subdivide(left, start, start + mid, bounds);
        self.subdivide(left + 1, start + mid, end, bounds);
    }

    /// Visits items whose bounds the box `[mins, maxs]` may touch while moving
    /// from `start` to `end`.
    pub fn query_sweep(&self, start: Vec3, end: Vec3, mins: Vec3, maxs: Vec3, mut visit: impl FnMut(u32)) {
        if self.nodes.is_empty() {
            return;
        }
        let delta = end - start;
        let mut stack = vec![0usize];
        while let Some(n) = stack.pop() {
            let node = &self.nodes[n];
            // Expand the node by the box so the box becomes a point.
            let lo = node.mins - maxs - Vec3::splat(0.1);
            let hi = node.maxs - mins + Vec3::splat(0.1);
            if !segment_hits_aabb(start, delta, lo, hi) {
                continue;
            }
            if node.count > 0 {
                for &i in &self.items[node.first as usize..(node.first + node.count) as usize] {
                    visit(i);
                }
            } else {
                stack.push(node.first as usize);
                stack.push(node.first as usize + 1);
            }
        }
    }

    /// Visits items whose bounds overlap `[mins, maxs]`.
    pub fn query_aabb(&self, mins: Vec3, maxs: Vec3, mut visit: impl FnMut(u32)) {
        self.query_sweep((mins + maxs) * 0.5, (mins + maxs) * 0.5, (mins - maxs) * 0.5, (maxs - mins) * 0.5, &mut visit);
    }
}

pub fn segment_hits_aabb(start: Vec3, delta: Vec3, lo: Vec3, hi: Vec3) -> bool {
    let mut t0 = 0.0f32;
    let mut t1 = 1.0f32;
    for a in 0..3 {
        if delta[a].abs() < 1e-9 {
            if start[a] < lo[a] || start[a] > hi[a] {
                return false;
            }
            continue;
        }
        let inv = 1.0 / delta[a];
        let (mut ta, mut tb) = ((lo[a] - start[a]) * inv, (hi[a] - start[a]) * inv);
        if ta > tb {
            std::mem::swap(&mut ta, &mut tb);
        }
        t0 = t0.max(ta);
        t1 = t1.min(tb);
        if t0 > t1 {
            return false;
        }
    }
    true
}
