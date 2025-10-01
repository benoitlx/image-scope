use crate::graph::GraphPlugin;
use bevy::prelude::*;
use bevy_pancam::{PanCam, PanCamPlugin};

mod graph;

fn main() {
    App::new()
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
