//! Réduction transitive d'un graphe orienté pouvant contenir des cycles.
//!
//! Une arête `u → v` est redondante si `v` reste atteignable depuis `u` sans elle
//! (par exemple `a → c` quand `a → b → c` existe déjà).
//!
//! Les dépendances RPM contiennent des cycles (`glibc` ↔ `glibc-common`, …). Dans un
//! graphe cyclique, retirer simultanément toutes les arêtes « contournables » peut
//! casser l'atteignabilité. On condense donc d'abord les composantes fortement
//! connexes (Tarjan), on réduit le DAG obtenu, puis on remappe :
//! - les arêtes internes à une composante sont conservées ;
//! - entre deux composantes, une seule arête d'origine est conservée par arête
//!   non redondante du DAG.

const UNVISITED: usize = usize::MAX;

/// Renvoie, pour chaque arête de `edges` (indices dans `0..n`), si elle est redondante.
pub fn transitive_reduction(n: usize, edges: &[(usize, usize)]) -> Vec<bool> {
    let mut adj = vec![Vec::new(); n];
    for &(u, v) in edges {
        adj[u].push(v);
    }
    let (comp, n_comp) = strongly_connected_components(&adj);

    // Arêtes du DAG condensé (dédupliquées).
    let mut dag: Vec<Vec<usize>> = vec![Vec::new(); n_comp];
    for &(u, v) in edges {
        let (cu, cv) = (comp[u], comp[v]);
        if cu != cv {
            dag[cu].push(cv);
        }
    }
    for succ in &mut dag {
        succ.sort_unstable();
        succ.dedup();
    }

    // Tarjan numérote les composantes puits en premier : tout successeur d'une
    // composante `c` a un indice < c, on peut donc calculer l'atteignabilité dans l'ordre.
    let words = n_comp.div_ceil(64);
    let mut reach = vec![vec![0u64; words]; n_comp];
    for c in 0..n_comp {
        let mut r = vec![0u64; words];
        for &d in &dag[c] {
            debug_assert!(d < c);
            set(&mut r, d);
            for (w, x) in r.iter_mut().zip(&reach[d]) {
                *w |= x;
            }
        }
        reach[c] = r;
    }

    // c → d est redondante si d est atteignable depuis un autre successeur de c.
    let is_redundant_dag_edge = |c: usize, d: usize| {
        dag[c]
            .iter()
            .any(|&other| other != d && get(&reach[other], d))
    };

    let mut kept_between: std::collections::HashSet<(usize, usize)> = Default::default();
    edges
        .iter()
        .map(|&(u, v)| {
            let (cu, cv) = (comp[u], comp[v]);
            if u == v {
                true
            } else if cu == cv {
                false
            } else if is_redundant_dag_edge(cu, cv) {
                true
            } else {
                // Plusieurs arêtes d'origine peuvent relier les deux mêmes composantes :
                // une seule suffit.
                !kept_between.insert((cu, cv))
            }
        })
        .collect()
}

fn set(bits: &mut [u64], i: usize) {
    bits[i / 64] |= 1 << (i % 64);
}

fn get(bits: &[u64], i: usize) -> bool {
    bits[i / 64] & (1 << (i % 64)) != 0
}

/// Tarjan itératif. Renvoie la composante de chaque nœud et le nombre de composantes,
/// numérotées dans l'ordre topologique inverse (puits d'abord).
pub(super) fn strongly_connected_components(adj: &[Vec<usize>]) -> (Vec<usize>, usize) {
    let n = adj.len();
    let mut index = vec![UNVISITED; n];
    let mut low = vec![0; n];
    let mut on_stack = vec![false; n];
    let mut comp = vec![UNVISITED; n];
    let mut stack = Vec::new();
    let mut call: Vec<(usize, usize)> = Vec::new();
    let mut next_index = 0;
    let mut n_comp = 0;

    for start in 0..n {
        if index[start] != UNVISITED {
            continue;
        }
        index[start] = next_index;
        low[start] = next_index;
        next_index += 1;
        stack.push(start);
        on_stack[start] = true;
        call.push((start, 0));

        while let Some(top) = call.last_mut() {
            let (v, i) = *top;
            if i < adj[v].len() {
                top.1 += 1;
                let w = adj[v][i];
                if index[w] == UNVISITED {
                    index[w] = next_index;
                    low[w] = next_index;
                    next_index += 1;
                    stack.push(w);
                    on_stack[w] = true;
                    call.push((w, 0));
                } else if on_stack[w] {
                    low[v] = low[v].min(index[w]);
                }
            } else {
                call.pop();
                if let Some(&(parent, _)) = call.last() {
                    low[parent] = low[parent].min(low[v]);
                }
                if low[v] == index[v] {
                    loop {
                        let w = stack.pop().unwrap();
                        on_stack[w] = false;
                        comp[w] = n_comp;
                        if w == v {
                            break;
                        }
                    }
                    n_comp += 1;
                }
            }
        }
    }
    (comp, n_comp)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kept(n: usize, edges: &[(usize, usize)]) -> Vec<(usize, usize)> {
        transitive_reduction(n, edges)
            .into_iter()
            .zip(edges)
            .filter(|(r, _)| !r)
            .map(|(_, e)| *e)
            .collect()
    }

    /// Atteignabilité par parcours, pour vérifier que la réduction la préserve.
    fn closure(n: usize, edges: &[(usize, usize)]) -> Vec<Vec<bool>> {
        (0..n)
            .map(|s| {
                let mut seen = vec![false; n];
                let mut todo = vec![s];
                while let Some(u) = todo.pop() {
                    for &(a, b) in edges {
                        if a == u && !seen[b] {
                            seen[b] = true;
                            todo.push(b);
                        }
                    }
                }
                seen
            })
            .collect()
    }

    #[test]
    fn removes_shortcut() {
        // a→b→c et a→c : a→c est redondante
        assert_eq!(kept(3, &[(0, 1), (1, 2), (0, 2)]), vec![(0, 1), (1, 2)]);
    }

    #[test]
    fn keeps_diamond() {
        let edges = [(0, 1), (0, 2), (1, 3), (2, 3)];
        assert_eq!(kept(4, &edges), edges.to_vec());
    }

    #[test]
    fn removes_long_shortcut() {
        // x→d redondante via x→a→b→d
        assert_eq!(
            kept(4, &[(0, 1), (1, 2), (2, 3), (0, 3)]),
            vec![(0, 1), (1, 2), (2, 3)]
        );
    }

    #[test]
    fn cycle_keeps_reachability() {
        // a↔b, a→c, b→c : une seule des deux arêtes vers c doit rester
        let edges = [(0, 1), (1, 0), (0, 2), (1, 2)];
        let k = kept(3, &edges);
        assert_eq!(k.len(), 3);
        assert_eq!(closure(3, &k), closure(3, &edges));
    }

    #[test]
    fn self_loop_is_redundant() {
        assert_eq!(kept(2, &[(0, 0), (0, 1)]), vec![(0, 1)]);
    }

    #[test]
    fn random_graphs_preserve_reachability() {
        use rand::Rng;
        let mut rng = rand::rng();
        for _ in 0..200 {
            let n = rng.random_range(1..15);
            let m = rng.random_range(0..40);
            let edges: Vec<_> = (0..m)
                .map(|_| (rng.random_range(0..n), rng.random_range(0..n)))
                .collect();
            let k = kept(n, &edges);
            // Les boucles sur soi-même sont volontairement supprimées.
            let without_self_loops: Vec<_> =
                edges.iter().copied().filter(|(a, b)| a != b).collect();
            assert_eq!(
                closure(n, &k),
                closure(n, &without_self_loops),
                "edges {edges:?}"
            );
        }
    }
}
