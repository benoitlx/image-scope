use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPlugin, EguiPrimaryContextPass, egui};
use bevy_pancam::{PanCam, PanCamPlugin};
use rand::Rng;
use serde::Deserialize;
use std::collections::HashMap;
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

#[derive(Component)]
struct Displacement(Vec3);

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

#[derive(Resource)]
struct EntityNameMap(HashMap<String, Entity>);

fn spawn_nodes(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
    mut names_map: ResMut<EntityNameMap>,
) {
    let shape = meshes.add(Circle::new(100.0));

    let mut rng = rand::rng();

    let file = File::open("packages-map.json").unwrap_or_else(|_| {
        process::exit(1);
    });
    let reader = BufReader::new(file);

    let nodes: Vec<Node> = serde_json::from_reader(reader).unwrap();

    for node in nodes.into_iter() {
        let start_pos = Vec3::new(
            rng.random_range(-50000.0..50000.0),
            rng.random_range(-50000.0..50000.0),
            0.0,
        );

        let id = commands
            .spawn((
                Mesh2d(shape.clone()),
                MeshMaterial2d(materials.add(Color::hsl(rng.random_range(0.0..360.0), 0.7, 0.5))),
                Transform::from_translation(start_pos),
                node.clone(),
                Displacement(start_pos),
            ))
            .with_children(|parent| {
                parent.spawn((
                    Text2d::new(node.Name.clone()),
                    Transform::from_xyz(25.0, 25.0, 0.0),
                ));
            })
            .id();
        names_map.0.insert(node.Name, id.clone());
    }
}

fn spawn_edges(
    mut commands: Commands,
    nodes: Query<&Node, With<Node>>,
    names_map: Res<EntityNameMap>,
) {
    // let node_ids: Vec<Entity> = nodes.iter().collect();

    for node in &nodes {
        for dep in node.dep.clone() {
            // println!("from {}", &node.Name);
            // println!("to {}", &dep);
            let from = names_map.0[&node.Name];
            let to = names_map.0[&dep];

            commands.spawn(Edge { from, to });
        }
    }
}

fn ui_forces(mut ui_state: ResMut<Parameters>, mut contexts: EguiContexts) -> Result {
    egui::Window::new("Forces").show(contexts.ctx_mut()?, |ui| {
        ui.label("Damping");
        ui.add(egui::Slider::new(&mut ui_state.damping, 0.95..=0.99999));

        ui.label("Max Diameter");
        ui.add(egui::Slider::new(
            &mut ui_state.max_diameter,
            200.0..=100000.0,
        ));
    });
    Ok(())
}

fn draw_edges(edges: Query<&Edge>, nodes: Query<&Transform, With<Node>>, mut gizmos: Gizmos) {
    for edge in edges.iter() {
        if let (Ok(from_tf), Ok(to_tf)) = (nodes.get(edge.from), nodes.get(edge.to)) {
            gizmos.line_2d(
                from_tf.translation.truncate(),
                to_tf.translation.truncate(),
                Color::WHITE,
            );
        }
    }
}

fn repulsion(params: Res<Parameters>, mut query: Query<(&Transform, &mut Displacement)>) {
    let k = params.max_diameter / 2.0 * bevy::math::ops::sqrt(3.1415 / 2000 as f32);

    let mut iter = query.iter_combinations_mut();
    while let Some([(transform_a, mut disp_a), (transform_b, _)]) = iter.fetch_next() {
        let pos_a = transform_a.translation.truncate();
        let pos_b = transform_b.translation.truncate();

        let dir = pos_a - pos_b;
        let dist = dir.length();
        if dist > 0.0 {
            disp_a.0 += (dir.normalize() * (k.powi(2) / dist)).extend(0.0);
        }
    }
}

fn attraction(params: Res<Parameters>, edge_query: Query<&Edge>) {}

fn apply_physics(
    params: Res<Parameters>,
    temp: Res<Temperature>,
    mut query: Query<(&mut Transform, &mut Displacement)>,
) {
    query.iter_mut().for_each(|(mut transform, mut disp)| {
        if disp.0.length() < temp.0 {
            transform.translation += disp.0;
        } else {
            transform.translation += disp.0 / disp.0.length() * temp.0;
        }

        disp.0 = Vec3::ZERO;

        // Limit nodes into a circle
        let pos = transform.translation.truncate();
        let r = pos.length();
        if r > params.max_diameter / 2.0 {
            transform.translation = (pos / r * params.max_diameter / 2.0).extend(0.0);
        }
    });
}

fn cooldown(mut temp: ResMut<Temperature>, params: Res<Parameters>) {
    // temp.0 /= 2.0;
    temp.0 *= params.damping;
}

fn raise_temp(_click: Trigger<Pointer<Click>>, mut temp: ResMut<Temperature>) {
    temp.0 = 40.0;
}

pub struct GraphPlugin;

impl Plugin for GraphPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(Parameters {
            damping: 0.995,
            max_diameter: 100000.0,
        });
        app.insert_resource(Temperature(3.0));
        app.insert_resource(EntityNameMap(HashMap::new()));
        app.add_systems(Startup, spawn_nodes);
        app.add_systems(Startup, spawn_edges.after(spawn_nodes));
        app.add_systems(EguiPrimaryContextPass, ui_forces);
        app.add_systems(
            Update,
            (repulsion, attraction, apply_physics, cooldown).chain(),
        );
        // app.add_systems(Update, draw_edges);
    }
}

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(GraphPlugin)
        .add_plugins(EguiPlugin::default())
        .add_plugins(PanCamPlugin::default())
        .add_systems(Startup, setup)
        .run();
}

fn setup(mut commands: Commands) {
    commands
        .spawn((
            Camera2d,
            PanCam {
                grab_buttons: vec![MouseButton::Middle],
                ..default()
            },
        ))
        .observe(raise_temp);
}
