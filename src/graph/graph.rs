use super::analysis::GraphAnalysis;
use super::coloring::{ColoringPlugin, LayerColors, build_origin_palette};
use super::graph_ui::{GraphUiPlugin, Parameters, Simulation};
use super::interaction::{DragState, Hovered, InteractionPlugin, Pinned};
use super::loading::{AdvisoriesData, LoadingPlugin, PackagesData};
use super::origin::Origin;
use super::package_info::{PackageInfoPlugin, Selection, SelectionInsight};
use super::quadtree::QuadTree;
use super::reduction::transitive_reduction;
use super::search::{Search, SearchPlugin};
use super::security::load_security;
use bevy::prelude::*;
use rand::Rng;
use std::collections::HashMap;

pub struct GraphPlugin;

pub(crate) use crate::model::Node;

#[derive(Component)]
struct Displacement(Vec3);

#[derive(Component)]
pub(super) struct Edge {
    pub from: Entity,
    pub to: Entity,
    /// Impliquée par un autre chemin (voir `reduction`).
    pub redundant: bool,
}

impl Edge {
    fn is_active(&self, params: &Parameters) -> bool {
        !(params.transitive_reduction && self.redundant)
    }
}

#[derive(Resource)]
struct EntityNameMap(HashMap<String, Entity>);

const EDGE_COLOR: Color = Color::srgba(1.0, 1.0, 1.0, 0.15);
const EDGE_DIMMED_COLOR: Color = Color::srgba(1.0, 1.0, 1.0, 0.04);
const DEPENDENCY_COLOR: Color = Color::srgb(1.0, 0.6, 0.2);
const DEPENDENT_COLOR: Color = Color::srgb(0.3, 0.8, 1.0);
const SELECTION_COLOR: Color = Color::srgb(1.0, 1.0, 0.3);
const ROOT_PATH_COLOR: Color = Color::srgb(1.0, 0.3, 0.9);
const SEARCH_COLOR: Color = Color::srgb(0.2, 0.9, 1.0);
const HOVER_COLOR: Color = Color::srgba(1.0, 1.0, 1.0, 0.7);
const PINNED_COLOR: Color = Color::srgba(1.0, 1.0, 1.0, 0.5);
/// Couleur des paquets racines (ceux dont rien ne dépend).
pub(super) const ROOT_COLOR: Color = Color::srgb(1.0, 0.84, 0.0);
const ROOT_SCALE: f32 = 1.6;

fn simulation_unlocked(simulation: Res<Simulation>) -> bool {
    !simulation.locked
}

/// Pas minimal (relatif à la longueur d'arête) : la simulation ne s'arrête jamais
/// d'elle-même et reste réactive, sans trembler une fois stabilisée.
const MIN_STEP: f32 = 1e-3;

impl Plugin for GraphPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            LoadingPlugin,
            GraphUiPlugin,
            PackageInfoPlugin,
            InteractionPlugin,
            ColoringPlugin,
            SearchPlugin,
        ));
        app.insert_resource(EntityNameMap(HashMap::new()));
        // Graphe reconstruit à chaque nouveau jeu de paquets (démarrage, glisser-déposer…),
        // index de sécurité recalculé aussi quand seuls les avis changent.
        app.add_systems(
            Update,
            (
                (clear_graph, spawn_nodes, spawn_edges, build_origin_palette)
                    .chain()
                    .run_if(resource_exists_and_changed::<PackagesData>),
                load_security.run_if(security_data_changed),
            )
                .chain()
                .in_set(RebuildGraph),
        );
        app.add_systems(
            Update,
            (
                (attraction, repulsion, center_force, apply_physics)
                    .chain()
                    .run_if(simulation_unlocked),
                draw_edges,
            )
                .chain()
                .after(RebuildGraph),
        );
    }
}

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct RebuildGraph;

/// Une seule condition (et pas `a.or(b)`) : `or` n'évalue pas `b` quand `a` est vrai,
/// `b` verrait alors le même changement à la frame suivante.
fn security_data_changed(
    packages: Option<Res<PackagesData>>,
    advisories: Option<Res<AdvisoriesData>>,
) -> bool {
    packages.as_ref().is_some_and(|p| p.is_changed())
        || (packages.is_some() && advisories.is_some_and(|a| a.is_changed()))
}

/// Supprime le graphe affiché et remet à zéro l'état qui en dépend.
#[allow(clippy::too_many_arguments)]
fn clear_graph(
    mut commands: Commands,
    nodes: Query<Entity, With<Node>>,
    edges: Query<Entity, With<Edge>>,
    mut cameras: Query<&mut Transform, (With<Camera2d>, Without<Node>)>,
    mut names_map: ResMut<EntityNameMap>,
    mut layer_colors: ResMut<LayerColors>,
    mut selection: ResMut<Selection>,
    mut insight: ResMut<SelectionInsight>,
    mut search: ResMut<Search>,
    mut hovered: ResMut<Hovered>,
    mut drag: ResMut<DragState>,
    mut simulation: ResMut<Simulation>,
    params: Res<Parameters>,
) {
    for entity in nodes.iter().chain(&edges) {
        commands.entity(entity).despawn();
    }
    for mut camera in &mut cameras {
        camera.translation = Vec3::new(0.0, 0.0, camera.translation.z);
    }
    names_map.0.clear();
    layer_colors.0.clear();
    *selection = default();
    *insight = default();
    *search = default();
    *hovered = default();
    *drag = default();
    simulation.reheat(&params);
}

fn spawn_nodes(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
    mut names_map: ResMut<EntityNameMap>,
    mut layer_colors: ResMut<LayerColors>,
    data: Res<PackagesData>,
    params: Res<Parameters>,
) {
    let nodes = data.nodes.clone();

    let shape = meshes.add(Circle::new(100.0));

    let mut rng = rand::rng();

    // Disque dont l'aire est proportionnelle au nombre de nœuds : chaque nœud
    // démarre à environ une longueur d'arête de ses voisins.
    // Bornée pour rester valable si l'attraction ou la répulsion vaut 0.
    let spawn_radius = (params.natural_length().max(params.k) * (nodes.len() as f32).sqrt() / 2.0)
        .min(params.max_diameter / 4.0);

    for node in nodes.into_iter() {
        // Couleur provisoire : `coloring::apply_colors` l'ajuste selon le mode choisi.
        let color = *layer_colors
            .0
            .entry(node.introduced_in.clone())
            .or_insert_with(|| Color::hsl(rng.random_range(0.0..360.0), 0.7, 0.5));
        let material = materials.add(color);
        let origin = Origin::classify(&node);

        let name = node.Name.clone();
        let id = commands
            .spawn((
                Mesh2d(shape.clone()),
                MeshMaterial2d(material),
                Transform::from_translation(
                    (Vec2::from_angle(rng.random_range(0.0..std::f32::consts::TAU))
                        * spawn_radius
                        * rng.random_range(0.0f32..1.0).sqrt())
                    .extend(1.0),
                ),
                node,
                origin,
                Displacement(Vec3::ZERO),
            ))
            .with_children(|parent| {
                parent.spawn((
                    Text2d::new(name.clone()),
                    TextFont {
                        font_size: 120.0,
                        ..default()
                    },
                    Transform::from_xyz(50.0, 50.0, 0.0),
                ));
            })
            .id();
        names_map.0.insert(name, id);
    }
}

fn spawn_edges(
    mut commands: Commands,
    mut nodes: Query<(Entity, &Node, &mut Transform)>,
    names_map: Res<EntityNameMap>,
) {
    let entities: Vec<Entity> = nodes.iter().map(|(e, ..)| e).collect();
    let index: HashMap<Entity, usize> = entities.iter().enumerate().map(|(i, e)| (*e, i)).collect();

    let mut edges = Vec::new();
    let mut n_unknown = 0;
    for (entity, node, ..) in &nodes {
        for dep in &node.dep {
            if *dep == node.Name {
                continue;
            }
            match names_map.0.get(dep) {
                Some(to) => edges.push((index[&entity], index[to])),
                None => n_unknown += 1,
            }
        }
    }
    edges.sort_unstable();
    edges.dedup();

    let redundant = transitive_reduction(entities.len(), &edges);
    let n_redundant = redundant.iter().filter(|r| **r).count();

    for (&(from, to), &redundant) in edges.iter().zip(&redundant) {
        commands.spawn(Edge {
            from: entities[from],
            to: entities[to],
            redundant,
        });
    }

    let sizes = entities
        .iter()
        .map(|e| {
            nodes
                .get(*e)
                .ok()
                .and_then(|(_, n, ..)| n.Size)
                .unwrap_or(0)
        })
        .collect();
    let analysis = GraphAnalysis::new(entities, &edges, &redundant, sizes);

    for (entity, _, mut transform) in &mut nodes {
        if analysis.is_root[analysis.index[&entity]] {
            transform.scale = Vec3::splat(ROOT_SCALE);
        }
    }

    info!(
        "Number of edges {} ({n_redundant} redundant, {n_unknown} unknown dependencies ignored), {} root packages",
        edges.len(),
        analysis.is_root.iter().filter(|r| **r).count()
    );
    commands.insert_resource(analysis);
}

fn draw_edges(
    mut gizmos: Gizmos,
    params: Res<Parameters>,
    selection: Res<Selection>,
    insight: Res<SelectionInsight>,
    hovered: Res<Hovered>,
    drag: Res<DragState>,
    search: Res<Search>,
    edges: Query<&Edge>,
    nodes: Query<&Transform, With<Node>>,
    pinned: Query<&Transform, (With<Node>, With<Pinned>)>,
) {
    // Résultats de recherche entourés (limité pour rester lisible et rapide).
    for tr in search
        .matches
        .iter()
        .take(500)
        .filter_map(|e| nodes.get(*e).ok())
    {
        gizmos.circle_2d(
            tr.translation.truncate(),
            100.0 * tr.scale.x + 45.0,
            SEARCH_COLOR,
        );
    }

    let selected = selection.entity;
    // Pendant un glisser, on met en évidence le nœud déplacé.
    let hovered = drag.entity.or(hovered.0);
    let touches_hovered = |edge: &Edge| hovered.is_some_and(|h| edge.from == h || edge.to == h);
    let on_root_path = |edge: &Edge| {
        insight
            .root_path
            .windows(2)
            .any(|w| w[0] == edge.from && w[1] == edge.to)
    };
    for edge in &edges {
        if !edge.is_active(&params) {
            continue;
        }
        let color = match selected {
            None => EDGE_COLOR,
            Some(_) if on_root_path(edge) => ROOT_PATH_COLOR,
            Some(s) if edge.from == s => DEPENDENCY_COLOR,
            Some(s) if edge.to == s => DEPENDENT_COLOR,
            _ if touches_hovered(edge) => HOVER_COLOR,
            Some(_) => EDGE_DIMMED_COLOR,
        };
        if let Ok([from, to]) = nodes.get_many([edge.from, edge.to]) {
            gizmos.line_2d(
                from.translation.truncate(),
                to.translation.truncate(),
                color,
            );
        }
    }

    if let Some(tr) = selected.and_then(|s| nodes.get(s).ok()) {
        gizmos.circle_2d(
            tr.translation.truncate(),
            100.0 * tr.scale.x + 60.0,
            SELECTION_COLOR,
        );
    }
    if let Some(tr) = hovered.and_then(|h| nodes.get(h).ok()) {
        gizmos.circle_2d(
            tr.translation.truncate(),
            100.0 * tr.scale.x + 30.0,
            HOVER_COLOR,
        );
    }
    for tr in &pinned {
        // Petit point central : nœud épinglé.
        gizmos.circle_2d(tr.translation.truncate(), 30.0 * tr.scale.x, PINNED_COLOR);
    }
}

fn repulsion(params: Res<Parameters>, mut query: Query<(&Transform, &mut Displacement)>) {
    let points: Vec<Vec2> = query
        .iter()
        .map(|(tr, _)| tr.translation.truncate())
        .collect();
    let tree = QuadTree::new(&points);

    let factor = params.repulsion * params.k.powi(2);
    let theta = params.theta;
    query.par_iter_mut().for_each(|(tr, mut disp)| {
        let force = tree.repulsion(tr.translation.truncate(), theta);
        disp.0 += (force * factor).extend(0.0);
    });
}

fn attraction(
    params: Res<Parameters>,
    edge_query: Query<&Edge>,
    mut node_query: Query<(&Transform, &mut Displacement)>,
) {
    edge_query
        .iter()
        .filter(|e| e.is_active(&params))
        .for_each(|edge| {
            if let Ok([(tr_a, mut disp_a), (tr_b, mut disp_b)]) =
                node_query.get_many_mut([edge.from, edge.to])
            {
                let dir = tr_a.translation - tr_b.translation;
                let step = params.attraction * dir.normalize_or_zero() * dir.length() / params.k;
                disp_a.0 -= step;
                disp_b.0 += step;
            }
        });
}

fn center_force(params: Res<Parameters>, mut node_query: Query<(&Transform, &mut Displacement)>) {
    node_query.iter_mut().for_each(|(transform, mut disp)| {
        disp.0 -= transform.translation * params.center * transform.translation.length();
    });
}

fn apply_physics(
    params: Res<Parameters>,
    mut simulation: ResMut<Simulation>,
    mut query: Query<(&mut Transform, &mut Displacement, Has<Pinned>)>,
) {
    let step = simulation.step;
    let mut energy = 0.0;
    let mut total_move = 0.0;
    let mut n = 0;

    query
        .iter_mut()
        .for_each(|(mut transform, mut disp, pinned)| {
            if pinned {
                disp.0 = Vec3::ZERO;
                return;
            }
            energy += disp.0.length_squared();
            let movement = disp.0.normalize_or_zero() * disp.0.length().min(step);
            transform.translation += movement;
            total_move += movement.length();
            n += 1;

            disp.0 = Vec3::ZERO;

            // Limit nodes into a circle
            let pos = transform.translation.truncate();
            let r = pos.length();
            if r > params.max_diameter / 2.0 {
                transform.translation = (pos / r * params.max_diameter / 2.0).extend(1.0);
            }
        });

    // Pas adaptatif (Yifan Hu, 2005) : on accélère tant que l'énergie baisse,
    // on ralentit dès qu'elle remonte (oscillation).
    if energy < simulation.energy {
        simulation.progress += 1;
        if simulation.progress >= 5 {
            simulation.progress = 0;
            simulation.step = (simulation.step / params.cooling).min(params.max_diameter / 10.0);
        }
    } else {
        simulation.progress = 0;
        simulation.step =
            (simulation.step * params.cooling).max(MIN_STEP * params.natural_length());
    }
    simulation.energy = energy;
    simulation.mean_move = total_move / n.max(1) as f32;
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::{Parameters, Simulation};
    use bevy::{
        log::LogPlugin,
        render::{RenderPlugin, settings::WgpuSettings},
        winit::WinitPlugin,
    };

    const ITER_UPDATE: usize = 100;

    macro_rules! bevy_two_nodes_graph_test {
        (
            params = { $($pfield:ident : $pvalue:expr),* $(,)? },
            update_systems = [ $( $update_sys:path ),* $(,)? ],
            new_dist $cmp:tt prev_dist $(,)?
        ) => {
            {
                let mut app = App::new();

                app.add_plugins(
                    DefaultPlugins
                        .set(RenderPlugin {
                            render_creation: WgpuSettings {
                                backends: None,
                                ..default()
                            }
                            .into(),
                            ..default()
                        })
                        // winit n'accepte qu'une boucle d'événements par processus,
                        // or les tests tournent en parallèle dans le même processus.
                        .disable::<WinitPlugin>()
                        .disable::<LogPlugin>(),
                )
                .insert_resource(PackagesData {
                    nodes: serde_json::from_str(
                        r#"
                           [
                               {
                                   "Name": "a",
                                   "dep": ["b"],
                                   "introduced_in": "0"
                               },
                               {
                                   "Name": "b",
                                   "dep": [],
                                   "introduced_in": "1"
                               }
                           ]
                        "#,
                    )
                    .unwrap(),
                    source: "test".into(),
                })
                .insert_resource(Parameters {
                    $($pfield : $pvalue),*,
                    ..default()
                })
                .insert_resource(Simulation::default())
                .insert_resource(EntityNameMap(HashMap::new()))
                .insert_resource(LayerColors::default())
                .add_systems(Startup, (spawn_nodes, spawn_edges).chain())
                .add_systems(Update, ( $($update_sys),* , apply_physics).chain());

                app.update();

                let translations: Vec<Vec3> = app
                    .world_mut()
                    .query_filtered::<&Transform, With<Node>>()
                    .iter(app.world())
                    .map(|&tr| tr.translation)
                    .collect();
                assert_eq!(translations.len(), 2);

                let mut prev_distance = translations[0].distance(translations[1]);

                for _ in 0..ITER_UPDATE {
                    app.update();

                    let translations: Vec<Vec3> = app
                        .world_mut()
                        .query_filtered::<&Transform, With<Node>>()
                        .iter(app.world())
                        .map(|&tr| tr.translation)
                        .collect();
                    let new_distance = translations[0].distance(translations[1]);

                    assert!(new_distance $cmp prev_distance);

                    prev_distance = new_distance;
                }
            }
        };
    }

    #[test]
    fn check_repulsion() {
        bevy_two_nodes_graph_test!(
            params = {repulsion: 100.0, attraction: 0.0, center: 0.0},
            update_systems = [repulsion],
            new_dist > prev_dist,
        );
    }

    #[test]
    fn check_attraction() {
        bevy_two_nodes_graph_test!(
            params = {repulsion: 0.0, attraction: 0.1, center: 0.0},
            update_systems = [attraction],
            new_dist < prev_dist,
        );
    }
}
