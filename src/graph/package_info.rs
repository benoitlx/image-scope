//! Sélection d'un paquet au clic et panneau d'informations.

use super::analysis::{GraphAnalysis, closure_weight, path_from_root};
use super::coloring::{OriginPalette, security_color, swatch, to_egui};
use super::graph::{Edge, Node, ROOT_COLOR};
use super::graph_ui::Parameters;
use super::insights::{advisory_ui, insights_window};
use super::interaction::DragState;
use super::origin::Origin;
use super::security::{SecurityIndex, SecurityStatus};
use bevy::picking::mesh_picking::MeshPickingPlugin;
use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};
use bevy_pancam::{PanCam, PanCamSystemSet};

#[derive(Resource, Default)]
pub struct Selection {
    pub entity: Option<Entity>,
    /// Centrer la caméra sur la sélection à la prochaine frame.
    pub focus: bool,
}

/// Informations dérivées de la sélection, recalculées quand elle change.
#[derive(Resource, Default)]
pub struct SelectionInsight {
    pub is_root: bool,
    /// Plus court chemin d'un paquet racine jusqu'à la sélection (racine en premier).
    pub root_path: Vec<Entity>,
    /// Paquets racines qui tirent la sélection, directement ou non.
    pub pulled_by: Vec<Entity>,
    /// `(taille, nombre)` des paquets qui ne sont nécessaires qu'à travers la sélection.
    pub exclusive: (u64, usize),
    /// `(taille, nombre)` de la sélection et de toutes ses dépendances.
    pub closure: (u64, usize),
}

pub struct PackageInfoPlugin;

impl Plugin for PackageInfoPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<MeshPickingPlugin>() {
            app.add_plugins(MeshPickingPlugin);
        }
        app.init_resource::<Selection>();
        app.init_resource::<SelectionInsight>();
        app.add_observer(on_click);
        app.add_systems(
            Update,
            (
                deselect_on_escape,
                focus_camera,
                update_insight,
                block_camera_over_egui.before(PanCamSystemSet),
            ),
        );
        app.add_systems(
            EguiPrimaryContextPass,
            (package_panel, insights_window).chain(),
        );
    }
}

fn on_click(
    trigger: Trigger<Pointer<Click>>,
    nodes: Query<(), With<Node>>,
    mut selection: ResMut<Selection>,
    drag: Res<DragState>,
    mut contexts: EguiContexts,
) {
    // `drag.moved` : le relâchement termine un glisser, ce n'est pas un clic.
    if trigger.event().button != PointerButton::Primary || drag.moved {
        return;
    }
    // Clic sur une fenêtre egui : ne pas sélectionner le nœud en dessous.
    if let Ok(ctx) = contexts.ctx_mut() {
        if ctx.is_pointer_over_area() {
            return;
        }
    }
    if nodes.contains(trigger.target()) {
        selection.entity = Some(trigger.target());
    }
}

/// Empêche la molette / le glisser de bouger la caméra quand la souris est sur egui.
///
/// `bevy_pancam` a sa propre intégration egui, mais elle dépend de `bevy_egui` 0.34 alors
/// que l'application utilise la 0.36 : elle ne voit donc jamais nos contextes egui.
fn block_camera_over_egui(mut contexts: EguiContexts, mut cameras: Query<&mut PanCam>) {
    let over_egui = contexts
        .ctx_mut()
        .map(|ctx| ctx.is_pointer_over_area() || ctx.is_using_pointer())
        .unwrap_or(false);
    for mut camera in &mut cameras {
        if camera.enabled == over_egui {
            camera.enabled = !over_egui;
        }
    }
}

fn update_insight(
    selection: Res<Selection>,
    analysis: Option<Res<GraphAnalysis>>,
    mut insight: ResMut<SelectionInsight>,
) {
    let Some(analysis) = analysis else {
        return;
    };
    if !selection.is_changed() && !analysis.is_added() {
        return;
    }
    *insight = SelectionInsight::default();
    let Some(&i) = selection.entity.and_then(|e| analysis.index.get(&e)) else {
        return;
    };

    // Chemin calculé sur le graphe réduit : ses arêtes sont toujours affichées, et la
    // réduction conserve l'atteignabilité, donc les mêmes racines.
    let (path, roots) = path_from_root(analysis.dependents(true), &analysis.is_root, i);
    insight.is_root = analysis.is_root[i];
    insight.root_path = path.iter().map(|&j| analysis.entities[j]).collect();
    insight.pulled_by = roots.iter().map(|&j| analysis.entities[j]).collect();
    insight.exclusive = analysis.exclusive[i];
    insight.closure = closure_weight(&analysis.dependencies, &analysis.sizes, i);
}

fn deselect_on_escape(
    keys: Res<ButtonInput<KeyCode>>,
    mut selection: ResMut<Selection>,
    mut contexts: EguiContexts,
) {
    // Échap dans un champ texte (barre de recherche) ne doit pas fermer le panneau.
    let typing = contexts
        .ctx_mut()
        .is_ok_and(|ctx| ctx.wants_keyboard_input());
    if keys.just_pressed(KeyCode::Escape) && !typing {
        selection.entity = None;
    }
}

fn focus_camera(
    mut selection: ResMut<Selection>,
    nodes: Query<&Transform, (With<Node>, Without<Camera2d>)>,
    mut cameras: Query<&mut Transform, With<Camera2d>>,
) {
    if !selection.focus {
        return;
    }
    selection.focus = false;
    let Some(target) = selection.entity.and_then(|e| nodes.get(e).ok()) else {
        return;
    };
    for mut camera in &mut cameras {
        camera.translation.x = target.translation.x;
        camera.translation.y = target.translation.y;
    }
}

struct Neighbor<'a> {
    entity: Entity,
    name: &'a str,
    redundant: bool,
}

fn package_panel(
    mut contexts: EguiContexts,
    mut selection: ResMut<Selection>,
    params: Res<Parameters>,
    insight: Res<SelectionInsight>,
    nodes: Query<&Node>,
    origins: Query<&Origin>,
    palette: Option<Res<OriginPalette>>,
    security: Option<Res<SecurityIndex>>,
    edges: Query<&Edge>,
) -> Result {
    let Some(entity) = selection.entity else {
        return Ok(());
    };
    let Ok(node) = nodes.get(entity) else {
        selection.entity = None;
        return Ok(());
    };

    let neighbor = |e: Entity, redundant: bool| {
        nodes.get(e).ok().map(|n| Neighbor {
            entity: e,
            name: n.Name.as_str(),
            redundant,
        })
    };
    let mut dependencies: Vec<Neighbor> = edges
        .iter()
        .filter(|e| e.from == entity)
        .filter_map(|e| neighbor(e.to, e.redundant))
        .collect();
    let mut dependents: Vec<Neighbor> = edges
        .iter()
        .filter(|e| e.to == entity)
        .filter_map(|e| neighbor(e.from, e.redundant))
        .collect();
    dependencies.sort_by_key(|n| n.name);
    dependents.sort_by_key(|n| n.name);

    let mut goto = None;
    let mut close = false;

    egui::SidePanel::right("package_info")
        .default_width(380.0)
        .show(contexts.ctx_mut()?, |ui| {
            ui.horizontal(|ui| {
                let heading = ui.heading(&node.Name);
                if let Some(fullname) = &node.fullname {
                    heading.on_hover_text(fullname);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    close = ui.button("✕").on_hover_text("Close (Esc)").clicked();
                });
            });
            if let Some(summary) = &node.Summary {
                ui.label(summary);
            }
            if insight.is_root {
                ui.colored_label(to_egui(ROOT_COLOR), "● Root package")
                    .on_hover_text("Nothing depends on it: it was installed on purpose");
            }
            if node.dropped {
                ui.colored_label(egui::Color32::LIGHT_RED, "Removed in a later layer");
            }
            ui.separator();

            egui::ScrollArea::vertical().show(ui, |ui| {
                egui::Grid::new("package_fields")
                    .num_columns(2)
                    .striped(true)
                    .show(ui, |ui| {
                        let version = [&node.Version, &node.Release]
                            .iter()
                            .filter_map(|v| v.as_deref())
                            .collect::<Vec<_>>()
                            .join("-");
                        let version = match &node.Arch {
                            Some(arch) => format!("{version}.{arch}"),
                            None => version,
                        };
                        row(ui, "Version", Some(version));
                        row(ui, "Layer", Some(node.introduced_in.clone()));
                        if let (Ok(origin), Some(palette)) = (origins.get(entity), &palette) {
                            ui.strong("Origin");
                            swatch(ui, palette.color(origin), &origin.label);
                            ui.end_row();
                        }
                        row(ui, "Size", node.Size.map(human_size));
                        weight_row(
                            ui,
                            "Exclusive size",
                            insight.exclusive,
                            "This package plus the dependencies only it needs: \
                             what removing it would free",
                        );
                        weight_row(
                            ui,
                            "With all deps",
                            insight.closure,
                            "This package plus all its dependencies, shared or not",
                        );
                        row(ui, "License", node.License.clone());
                        row(ui, "Vendor", node.Vendor.clone());
                        row(ui, "Packager", node.Packager.clone());
                        row(ui, "Group", node.Group.clone());
                        row(ui, "Source RPM", node.Sourcerpm.clone());
                        row(ui, "Build date", node.Buildtime.map(format_date));
                        row(ui, "Build host", node.Buildhost.clone());
                        row(ui, "Install date", node.Installtime.map(format_date));
                        link_row(ui, "URL", node.Url.as_deref());
                        link_row(ui, "Bug tracker", node.Bugurl.as_deref());
                    });

                if let Some(description) = &node.Description {
                    egui::CollapsingHeader::new("Description")
                        .default_open(true)
                        .show(ui, |ui| ui.label(description));
                }

                if let Some(security) = security.as_deref().filter(|s| s.is_loaded()) {
                    security_section(ui, security, entity);
                }

                let name_of = |e: Entity| nodes.get(e).map(|n| n.Name.as_str()).unwrap_or("?");
                egui::CollapsingHeader::new("Why is it installed?")
                    .default_open(true)
                    .show(ui, |ui| {
                        if insight.is_root {
                            ui.label("It is a root package: nothing depends on it.");
                            return;
                        }
                        ui.label("Shortest chain from a root package:");
                        ui.horizontal_wrapped(|ui| {
                            for (k, &e) in insight.root_path.iter().enumerate() {
                                if k > 0 {
                                    ui.label("→");
                                }
                                if e == entity {
                                    ui.strong(name_of(e));
                                } else if ui.link(name_of(e)).clicked() {
                                    goto = Some(e);
                                }
                            }
                        });
                        let mut roots: Vec<Entity> = insight.pulled_by.clone();
                        roots.sort_by_key(|e| name_of(*e));
                        egui::CollapsingHeader::new(format!(
                            "Pulled in by {} root packages",
                            roots.len()
                        ))
                        .id_salt("pulled_by")
                        .show(ui, |ui| {
                            for e in roots {
                                if ui.link(name_of(e)).clicked() {
                                    goto = Some(e);
                                }
                            }
                        });
                    });

                let hide_redundant = params.transitive_reduction;
                neighbor_list(ui, "Dependencies", &dependencies, hide_redundant, &mut goto);
                neighbor_list(ui, "Required by", &dependents, hide_redundant, &mut goto);
            });
        });

    if close {
        selection.entity = None;
    } else if let Some(target) = goto {
        selection.entity = Some(target);
        selection.focus = true;
    }
    Ok(())
}

fn security_section(ui: &mut egui::Ui, security: &SecurityIndex, entity: Entity) {
    let status = security.status(entity);
    let Some(package) = security.packages.get(&entity) else {
        return;
    };
    egui::CollapsingHeader::new("Security")
        .default_open(true)
        .show(ui, |ui| match status {
            SecurityStatus::NotCovered => {
                swatch(
                    ui,
                    security_color(status),
                    "Not covered: Bodhi only tracks Fedora packages.",
                );
            }
            _ => {
                if package.pending.is_empty() {
                    swatch(
                        ui,
                        security_color(status),
                        "No pending Fedora security fix.",
                    );
                }
                let mut pending = package.pending.clone();
                pending.sort_by_key(|&i| {
                    let a = &security.advisories[i];
                    (!a.is_stable(), a.severity_rank())
                });
                for i in pending {
                    advisory_ui(ui, &security.advisories[i]);
                    ui.separator();
                }
                if !package.fixed.is_empty() {
                    egui::CollapsingHeader::new(format!(
                        "{} past advisories already fixed in this version",
                        package.fixed.len()
                    ))
                    .id_salt("fixed_advisories")
                    .show(ui, |ui| {
                        for &i in &package.fixed {
                            let a = &security.advisories[i];
                            ui.horizontal(|ui| {
                                ui.hyperlink_to(&a.alias, &a.url);
                                ui.weak(format!("{} CVEs · {}", a.cves.len(), a.severity));
                            });
                        }
                    });
                }
            }
        });
}

fn row(ui: &mut egui::Ui, label: &str, value: Option<String>) {
    if let Some(value) = value.filter(|v| !v.is_empty()) {
        ui.strong(label);
        ui.add(egui::Label::new(value).wrap());
        ui.end_row();
    }
}

fn weight_row(ui: &mut egui::Ui, label: &str, (size, count): (u64, usize), help: &str) {
    if count == 0 {
        return;
    }
    ui.strong(label).on_hover_text(help);
    ui.label(format!("{} ({count} packages)", human_size(size)));
    ui.end_row();
}

fn link_row(ui: &mut egui::Ui, label: &str, url: Option<&str>) {
    if let Some(url) = url.filter(|u| !u.is_empty()) {
        ui.strong(label);
        ui.hyperlink(url);
        ui.end_row();
    }
}

fn neighbor_list(
    ui: &mut egui::Ui,
    title: &str,
    neighbors: &[Neighbor],
    hide_redundant: bool,
    goto: &mut Option<Entity>,
) {
    let direct = neighbors.iter().filter(|n| !n.redundant).count();
    let header = if hide_redundant {
        format!(
            "{title} ({direct} direct, {} implied)",
            neighbors.len() - direct
        )
    } else {
        format!("{title} ({})", neighbors.len())
    };
    egui::CollapsingHeader::new(header)
        .id_salt(title)
        .default_open(true)
        .show(ui, |ui| {
            for n in neighbors {
                ui.horizontal(|ui| {
                    if ui.link(n.name).clicked() {
                        *goto = Some(n.entity);
                    }
                    if hide_redundant && n.redundant {
                        ui.weak("implied").on_hover_text(
                            "Also reachable through another dependency, hidden in the graph",
                        );
                    }
                });
            }
        });
}

pub(super) fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

/// Timestamp Unix → `AAAA-MM-JJ` (UTC), sans dépendance externe.
fn format_date(timestamp: u64) -> String {
    // Algorithme « civil_from_days » de Howard Hinnant.
    let z = (timestamp / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_dates() {
        assert_eq!(format_date(0), "1970-01-01");
        assert_eq!(format_date(951_782_400), "2000-02-29");
        assert_eq!(format_date(1_794_103_640), "2026-11-08");
    }

    #[test]
    fn formats_sizes() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(277_212), "270.7 KiB");
        assert_eq!(human_size(3 * 1024 * 1024 * 1024), "3.0 GiB");
    }
}
