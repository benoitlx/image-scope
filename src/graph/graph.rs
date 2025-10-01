use super::graph_ui::{GraphUiPlugin, Parameters};
use bevy::prelude::*;
use rand::Rng;
use serde::Deserialize;
use std::collections::HashMap;
use std::io::BufReader;

pub struct GraphPlugin;

#[derive(Component, Deserialize, Clone, Debug)]
#[allow(non_snake_case)]
struct Node {
    Name: String,
    // Version: String,
    // Release: Option<String>,
    // Arch: Option<String>,
    // Installtime: Option<usize>,
    // Group: Option<String>,
    // Size: Option<usize>,
    // License: Option<String>,
    // Sourcerpm: Option<String>,
    // Buildtime: Option<usize>,
    // Buildhost: Option<String>,
    // Packager: Option<String>,
    // Vendor: Option<String>,
    // Url: Option<String>,
    // Bugurl: Option<String>,
    // Summary: Option<String>,
    // Description: Option<String>,
    introduced_in: String,
    dep: Vec<String>,
    // dropped: bool,
    // fullname: String,
}

#[derive(Component)]
struct Displacement(Vec3);

#[derive(Component)]
struct Edge {
    from: Entity,
    to: Entity,
}

#[derive(Resource)]
struct EntityNameMap(HashMap<String, Entity>);

#[derive(Resource)]
struct ColorLayerMap(HashMap<String, Color>);

impl Plugin for GraphPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(GraphUiPlugin);
        app.insert_resource(EntityNameMap(HashMap::new()));
        app.insert_resource(ColorLayerMap(HashMap::new()));
        app.add_systems(Startup, (spawn_nodes, spawn_edges).chain());
        app.add_systems(
            Update,
            (
                attraction,
                repulsion,
                center_force,
                apply_physics,
                draw_edges,
            )
                .chain(),
        );
    }
}

fn spawn_nodes(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
    mut names_map: ResMut<EntityNameMap>,
    mut color_map: ResMut<ColorLayerMap>,
    load_params: Res<crate::LoadParams>,
) {
    let mut nodes: Vec<Node> = Vec::new();
    if let Some(raw_json) = &load_params.opt_raw_json {
        let mut nodes_raw: Vec<Node> = serde_json::from_str(&raw_json).unwrap();
        nodes.append(&mut nodes_raw);
    }

    if let Some(file) = &load_params.opt_file {
        let reader = BufReader::new(file);
        let mut nodes_file: Vec<Node> = serde_json::from_reader(reader).unwrap();
        nodes.append(&mut nodes_file);
    }

    let shape = meshes.add(Circle::new(100.0));

    let mut rng = rand::rng();

    for node in nodes.into_iter() {
        let new_node = node.clone();

        let color;
        if !color_map.0.contains_key(&new_node.introduced_in) {
            color = Color::hsl(rng.random_range(0.0..360.0), 0.7, 0.5);
            color_map.0.insert(new_node.introduced_in, color);
        } else {
            color = *color_map.0.get(&new_node.introduced_in).unwrap();
        }

        let id = commands
            .spawn((
                Mesh2d(shape.clone()),
                MeshMaterial2d(materials.add(color)),
                Transform::from_xyz(
                    rng.random_range(-50000.0..50000.0),
                    rng.random_range(-50000.0..50000.0),
                    1.0,
                ),
                node.clone(),
                Displacement(Vec3::ZERO),
            ))
            .with_children(|parent| {
                parent.spawn((
                    Text2d::new(new_node.Name),
                    TextFont {
                        font_size: 120.0,
                        ..default()
                    },
                    Transform::from_xyz(50.0, 50.0, 0.0),
                ));
            })
            .id();
        names_map.0.insert(node.Name, id.clone());
    }
}

fn spawn_edges(mut commands: Commands, nodes: Query<&Node>, names_map: Res<EntityNameMap>) {
    let mut n_edges = 0;
    for node in &nodes {
        for dep in node.dep.clone() {
            let from = names_map.0[&node.Name];
            let to = names_map.0[&dep];

            commands.spawn((
                Edge { from, to },
                Sprite {
                    color: Color::WHITE,
                    custom_size: Some(Vec2::ZERO),
                    ..default()
                },
                Transform::from_xyz(0.0, 0.0, 0.0),
            ));

            n_edges += 1;
        }
    }
    println!("Number of edges {}", n_edges);
}

fn draw_edges(
    mut sprite_query: Query<(&mut Transform, &mut Sprite, &Edge), (With<Sprite>, Without<Node>)>,
    nodes: Query<&Transform, With<Node>>,
) {
    sprite_query.iter_mut().for_each(|(mut tr, mut spr, edge)| {
        if let (Ok(from_tf), Ok(to_tf)) = (nodes.get(edge.from), nodes.get(edge.to)) {
            let start = from_tf.translation;
            let end = to_tf.translation;

            let length = start.distance(end);
            let diff = start - end;
            let theta = diff.y.atan2(diff.x);
            let midpoint = (start + end) / 2.;

            spr.custom_size = Some(Vec2::new(length, 0.00000001 * length * length));

            *tr = Transform::from_xyz(midpoint.x, midpoint.y, -1.0)
                .with_rotation(Quat::from_rotation_z(theta));
        }
    });
}

fn repulsion(params: Res<Parameters>, mut query: Query<(&Transform, &mut Displacement)>) {
    let mut iter = query.iter_combinations_mut();
    while let Some([(transform_a, mut disp_a), (transform_b, mut disp_b)]) = iter.fetch_next() {
        let pos_a = transform_a.translation.truncate();
        let pos_b = transform_b.translation.truncate();

        let dir = pos_b - pos_a;
        let dist = dir.length();
        let delta = params.repulsion * (dir.normalize() * (params.k.powi(2) / dist)).extend(0.0);
        if dist > 0.0 {
            disp_a.0 -= delta;
            disp_b.0 += delta;
        }
    }
}

fn attraction(
    params: Res<Parameters>,
    edge_query: Query<&Edge>,
    mut node_query: Query<(&Transform, &mut Displacement)>,
) {
    edge_query.iter().for_each(|edge| {
        if let Ok([(tr_a, mut disp_a), (tr_b, mut disp_b)]) =
            node_query.get_many_mut([edge.from, edge.to])
        {
            let dir = tr_a.translation - tr_b.translation;
            let step = params.attraction * dir.normalize() * dir.length() / params.k;
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

fn apply_physics(params: Res<Parameters>, mut query: Query<(&mut Transform, &mut Displacement)>) {
    query.iter_mut().for_each(|(mut transform, mut disp)| {
        let step = disp.0.length().min(params.max_step);
        transform.translation += disp.0.normalize_or_zero() * step; //  * temp.0;

        disp.0 = Vec3::ZERO;

        // Limit nodes into a circle
        let pos = transform.translation.truncate();
        let r = pos.length();
        if r > params.max_diameter / 2.0 {
            transform.translation = (pos / r * params.max_diameter / 2.0).extend(0.0);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::Parameters;
    use super::*;
    use bevy::{
        render::{RenderPlugin, settings::WgpuSettings},
        winit::{WakeUp, WinitPlugin},
    };

    const ITER_UPDATE: usize = 100;

    macro_rules! bevy_two_nodes_graph_test {
        (
            params = { $($pfield:ident : $pvalue:expr),* $(,)? },
            update_systems = [ $( $update_sys:path ),* $(,)? ],
            new_dist $cmp:tt prev_dist $(,)?
        ) => {
            {
                let mut winit = WinitPlugin::<WakeUp>::default();
                winit.run_on_any_thread = true;

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
                        .set(winit),
                )
                .insert_resource(crate::LoadParams {
                    opt_raw_json: Some(
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
                        "#
                        .into(),
                    ),
                    opt_file: None,
                })
                .insert_resource(Parameters {
                    $($pfield : $pvalue),*,
                    ..default()
                })
                .insert_resource(EntityNameMap(HashMap::new()))
                .insert_resource(ColorLayerMap(HashMap::new()))
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
            params = {repulsion: 0.0, attraction: 10.0, center: 0.0},
            update_systems = [attraction],
            new_dist < prev_dist,
        );
    }
}
