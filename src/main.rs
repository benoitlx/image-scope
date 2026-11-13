#[cfg(feature = "viewer")]
use crate::graph::GraphPlugin;
#[cfg(feature = "viewer")]
use bevy::dev_tools::fps_overlay::{FpsOverlayConfig, FpsOverlayPlugin};
#[cfg(feature = "viewer")]
use bevy::prelude::*;
#[cfg(feature = "viewer")]
use bevy_pancam::{PanCam, PanCamPlugin};

#[cfg(feature = "viewer")]
mod graph;
mod model;
#[cfg(not(target_arch = "wasm32"))]
mod scan;

fn main() {
    // Sous-commandes (inspect, advisories…) : pas de fenêtre.
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(code) = scan::run_cli(&std::env::args().skip(1).collect::<Vec<_>>()) {
        std::process::exit(code);
    }

    #[cfg(feature = "viewer")]
    run_viewer();

    #[cfg(not(feature = "viewer"))]
    {
        eprintln!("This build has no viewer (--no-default-features). See `image-scope help`.");
        std::process::exit(2);
    }
}

#[cfg(feature = "viewer")]
fn run_viewer() {
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "image-scope".into(),
                // Web : dessine dans le <canvas id="bevy"> de index.html, à sa taille.
                canvas: Some("#bevy".into()),
                fit_canvas_to_parent: true,
                ..default()
            }),
            ..default()
        }))
        .add_plugins(FpsOverlayPlugin {
            config: FpsOverlayConfig {
                text_config: TextFont {
                    font_size: 20.0,
                    ..default()
                },
                ..default()
            },
        })
        .add_plugins(GraphPlugin)
        .add_plugins(PanCamPlugin::default())
        .add_systems(Startup, setup)
        .run();
}

#[cfg(feature = "viewer")]
fn setup(mut commands: Commands) {
    commands.spawn((
        Camera2d,
        PanCam {
            grab_buttons: vec![MouseButton::Middle],
            ..default()
        },
    ));
}
