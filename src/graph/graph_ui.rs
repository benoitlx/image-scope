use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPlugin, EguiPrimaryContextPass, egui};

#[derive(Resource)]
pub struct Parameters {
    pub repulsion: f32,
    pub attraction: f32,
    pub center: f32,
    pub k: f32,
    pub max_step: f32,
    pub max_diameter: f32,
}

fn ui_forces(mut ui_state: ResMut<Parameters>, mut contexts: EguiContexts) -> Result {
    egui::Window::new("Forces").show(contexts.ctx_mut()?, |ui| {
        ui.label("Repulsion factor");
        ui.add(egui::Slider::new(&mut ui_state.repulsion, 90.0..=10000.0));

        ui.label("Attraction factor");
        ui.add(egui::Slider::new(&mut ui_state.attraction, 0.01..=10.0));

        ui.label("Center factor");
        ui.add(egui::Slider::new(&mut ui_state.center, 0.000001..=0.1));

        ui.label("k");
        ui.add(egui::Slider::new(&mut ui_state.k, 0.1..=10.0));

        ui.label("max step");
        ui.add(egui::Slider::new(&mut ui_state.max_step, 0.1..=100.0));

        ui.label("max diameter");
        ui.add(egui::Slider::new(
            &mut ui_state.max_diameter,
            1000.0..=500000.0,
        ));
    });
    Ok(())
}

pub struct GraphUiPlugin;

impl Plugin for GraphUiPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(Parameters {
            repulsion: 300.0,
            attraction: 0.01,
            center: 0.00001,
            k: 10000.0 / 2.0 * bevy::math::ops::sqrt(3.1415 / 2000 as f32),
            max_step: 10.0,
            max_diameter: 30000.0,
        });
        app.add_plugins(EguiPlugin::default());
        app.add_systems(EguiPrimaryContextPass, ui_forces);
    }
}
