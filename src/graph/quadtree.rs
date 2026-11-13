//! Quadtree de Barnes-Hut pour approximer la répulsion entre nœuds en O(n log n).

use bevy::math::Vec2;

const EMPTY: u32 = u32::MAX;
/// Évite une récursion infinie quand plusieurs points sont confondus.
const MAX_DEPTH: u32 = 24;

struct Cell {
    center: Vec2,
    half: f32,
    mass: f32,
    com: Vec2,
    children: [u32; 4],
}

impl Cell {
    fn is_leaf(&self) -> bool {
        self.children == [EMPTY; 4]
    }

    fn contains(&self, p: Vec2) -> bool {
        (p - self.center).abs().max_element() <= self.half
    }
}

pub struct QuadTree {
    cells: Vec<Cell>,
}

impl QuadTree {
    pub fn new(points: &[Vec2]) -> Self {
        let mut tree = QuadTree {
            cells: Vec::with_capacity(points.len() * 2),
        };
        if points.is_empty() {
            return tree;
        }

        let (min, max) = points.iter().fold(
            (Vec2::splat(f32::INFINITY), Vec2::splat(f32::NEG_INFINITY)),
            |(lo, hi), p| (lo.min(*p), hi.max(*p)),
        );
        let center = (min + max) / 2.0;
        let half = ((max - min).max_element() / 2.0).max(1.0);

        let mut pts = points.to_vec();
        tree.build(&mut pts, center, half, 0);
        tree
    }

    fn build(&mut self, pts: &mut [Vec2], center: Vec2, half: f32, depth: u32) -> u32 {
        let idx = self.cells.len() as u32;
        let mass = pts.len() as f32;
        let com = pts.iter().copied().sum::<Vec2>() / mass;
        self.cells.push(Cell {
            center,
            half,
            mass,
            com,
            children: [EMPTY; 4],
        });

        if pts.len() == 1 || depth >= MAX_DEPTH {
            return idx;
        }

        let mid_x = partition(pts, |p| p.x < center.x);
        let (left, right) = pts.split_at_mut(mid_x);
        let mid_l = partition(left, |p| p.y < center.y);
        let mid_r = partition(right, |p| p.y < center.y);
        let (bottom_left, top_left) = left.split_at_mut(mid_l);
        let (bottom_right, top_right) = right.split_at_mut(mid_r);

        let q = half / 2.0;
        let quadrants = [
            (bottom_left, Vec2::new(-q, -q)),
            (top_left, Vec2::new(-q, q)),
            (bottom_right, Vec2::new(q, -q)),
            (top_right, Vec2::new(q, q)),
        ];

        let mut children = [EMPTY; 4];
        for (i, (slice, offset)) in quadrants.into_iter().enumerate() {
            if !slice.is_empty() {
                children[i] = self.build(slice, center + offset, q, depth + 1);
            }
        }
        self.cells[idx as usize].children = children;
        idx
    }

    /// Somme approximée de `mass * (p - com) / |p - com|²` sur tous les autres points.
    ///
    /// `theta` règle la précision : 0 = calcul exact, ~1 = rapide mais grossier.
    pub fn repulsion(&self, p: Vec2, theta: f32) -> Vec2 {
        let mut force = Vec2::ZERO;
        if self.cells.is_empty() {
            return force;
        }

        let theta2 = theta * theta;
        let mut stack: Vec<u32> = Vec::with_capacity(64);
        stack.push(0);

        while let Some(i) = stack.pop() {
            let cell = &self.cells[i as usize];
            let d = p - cell.com;
            let dist2 = d.length_squared();

            let far_enough = !cell.contains(p) && (2.0 * cell.half).powi(2) < theta2 * dist2;
            if cell.is_leaf() || far_enough {
                // dist2 ~ 0 : c'est le point lui-même (ou un point confondu)
                if dist2 > 1e-6 {
                    force += d * cell.mass / dist2;
                }
            } else {
                stack.extend(cell.children.iter().copied().filter(|&c| c != EMPTY));
            }
        }
        force
    }
}

fn partition(pts: &mut [Vec2], pred: impl Fn(&Vec2) -> bool) -> usize {
    let mut i = 0;
    for j in 0..pts.len() {
        if pred(&pts[j]) {
            pts.swap(i, j);
            i += 1;
        }
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::Rng;

    fn brute_force(points: &[Vec2], p: Vec2) -> Vec2 {
        points
            .iter()
            .map(|q| p - *q)
            .filter(|d| d.length_squared() > 1e-6)
            .map(|d| d / d.length_squared())
            .sum()
    }

    fn random_points(n: usize) -> Vec<Vec2> {
        let mut rng = rand::rng();
        (0..n)
            .map(|_| {
                Vec2::new(
                    rng.random_range(-1000.0..1000.0),
                    rng.random_range(-1000.0..1000.0),
                )
            })
            .collect()
    }

    #[test]
    fn exact_when_theta_is_zero() {
        let points = random_points(500);
        let tree = QuadTree::new(&points);
        for p in &points {
            let expected = brute_force(&points, *p);
            let got = tree.repulsion(*p, 0.0);
            assert!((expected - got).length() <= 1e-4 * expected.length().max(1e-3));
        }
    }

    #[test]
    fn close_to_exact_with_default_theta() {
        let points = random_points(2000);
        let tree = QuadTree::new(&points);
        let mut total_err = 0.0;
        let mut total = 0.0;
        for p in &points {
            let expected = brute_force(&points, *p);
            total_err += (expected - tree.repulsion(*p, 0.9)).length();
            total += expected.length();
        }
        assert!(
            total_err / total < 0.05,
            "relative error {}",
            total_err / total
        );
    }

    #[test]
    fn handles_duplicate_points() {
        let points = vec![Vec2::ZERO; 10];
        let tree = QuadTree::new(&points);
        assert_eq!(tree.repulsion(Vec2::ZERO, 0.9), Vec2::ZERO);
    }
}
