use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPlugin, EguiPrimaryContextPass, egui};
use rand::Rng;
use serde::Deserialize;
use std::fs::File;
use std::io::BufReader;
use std::process;

#[derive(Component, Deserialize, Clone, Debug)]
struct Node {
    Name: String,
    Version: String,
    Release: Option<String>,
    Arch: Option<String>,
    Installtime: Option<usize>,
    Group: Option<String>,
    Size: Option<usize>,
    License: Option<String>,
    Sourcerpm: Option<String>,
    Buildtime: Option<usize>,
    Buildhost: Option<String>,
    Packager: Option<String>,
    Vendor: Option<String>,
    Url: Option<String>,
    Bugurl: Option<String>,
    Summary: Option<String>,
    Description: Option<String>,
    introduced_in: Option<String>,
    dep: Vec<String>,
    dropped: bool,
    fullname: String,
}

#[derive(Deserialize, Debug)]
struct NodeList {
    nodes: Vec<Node>,
}

#[derive(Component)]
struct Edge {
    from: Entity,
    to: Entity,
}

#[derive(Resource)]
struct Parameters {
    damping: f32,
    max_diameter: f32,
}

#[derive(Resource)]
struct Temperature(f32);

const NODE_COUNT: usize = 100;

fn spawn_nodes(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    let shape = meshes.add(Circle::new(10.0));

    let mut rng = rand::rng();

    let file = File::open("packages-map.json").unwrap_or_else(|_| {
        process::exit(1);
    });
    let reader = BufReader::new(file);

    let nodes: Vec<Node> = serde_json::from_reader(reader).unwrap();

    for node in nodes.into_iter() {
        let start_pos = Vec3::new(
            rng.random_range(-100.0..100.0),
            rng.random_range(-100.0..100.0),
            0.0,
        );

        commands
            .spawn((
                Mesh2d(shape.clone()),
                MeshMaterial2d(materials.add(Color::hsl(rng.random_range(0.0..360.0), 0.7, 0.5))),
                Transform::from_translation(start_pos),
                node.clone(),
            ))
            .with_children(|parent| {
                parent.spawn((Text2d::new(node.Name), Transform::from_xyz(25.0, 25.0, 0.0)));
            });
    }
}

fn spawn_edges(mut commands: Commands, nodes: Query<Entity, With<Node>>) {
    let node_ids: Vec<Entity> = nodes.iter().collect();
    let mut rng = rand::rng();

    for _ in 0..NODE_COUNT {
        let from = node_ids[rng.random_range(0..node_ids.len())];
        let to = node_ids[rng.random_range(0..node_ids.len())];

        if from != to {
            commands.spawn(Edge { from, to });
        }
    }
}

fn ui_forces(mut ui_state: ResMut<Parameters>, mut contexts: EguiContexts) -> Result {
    egui::Window::new("Forces").show(contexts.ctx_mut()?, |ui| {
        ui.label("Damping");
        ui.add(egui::Slider::new(&mut ui_state.damping, 0.95..=0.999));

        ui.label("Max Diameter");
        ui.add(egui::Slider::new(
            &mut ui_state.max_diameter,
            200.0..=1400.0,
        ));
    });
    Ok(())
}

fn node_and_edge_physics(
    params: Res<Parameters>,
    temp: Res<Temperature>,
    mut query: Query<(Entity, &mut Transform, &mut Node)>,
    edge_query: Query<&Edge>,
) {
    // Gather positions once
    let nodes: Vec<(Entity, Vec2)> = query
        .iter()
        .map(|(e, t, _)| (e, t.translation.truncate()))
        .collect();

    // We'll apply forces after computing them for each node
    for (entity, mut transform, _) in query.iter_mut() {
        let node_pos = transform.translation.truncate();

        let mut disp = Vec2::ZERO;
        let pos = transform.translation.truncate();

        let k = params.max_diameter / 2.0 * bevy::math::ops::sqrt(3.1415 / NODE_COUNT as f32);

        // Repulsion from other nodes
        for (_, other_pos) in &nodes {
            let dir = pos - *other_pos;
            let dist = dir.length();
            if dist > 0.0 {
                disp += dir.normalize() * (k.powi(2) / dist);
            }
        }

        for edge in edge_query.iter() {
            if edge.from == entity || edge.to == entity {
                let other_entity = if edge.from == entity {
                    edge.to
                } else {
                    edge.from
                };
                if let Some((_, other_pos)) = nodes.iter().find(|(e, _)| *e == other_entity) {
                    let dir = *other_pos - node_pos;
                    disp += dir.normalize() * dir.length_squared() / k;
                }
            }
        }

        // --- Apply physics ---
        if disp.length() < temp.0 {
            transform.translation += disp.extend(0.0);
        } else {
            transform.translation += disp.extend(0.0) / disp.length() * temp.0;
        }

        // Limit distance from center
        let pos = transform.translation.truncate();
        let r = pos.length();
        if r > params.max_diameter / 2.0 {
            transform.translation = (pos / r * params.max_diameter / 2.0).extend(0.0);
        }
    }
}

fn draw_edges(edges: Query<&Edge>, nodes: Query<&Transform, With<Node>>, mut gizmos: Gizmos) {
    for edge in edges.iter() {
        if let (Ok(from_tf), Ok(to_tf)) = (nodes.get(edge.from), nodes.get(edge.to)) {
            gizmos.arrow_2d(
                from_tf.translation.truncate(),
                to_tf.translation.truncate(),
                Color::WHITE,
            );
        }
    }
}

fn cooldown(mut temp: ResMut<Temperature>, params: Res<Parameters>) {
    // temp.0 /= 2.0;
    temp.0 *= params.damping;
}

fn raise_temp(_click: Trigger<Pointer<Click>>, mut temp: ResMut<Temperature>) {
    temp.0 = 1.0;
}

fn setup(mut commands: Commands) {
    commands.spawn(Camera2d).observe(raise_temp);
}

pub struct GraphPlugin;

impl Plugin for GraphPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(Parameters {
            damping: 0.995,
            max_diameter: 1200.0,
        });
        app.insert_resource(Temperature(3.0));
        app.add_systems(Startup, setup);
        app.add_systems(Startup, spawn_nodes);
        app.add_systems(Startup, spawn_edges.after(spawn_nodes));
        app.add_systems(Update, (node_and_edge_physics, cooldown).chain());
        app.add_systems(Update, draw_edges);
        app.add_systems(EguiPrimaryContextPass, ui_forces);
    }
}

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(GraphPlugin)
        .add_plugins(EguiPlugin::default())
        .run();
}
