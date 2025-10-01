use crate::graph::GraphPlugin;
use bevy::prelude::*;
use bevy_pancam::{PanCam, PanCamPlugin};
use std::fs::File;
use std::process;

mod graph;

#[derive(Resource)]
pub struct LoadParams {
    opt_raw_json: Option<String>,
    opt_file: Option<File>,
}

fn main() {
    let file = File::open("packages-map.json").unwrap_or_else(|_| {
        process::exit(1);
    });

    App::new()
        .insert_resource(LoadParams {
            opt_raw_json: None,
            opt_file: Some(file),
        })
        .add_plugins(DefaultPlugins)
        .add_plugins(GraphPlugin)
        .add_plugins(PanCamPlugin::default())
        .add_systems(Startup, setup)
        .run();
}

fn setup(mut commands: Commands) {
    commands.spawn((
        Camera2d,
        PanCam {
            grab_buttons: vec![MouseButton::Middle],
            ..default()
        },
    ));
}
