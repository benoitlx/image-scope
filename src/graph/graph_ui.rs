use super::interaction::Pinned;
use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPlugin, EguiPrimaryContextPass, egui};

#[derive(Resource)]
pub struct Parameters {
    pub repulsion: f32,
    pub attraction: f32,
    pub center: f32,
    pub k: f32,
    pub max_diameter: f32,
    /// Précision de Barnes-Hut : 0 = exact, plus grand = plus rapide.
    pub theta: f32,
    /// Facteur d'adaptation du pas (Yifan Hu) : le pas est multiplié par ce
    /// facteur quand l'énergie remonte, et divisé quand elle baisse durablement.
    pub cooling: f32,
    /// Masque (et ignore dans la simulation) les dépendances impliquées par un autre chemin.
    pub transitive_reduction: bool,
}

impl Default for Parameters {
    fn default() -> Self {
        Parameters {
            repulsion: 10000.0,
            attraction: 10.0,
            center: 0.00001,
            k: 10.0,
            max_diameter: 500000.0,
            theta: 0.9,
            cooling: 0.9,
            transitive_reduction: true,
        }
    }
}

impl Parameters {
    /// Distance d'équilibre entre deux nœuds reliés (attraction = répulsion).
    pub fn natural_length(&self) -> f32 {
        self.k * (self.repulsion * self.k / self.attraction.max(1e-6)).sqrt()
    }
}

/// État de la simulation à pas adaptatif. Elle tourne en continu, sauf si
/// l'utilisateur la verrouille.
#[derive(Resource)]
pub struct Simulation {
    pub locked: bool,
    /// Déplacement maximal d'un nœud par frame.
    pub step: f32,
    /// Somme des |force|² à la frame précédente.
    pub energy: f32,
    /// Nombre de frames consécutives où l'énergie a baissé.
    pub progress: u32,
    /// Déplacement moyen des nœuds à la dernière frame.
    pub mean_move: f32,
}

impl Simulation {
    pub fn new(params: &Parameters) -> Self {
        Simulation {
            locked: false,
            step: params.natural_length() / 10.0,
            energy: f32::INFINITY,
            progress: 0,
            mean_move: 0.0,
        }
    }

    /// Redonne de l'élan à la simulation (paramètres changés, nœud déplacé…),
    /// sans toucher au verrou.
    pub fn reheat(&mut self, params: &Parameters) {
        *self = Simulation {
            locked: self.locked,
            ..Simulation::new(params)
        };
    }
}

impl Default for Simulation {
    fn default() -> Self {
        Simulation::new(&Parameters::default())
    }
}

fn ui_forces(
    mut commands: Commands,
    mut params: ResMut<Parameters>,
    mut simulation: ResMut<Simulation>,
    pinned: Query<Entity, With<Pinned>>,
    mut contexts: EguiContexts,
) -> Result {
    let params = params.bypass_change_detection();
    let mut changed = false;

    egui::Window::new("Forces").show(contexts.ctx_mut()?, |ui| {
        ui.label("Repulsion factor");
        changed |= ui
            .add(egui::Slider::new(&mut params.repulsion, 90.0..=10000.0))
            .changed();

        ui.label("Attraction factor");
        changed |= ui
            .add(egui::Slider::new(&mut params.attraction, 0.01..=10.0))
            .changed();

        ui.label("Center factor");
        changed |= ui
            .add(egui::Slider::new(&mut params.center, 0.000001..=0.1).logarithmic(true))
            .changed();

        ui.label("k");
        changed |= ui
            .add(egui::Slider::new(&mut params.k, 0.1..=10.0))
            .changed();

        ui.label("max diameter");
        changed |= ui
            .add(egui::Slider::new(&mut params.max_diameter, 1000.0..=500000.0).logarithmic(true))
            .changed();

        ui.label("Barnes-Hut theta");
        changed |= ui
            .add(egui::Slider::new(&mut params.theta, 0.0..=2.0))
            .changed();

        ui.label("Step adaptation");
        changed |= ui
            .add(egui::Slider::new(&mut params.cooling, 0.5..=0.99))
            .changed();

        changed |= ui
            .checkbox(&mut params.transitive_reduction, "Transitive reduction")
            .on_hover_text("Hide dependencies already implied by another path (a→c when a→b→c)")
            .changed();

        ui.separator();
        ui.label(format!("Edge length ≈ {:.0}", params.natural_length()));
        if simulation.locked {
            ui.label("Layout locked");
        } else {
            ui.label(format!(
                "Step {:.1} · mean move {:.2}",
                simulation.step, simulation.mean_move
            ));
        }
        ui.horizontal(|ui| {
            let lock_label = if simulation.locked {
                "🔓 Unlock"
            } else {
                "🔒 Lock"
            };
            if ui
                .button(lock_label)
                .on_hover_text("Freeze the layout (dragging still moves the node itself)")
                .clicked()
            {
                simulation.locked = !simulation.locked;
                changed = true;
            }
            changed |= ui.button("Reheat").clicked();
            let n_pinned = pinned.iter().count();
            if ui
                .add_enabled(
                    n_pinned > 0,
                    egui::Button::new(format!("Unpin all ({n_pinned})")),
                )
                .on_hover_text("Dragged nodes stay pinned; right click a node to unpin it")
                .clicked()
            {
                for entity in &pinned {
                    commands.entity(entity).remove::<Pinned>();
                }
                changed = true;
            }
        });
    });

    if changed {
        simulation.reheat(params);
    }
    Ok(())
}

pub struct GraphUiPlugin;

impl Plugin for GraphUiPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(Parameters::default());
        app.insert_resource(Simulation::default());
        app.add_plugins(EguiPlugin::default());
        app.add_systems(EguiPrimaryContextPass, ui_forces);
    }
}
