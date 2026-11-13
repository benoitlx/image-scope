//! Coloration des nœuds selon un mode choisi : couche, origine ou sécurité.

use super::analysis::GraphAnalysis;
use super::graph::{Node, ROOT_COLOR};
use super::origin::{Origin, OriginKind};
use super::security::{SecurityIndex, SecurityStatus};
use bevy::prelude::*;
use bevy_egui::egui;
use std::collections::HashMap;

#[derive(Resource, Default, Clone, Copy, PartialEq, Eq, Debug)]
pub enum ColorMode {
    /// Couleur de la couche d'introduction, racines en doré.
    #[default]
    Layer,
    /// Fedora en bleu discret, chaque source tierce dans sa couleur.
    Origin,
    /// Statut vis-à-vis des avis de sécurité Fedora.
    Security,
}

/// Couleur de chaque couche (`introduced_in`).
#[derive(Resource, Default)]
pub struct LayerColors(pub HashMap<String, Color>);

/// Couleur de chaque origine (`Origin::label`).
#[derive(Resource, Default)]
pub struct OriginPalette(pub HashMap<String, Color>);

pub const FEDORA_COLOR: Color = Color::srgb(0.35, 0.45, 0.62);
const KEY_COLOR: Color = Color::srgb(0.3, 0.3, 0.33);
const UNKNOWN_COLOR: Color = Color::srgb(1.0, 0.25, 0.25);
const THIRD_PARTY_PALETTE: [Color; 10] = [
    Color::srgb(1.0, 0.6, 0.1),
    Color::srgb(0.7, 0.4, 1.0),
    Color::srgb(0.2, 0.85, 0.9),
    Color::srgb(1.0, 0.45, 0.75),
    Color::srgb(0.95, 0.9, 0.3),
    Color::srgb(0.6, 0.9, 0.3),
    Color::srgb(0.2, 0.7, 0.6),
    Color::srgb(1.0, 0.65, 0.5),
    Color::srgb(0.5, 0.7, 1.0),
    Color::srgb(0.75, 0.95, 0.75),
];

pub fn security_color(status: SecurityStatus) -> Color {
    match status {
        SecurityStatus::FixAvailable => Color::srgb(1.0, 0.2, 0.2),
        SecurityStatus::FixInTesting => Color::srgb(1.0, 0.6, 0.1),
        SecurityStatus::UpToDate => Color::srgb(0.25, 0.55, 0.35),
        SecurityStatus::NotCovered => Color::srgb(0.3, 0.3, 0.33),
    }
}

impl OriginPalette {
    pub fn color(&self, origin: &Origin) -> Color {
        match origin.kind {
            OriginKind::Fedora => FEDORA_COLOR,
            OriginKind::Key => KEY_COLOR,
            OriginKind::Unknown => UNKNOWN_COLOR,
            OriginKind::Copr | OriginKind::Vendor => {
                self.0.get(&origin.label).copied().unwrap_or(Color::WHITE)
            }
        }
    }
}

/// Attribue une couleur à chaque source tierce, les plus fréquentes d'abord.
pub fn build_origin_palette(mut commands: Commands, origins: Query<&Origin>) {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for o in origins
        .iter()
        .filter(|o| matches!(o.kind, OriginKind::Copr | OriginKind::Vendor))
    {
        *counts.entry(o.label.as_str()).or_default() += 1;
    }
    let mut labels: Vec<_> = counts.into_iter().collect();
    labels.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let palette = labels
        .into_iter()
        .enumerate()
        .map(|(i, (label, _))| {
            (
                label.to_owned(),
                THIRD_PARTY_PALETTE[i % THIRD_PARTY_PALETTE.len()],
            )
        })
        .collect();
    commands.insert_resource(OriginPalette(palette));
}

pub struct ColoringPlugin;

impl Plugin for ColoringPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ColorMode>()
            .init_resource::<LayerColors>()
            .add_systems(
                Update,
                apply_colors.run_if(
                    resource_exists::<GraphAnalysis>
                        .and(resource_exists::<SecurityIndex>)
                        .and(resource_exists::<OriginPalette>),
                ),
            );
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_colors(
    mode: Res<ColorMode>,
    analysis: Res<GraphAnalysis>,
    security: Res<SecurityIndex>,
    palette: Res<OriginPalette>,
    layers: Res<LayerColors>,
    mut materials: ResMut<Assets<ColorMaterial>>,
    mut cache: Local<HashMap<[u8; 4], Handle<ColorMaterial>>>,
    mut nodes: Query<(Entity, &Node, &Origin, &mut MeshMaterial2d<ColorMaterial>)>,
) {
    if !(mode.is_changed()
        || analysis.is_changed()
        || security.is_changed()
        || palette.is_changed())
    {
        return;
    }
    for (entity, node, origin, mut material) in &mut nodes {
        let color = match *mode {
            ColorMode::Layer => {
                let is_root = analysis
                    .index
                    .get(&entity)
                    .is_some_and(|&i| analysis.is_root[i]);
                if is_root {
                    ROOT_COLOR
                } else {
                    layers
                        .0
                        .get(&node.introduced_in)
                        .copied()
                        .unwrap_or(Color::WHITE)
                }
            }
            ColorMode::Origin => palette.color(origin),
            ColorMode::Security => security_color(security.status(entity)),
        };
        // Un matériau partagé par couleur, pour que Bevy puisse regrouper les rendus.
        let handle = cache
            .entry(color.to_srgba().to_u8_array())
            .or_insert_with(|| materials.add(color))
            .clone();
        if material.0 != handle {
            material.0 = handle;
        }
    }
}

pub fn to_egui(color: Color) -> egui::Color32 {
    let [r, g, b, a] = color.to_srgba().to_u8_array();
    egui::Color32::from_rgba_unmultiplied(r, g, b, a)
}

/// Pastille de couleur suivie d'un texte.
pub fn swatch(
    ui: &mut egui::Ui,
    color: Color,
    text: impl Into<egui::WidgetText>,
) -> egui::Response {
    ui.horizontal(|ui| {
        ui.colored_label(to_egui(color), "●");
        ui.label(text);
    })
    .response
}
