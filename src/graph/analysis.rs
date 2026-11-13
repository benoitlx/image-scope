//! Analyses du graphe de dépendances : paquets racines, chemin depuis une racine,
//! poids exclusif (arbre des dominateurs).
//!
//! Convention : une arête `(u, v)` signifie « u dépend de v ».

use super::reduction::strongly_connected_components;
use bevy::prelude::{Entity, Resource};
use petgraph::algo::dominators::simple_fast;
use petgraph::graph::{DiGraph, NodeIndex};
use std::collections::{HashMap, VecDeque};

/// Résultats d'analyse calculés une fois au chargement du graphe.
#[derive(Resource, Default)]
pub struct GraphAnalysis {
    pub entities: Vec<Entity>,
    pub index: HashMap<Entity, usize>,
    pub is_root: Vec<bool>,
    /// `dependencies[u]` : paquets dont `u` dépend (toutes les arêtes).
    pub dependencies: Vec<Vec<usize>>,
    /// `dependents_all[v]` : paquets qui dépendent de `v` (toutes les arêtes).
    dependents_all: Vec<Vec<usize>>,
    /// Idem, sans les arêtes redondantes (réduction transitive).
    dependents_reduced: Vec<Vec<usize>>,
    pub sizes: Vec<u64>,
    /// `(taille, nombre de paquets)` exclusifs de chaque nœud.
    pub exclusive: Vec<(u64, usize)>,
}

impl GraphAnalysis {
    pub fn new(
        entities: Vec<Entity>,
        edges: &[(usize, usize)],
        redundant: &[bool],
        sizes: Vec<u64>,
    ) -> Self {
        let n = entities.len();
        let index = entities.iter().enumerate().map(|(i, e)| (*e, i)).collect();
        let is_root = roots(n, edges);
        let mut dependencies = vec![Vec::new(); n];
        let mut dependents_all = vec![Vec::new(); n];
        let mut dependents_reduced = vec![Vec::new(); n];
        for (&(u, v), &r) in edges.iter().zip(redundant) {
            dependencies[u].push(v);
            dependents_all[v].push(u);
            if !r {
                dependents_reduced[v].push(u);
            }
        }
        let exclusive = exclusive_weights(n, edges, &is_root, &sizes);
        GraphAnalysis {
            entities,
            index,
            is_root,
            dependencies,
            dependents_all,
            dependents_reduced,
            sizes,
            exclusive,
        }
    }

    pub fn dependents(&self, reduced: bool) -> &[Vec<usize>] {
        if reduced {
            &self.dependents_reduced
        } else {
            &self.dependents_all
        }
    }
}

/// Paquets racines : ceux dont aucun autre paquet ne dépend.
///
/// Si plusieurs paquets dépendent les uns des autres en cycle et que rien d'extérieur
/// ne dépend d'eux, ils sont tous racines (composante source du graphe condensé).
pub fn roots(n: usize, edges: &[(usize, usize)]) -> Vec<bool> {
    let mut adj = vec![Vec::new(); n];
    for &(u, v) in edges {
        adj[u].push(v);
    }
    let (comp, n_comp) = strongly_connected_components(&adj);
    let mut has_dependent = vec![false; n_comp];
    for &(u, v) in edges {
        if comp[u] != comp[v] {
            has_dependent[comp[v]] = true;
        }
    }
    (0..n).map(|i| !has_dependent[comp[i]]).collect()
}

/// Poids exclusif de chaque paquet : lui-même plus tous les paquets qui ne sont
/// nécessaires qu'à travers lui (son sous-arbre dans l'arbre des dominateurs, avec une
/// racine virtuelle reliée à tous les paquets racines).
///
/// Renvoie `(taille cumulée, nombre de paquets)` pour chaque nœud.
pub fn exclusive_weights(
    n: usize,
    edges: &[(usize, usize)],
    is_root: &[bool],
    sizes: &[u64],
) -> Vec<(u64, usize)> {
    let mut graph = DiGraph::<(), ()>::with_capacity(n + 1, edges.len() + n);
    for _ in 0..=n {
        graph.add_node(());
    }
    let virtual_root = NodeIndex::new(n);
    for (i, _) in is_root.iter().enumerate().filter(|(_, r)| **r) {
        graph.add_edge(virtual_root, NodeIndex::new(i), ());
    }
    for &(u, v) in edges {
        if u != v {
            graph.add_edge(NodeIndex::new(u), NodeIndex::new(v), ());
        }
    }
    let dominators = simple_fast(&graph, virtual_root);

    // Enfants dans l'arbre des dominateurs.
    let mut children = vec![Vec::new(); n + 1];
    for i in 0..n {
        if let Some(idom) = dominators.immediate_dominator(NodeIndex::new(i)) {
            children[idom.index()].push(i);
        }
    }

    // Parcours en profondeur, puis accumulation des feuilles vers la racine.
    let mut order = Vec::with_capacity(n + 1);
    let mut stack = vec![n];
    while let Some(u) = stack.pop() {
        order.push(u);
        stack.extend(&children[u]);
    }
    let mut weights = vec![(0u64, 0usize); n + 1];
    for &u in order.iter().rev() {
        let mut w = if u < n { (sizes[u], 1) } else { (0, 0) };
        for &c in &children[u] {
            w.0 += weights[c].0;
            w.1 += weights[c].1;
        }
        weights[u] = w;
    }
    weights.truncate(n);
    weights
}

/// Plus court chemin d'un paquet racine jusqu'à `start`, en remontant `dependents`
/// (`dependents[v]` = paquets qui dépendent de `v`).
///
/// Renvoie le chemin (racine en premier, `start` en dernier) et toutes les racines
/// qui tirent `start`, directement ou non.
pub fn path_from_root(
    dependents: &[Vec<usize>],
    is_root: &[bool],
    start: usize,
) -> (Vec<usize>, Vec<usize>) {
    let mut parent = vec![usize::MAX; dependents.len()];
    let mut seen = vec![false; dependents.len()];
    let mut queue = VecDeque::from([start]);
    seen[start] = true;
    let mut nearest_root = None;
    let mut roots = Vec::new();

    while let Some(u) = queue.pop_front() {
        if is_root[u] {
            nearest_root.get_or_insert(u);
            roots.push(u);
        }
        for &d in &dependents[u] {
            if !seen[d] {
                seen[d] = true;
                parent[d] = u;
                queue.push_back(d);
            }
        }
    }

    let mut path = Vec::new();
    let mut current = nearest_root;
    while let Some(u) = current {
        path.push(u);
        current = (u != start).then(|| parent[u]);
    }
    (path, roots)
}

/// Taille et nombre de paquets de `start` et de toutes ses dépendances (transitives).
pub fn closure_weight(dependencies: &[Vec<usize>], sizes: &[u64], start: usize) -> (u64, usize) {
    let mut seen = vec![false; dependencies.len()];
    let mut stack = vec![start];
    seen[start] = true;
    let mut weight = (0, 0);
    while let Some(u) = stack.pop() {
        weight.0 += sizes[u];
        weight.1 += 1;
        for &v in &dependencies[u] {
            if !seen[v] {
                seen[v] = true;
                stack.push(v);
            }
        }
    }
    weight
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reverse(n: usize, edges: &[(usize, usize)]) -> Vec<Vec<usize>> {
        let mut r = vec![Vec::new(); n];
        for &(u, v) in edges {
            r[v].push(u);
        }
        r
    }

    fn forward(n: usize, edges: &[(usize, usize)]) -> Vec<Vec<usize>> {
        let mut f = vec![Vec::new(); n];
        for &(u, v) in edges {
            f[u].push(v);
        }
        f
    }

    // 0 et 1 sont des racines ; 0 → 2 → 4, 1 → 3 → 4, 0 → 5
    const EDGES: [(usize, usize); 5] = [(0, 2), (2, 4), (1, 3), (3, 4), (0, 5)];

    #[test]
    fn finds_roots() {
        assert_eq!(
            roots(6, &EDGES),
            vec![true, true, false, false, false, false]
        );
    }

    #[test]
    fn cycle_without_dependents_is_root() {
        // 0 ↔ 1, 1 → 2
        assert_eq!(roots(3, &[(0, 1), (1, 0), (1, 2)]), vec![true, true, false]);
    }

    #[test]
    fn exclusive_weight_excludes_shared_dependencies() {
        let sizes = [1, 10, 100, 1000, 10000, 100000];
        let w = exclusive_weights(6, &EDGES, &roots(6, &EDGES), &sizes);
        // 4 est partagé entre 0 et 1 : il n'appartient à aucun des deux
        assert_eq!(w[0], (1 + 100 + 100000, 3));
        assert_eq!(w[1], (10 + 1000, 2));
        assert_eq!(w[4], (10000, 1));
    }

    #[test]
    fn path_goes_to_nearest_root() {
        let is_root = roots(6, &EDGES);
        let (path, roots) = path_from_root(&reverse(6, &EDGES), &is_root, 4);
        assert!(path == vec![0, 2, 4] || path == vec![1, 3, 4]);
        let mut roots = roots;
        roots.sort();
        assert_eq!(roots, vec![0, 1]);
    }

    #[test]
    fn path_of_root_is_itself() {
        let is_root = roots(6, &EDGES);
        assert_eq!(
            path_from_root(&reverse(6, &EDGES), &is_root, 0),
            (vec![0], vec![0])
        );
    }

    #[test]
    fn closure_counts_each_package_once() {
        let sizes = [1, 10, 100, 1000, 10000, 100000];
        assert_eq!(closure_weight(&forward(6, &EDGES), &sizes, 0), (110101, 4));
    }
}
