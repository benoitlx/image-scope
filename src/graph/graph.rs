use super::graph_ui::{GraphUiPlugin, Parameters};
use bevy::prelude::*;
use rand::Rng;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::process;

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
) {
    let shape = meshes.add(Circle::new(100.0));

    let mut rng = rand::rng();

    let file = File::open("packages-map.json").unwrap_or_else(|_| {
        process::exit(1);
    });
    let reader = BufReader::new(file);

    let nodes: Vec<Node> = serde_json::from_reader(reader).unwrap();

    // let test_string = r#"
    //    [
    //        {
    //            "Name": "a",
    //            "dep": ["b"]
    //        },
    //        {
    //            "Name": "b",
    //            "dep": []
    //        }
    //    ]
    // "#;
    // let nodes: Vec<Node> = serde_json::from_str(test_string).unwrap();

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
