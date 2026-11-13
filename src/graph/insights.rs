//! Fenêtre « Insights » : mode de coloration, paquets racines, origines, sécurité.

use super::analysis::GraphAnalysis;
use super::coloring::{ColorMode, LayerColors, OriginPalette, security_color, swatch, to_egui};
use super::graph::{Node, ROOT_COLOR};
use super::origin::{Origin, OriginKind};
use super::package_info::{Selection, human_size};
use super::security::{Advisory, SecurityIndex, SecurityStatus};
use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use std::collections::BTreeMap;

#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Roots,
    Origins,
    Security,
}

pub struct OriginGroup {
    origin: Origin,
    size: u64,
    packages: Vec<(Entity, String)>,
}

pub struct PendingAdvisory {
    advisory: usize,
    packages: Vec<(Entity, String)>,
}

#[derive(Default)]
pub struct Cache {
    built: bool,
    roots: Vec<(Entity, String, u64, usize)>,
    origins: Vec<OriginGroup>,
    pending: Vec<PendingAdvisory>,
    status_counts: BTreeMap<SecurityStatus, usize>,
}

#[allow(clippy::too_many_arguments)]
pub fn insights_window(
    mut contexts: EguiContexts,
    analysis: Option<Res<GraphAnalysis>>,
    security: Option<Res<SecurityIndex>>,
    palette: Option<Res<OriginPalette>>,
    layers: Res<LayerColors>,
    mut mode: ResMut<ColorMode>,
    nodes: Query<(Entity, &Node, &Origin)>,
    mut selection: ResMut<Selection>,
    mut tab: Local<Tab>,
    mut cache: Local<Cache>,
) -> Result {
    let (Some(analysis), Some(security), Some(palette)) = (analysis, security, palette) else {
        return Ok(());
    };
    // Reconstruit après le chargement d'un autre jeu de données.
    if !cache.built || analysis.is_changed() || security.is_changed() || palette.is_changed() {
        *cache = build_cache(&analysis, &security, &nodes);
    }

    let mut goto = None;
    let mut new_mode = *mode;

    egui::Window::new("Insights")
        .default_open(false)
        .default_width(420.0)
        .show(contexts.ctx_mut()?, |ui| {
            ui.horizontal(|ui| {
                ui.label("Color by");
                ui.radio_value(&mut new_mode, ColorMode::Layer, "Layer");
                ui.radio_value(&mut new_mode, ColorMode::Origin, "Origin");
                ui.radio_value(&mut new_mode, ColorMode::Security, "Security");
            });
            legend(ui, new_mode, &layers, &cache);
            ui.separator();

            ui.horizontal(|ui| {
                ui.selectable_value(
                    &mut *tab,
                    Tab::Roots,
                    format!("Roots ({})", cache.roots.len()),
                );
                let third_party: usize = cache
                    .origins
                    .iter()
                    .filter(|g| g.origin.is_third_party())
                    .map(|g| g.packages.len())
                    .sum();
                ui.selectable_value(
                    &mut *tab,
                    Tab::Origins,
                    format!("Origins ({third_party} third-party)"),
                );
                ui.selectable_value(
                    &mut *tab,
                    Tab::Security,
                    format!("Security ({})", cache.pending.len()),
                );
            });
            ui.separator();

            egui::ScrollArea::vertical()
                .max_height(450.0)
                .show(ui, |ui| match *tab {
                    Tab::Roots => roots_tab(ui, &cache, &mut goto),
                    Tab::Origins => origins_tab(ui, &cache, &palette, &mut goto),
                    Tab::Security => security_tab(ui, &cache, &security, &mut goto),
                });
        });

    if new_mode != *mode {
        *mode = new_mode;
    }
    if let Some(e) = goto {
        selection.entity = Some(e);
        selection.focus = true;
    }
    Ok(())
}

fn build_cache(
    analysis: &GraphAnalysis,
    security: &SecurityIndex,
    nodes: &Query<(Entity, &Node, &Origin)>,
) -> Cache {
    let mut roots: Vec<_> = (0..analysis.entities.len())
        .filter(|&i| analysis.is_root[i])
        .filter_map(|i| {
            let e = analysis.entities[i];
            let (size, count) = analysis.exclusive[i];
            nodes
                .get(e)
                .ok()
                .map(|(_, n, _)| (e, n.Name.clone(), size, count))
        })
        .collect();
    roots.sort_by(|a, b| b.2.cmp(&a.2));

    let mut groups: BTreeMap<(OriginKind, String), OriginGroup> = BTreeMap::new();
    let mut pending: BTreeMap<usize, Vec<(Entity, String)>> = BTreeMap::new();
    let mut status_counts = BTreeMap::new();
    for (entity, node, origin) in nodes {
        let group = groups
            .entry((origin.kind, origin.label.clone()))
            .or_insert_with(|| OriginGroup {
                origin: origin.clone(),
                size: 0,
                packages: Vec::new(),
            });
        group.size += node.Size.unwrap_or(0);
        group.packages.push((entity, node.Name.clone()));

        if origin.kind != OriginKind::Key {
            *status_counts.entry(security.status(entity)).or_insert(0) += 1;
        }
        for &i in security
            .packages
            .get(&entity)
            .map(|p| p.pending.as_slice())
            .unwrap_or(&[])
        {
            pending
                .entry(i)
                .or_default()
                .push((entity, node.Name.clone()));
        }
    }

    let mut origins: Vec<OriginGroup> = groups.into_values().collect();
    for g in &mut origins {
        g.packages.sort_by(|a, b| a.1.cmp(&b.1));
    }
    // Sources tierces d'abord (les plus grosses en tête), puis Fedora, puis les clés.
    origins.sort_by_key(|g| {
        (
            !g.origin.is_third_party(),
            g.origin.kind,
            std::cmp::Reverse(g.packages.len()),
        )
    });

    let mut pending: Vec<PendingAdvisory> = pending
        .into_iter()
        .map(|(advisory, mut packages)| {
            packages.sort_by(|a, b| a.1.cmp(&b.1));
            PendingAdvisory { advisory, packages }
        })
        .collect();
    pending.sort_by_key(|p| {
        let a = &security.advisories[p.advisory];
        (!a.is_stable(), a.severity_rank(), a.alias.clone())
    });

    Cache {
        built: true,
        roots,
        origins,
        pending,
        status_counts,
    }
}

fn legend(ui: &mut egui::Ui, mode: ColorMode, layers: &LayerColors, cache: &Cache) {
    match mode {
        ColorMode::Layer => {
            ui.horizontal_wrapped(|ui| {
                swatch(ui, ROOT_COLOR, "root package");
                for (layer, color) in &layers.0 {
                    swatch(ui, *color, format!("layer {layer}"));
                }
            });
        }
        ColorMode::Origin => {
            ui.weak("Fedora in muted blue; see the Origins tab for each source's color.");
        }
        ColorMode::Security => {
            let count = |s| cache.status_counts.get(&s).copied().unwrap_or(0);
            for (status, text) in [
                (
                    SecurityStatus::FixAvailable,
                    "fix published: rebuild the image",
                ),
                (SecurityStatus::FixInTesting, "fix in updates-testing"),
                (SecurityStatus::UpToDate, "no pending Fedora advisory"),
                (
                    SecurityStatus::NotCovered,
                    "not tracked by Bodhi (third-party)",
                ),
            ] {
                swatch(
                    ui,
                    security_color(status),
                    format!("{} — {text}", count(status)),
                );
            }
        }
    }
}

fn roots_tab(ui: &mut egui::Ui, cache: &Cache, goto: &mut Option<Entity>) {
    ui.label(
        "Packages nothing depends on, sorted by exclusive size (what removing them would free).",
    );
    egui::Grid::new("root_packages")
        .num_columns(3)
        .striped(true)
        .show(ui, |ui| {
            for (e, name, size, count) in &cache.roots {
                if ui.link(name).clicked() {
                    *goto = Some(*e);
                }
                ui.label(human_size(*size));
                ui.weak(format!("{count} pkgs"));
                ui.end_row();
            }
        });
}

fn origins_tab(
    ui: &mut egui::Ui,
    cache: &Cache,
    palette: &OriginPalette,
    goto: &mut Option<Entity>,
) {
    let third_party: Vec<_> = cache
        .origins
        .iter()
        .filter(|g| g.origin.is_third_party())
        .collect();
    let n: usize = third_party.iter().map(|g| g.packages.len()).sum();
    let size: u64 = third_party.iter().map(|g| g.size).sum();
    ui.label(format!(
        "{n} third-party packages ({}) from {} sources.",
        human_size(size),
        third_party.len()
    ));
    for group in &cache.origins {
        let header = egui::RichText::new(format!(
            "● {} — {} packages, {}",
            group.origin.label,
            group.packages.len(),
            human_size(group.size)
        ))
        .color(to_egui(palette.color(&group.origin)));
        egui::CollapsingHeader::new(header)
            .id_salt(("origin", &group.origin.label))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for (e, name) in &group.packages {
                        if ui.link(name).clicked() {
                            *goto = Some(*e);
                        }
                    }
                });
            });
    }
}

fn security_tab(
    ui: &mut egui::Ui,
    cache: &Cache,
    security: &SecurityIndex,
    goto: &mut Option<Entity>,
) {
    let Some((release, fetched_at)) = &security.source else {
        ui.label("No advisory data.");
        ui.label("Run `image-scope advisories` and drop the file onto the window, or load an image from the Data window.");
        return;
    };
    ui.label(format!(
        "Fedora {release} security advisories (Bodhi), fetched {fetched_at}."
    ));
    ui.weak("Only Fedora packages are covered; third-party packages are not tracked by Bodhi.");
    if cache.pending.is_empty() {
        swatch(
            ui,
            security_color(SecurityStatus::UpToDate),
            "No pending security fix for the packages of this image.",
        );
        return;
    }
    ui.separator();
    for pending in &cache.pending {
        let advisory = &security.advisories[pending.advisory];
        advisory_ui(ui, advisory);
        ui.horizontal_wrapped(|ui| {
            ui.weak("Affects:");
            for (e, name) in &pending.packages {
                if ui.link(name).clicked() {
                    *goto = Some(*e);
                }
            }
        });
        ui.separator();
    }
}

/// En-tête d'un avis : statut, lien Bodhi, gravité, build correctif et CVE.
pub fn advisory_ui(ui: &mut egui::Ui, advisory: &Advisory) {
    let status = if advisory.is_stable() {
        SecurityStatus::FixAvailable
    } else {
        SecurityStatus::FixInTesting
    };
    ui.horizontal_wrapped(|ui| {
        ui.colored_label(to_egui(security_color(status)), "●");
        ui.hyperlink_to(&advisory.alias, &advisory.url);
        ui.weak(format!("{} · {}", advisory.severity, advisory.status));
        if let Some(date) = &advisory.date {
            ui.weak(date.split(' ').next().unwrap_or(date));
        }
    });
    ui.weak(format!("Fixed in {}", advisory.builds.join(", ")));
    cve_links(ui, &advisory.cves);
}

fn cve_links(ui: &mut egui::Ui, cves: &[String]) {
    const SHOWN: usize = 8;
    let link = |ui: &mut egui::Ui, cve: &String| {
        ui.hyperlink_to(cve, format!("https://www.cve.org/CVERecord?id={cve}"));
    };
    if cves.is_empty() {
        ui.weak("No CVE identifier in the advisory");
        return;
    }
    ui.horizontal_wrapped(|ui| {
        for cve in cves.iter().take(SHOWN) {
            link(ui, cve);
        }
    });
    if cves.len() > SHOWN {
        egui::CollapsingHeader::new(format!("{} more CVEs", cves.len() - SHOWN))
            .id_salt(("cves", &cves[0]))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for cve in &cves[SHOWN..] {
                        link(ui, cve);
                    }
                });
            });
    }
}
