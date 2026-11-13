//! Barre de recherche : trouver un paquet par son nom (ou son résumé).
//!
//! - `Ctrl+F` place le curseur dans la barre ;
//! - les résultats s'affichent dessous, les correspondances sont entourées dans le graphe ;
//! - `↑` / `↓` pour choisir, `Entrée` ou clic pour sélectionner et centrer la caméra ;
//! - `Échap` efface la recherche.

use super::coloring::{OriginPalette, to_egui};
use super::graph::Node;
use super::origin::Origin;
use super::package_info::Selection;
use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};

/// Nombre maximal de résultats affichés dans la liste.
const MAX_LISTED: usize = 50;

#[derive(Resource, Default)]
pub struct Search {
    pub query: String,
    /// Paquets correspondants, du plus pertinent au moins pertinent.
    pub matches: Vec<Entity>,
    highlighted: usize,
}

pub struct SearchPlugin;

impl Plugin for SearchPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Search>()
            .add_systems(EguiPrimaryContextPass, search_bar);
    }
}

/// Pertinence d'un paquet pour une requête en minuscules (plus petit = meilleur).
pub fn score(query: &str, name: &str, summary: Option<&str>) -> Option<u8> {
    if query.is_empty() {
        return None;
    }
    let name = name.to_lowercase();
    if name == query {
        Some(0)
    } else if name.starts_with(query) {
        Some(1)
    } else if name.contains(query) {
        Some(2)
    } else if query.len() >= 3 && summary.is_some_and(|s| s.to_lowercase().contains(query)) {
        Some(3)
    } else {
        None
    }
}

fn find_matches(query: &str, nodes: &Query<(Entity, &Node, &Origin)>) -> Vec<Entity> {
    let query = query.trim().to_lowercase();
    let mut scored: Vec<_> = nodes
        .iter()
        .filter_map(|(e, n, _)| {
            score(&query, &n.Name, n.Summary.as_deref())
                .map(|s| (s, n.Name.len(), n.Name.clone(), e))
        })
        .collect();
    scored.sort();
    scored.into_iter().map(|(.., e)| e).collect()
}

fn search_bar(
    mut contexts: EguiContexts,
    mut search: ResMut<Search>,
    mut selection: ResMut<Selection>,
    nodes: Query<(Entity, &Node, &Origin)>,
    palette: Option<Res<OriginPalette>>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    let search = search.as_mut();
    let shortcut = ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::F));
    let mut picked = None;

    egui::Area::new(egui::Id::new("search_bar"))
        .anchor(egui::Align2::CENTER_TOP, [0.0, 8.0])
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_width(380.0);
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut search.query)
                        .hint_text("🔍 Search a package (Ctrl+F)")
                        .desired_width(f32::INFINITY),
                );
                if shortcut {
                    edit.request_focus();
                }
                if edit.changed() {
                    search.matches = find_matches(&search.query, &nodes);
                    search.highlighted = 0;
                }

                let mut moved = false;
                if edit.has_focus() && !search.matches.is_empty() {
                    let last = search.matches.len().min(MAX_LISTED) - 1;
                    ui.input(|i| {
                        if i.key_pressed(egui::Key::ArrowDown) {
                            search.highlighted = (search.highlighted + 1).min(last);
                            moved = true;
                        }
                        if i.key_pressed(egui::Key::ArrowUp) {
                            search.highlighted = search.highlighted.saturating_sub(1);
                            moved = true;
                        }
                    });
                }
                if edit.lost_focus() {
                    if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        picked = search.matches.get(search.highlighted).copied();
                    } else if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        search.query.clear();
                        search.matches.clear();
                    }
                }

                if search.query.trim().is_empty() {
                    return;
                }
                if search.matches.is_empty() {
                    ui.weak("No package found");
                    return;
                }
                ui.weak(format!(
                    "{} match{} · ↑↓ to choose, Enter to select",
                    search.matches.len(),
                    if search.matches.len() > 1 { "es" } else { "" }
                ));
                egui::ScrollArea::vertical()
                    .max_height(320.0)
                    .show(ui, |ui| {
                        for (k, &entity) in search.matches.iter().take(MAX_LISTED).enumerate() {
                            let Ok((_, node, origin)) = nodes.get(entity) else {
                                continue;
                            };
                            let response = ui
                                .horizontal(|ui| {
                                    if let Some(palette) = &palette {
                                        ui.colored_label(to_egui(palette.color(origin)), "●");
                                    }
                                    let r =
                                        ui.selectable_label(k == search.highlighted, &node.Name);
                                    if let Some(summary) = &node.Summary {
                                        ui.add(
                                            egui::Label::new(egui::RichText::new(summary).weak())
                                                .truncate(),
                                        );
                                    }
                                    r
                                })
                                .inner;
                            if response.clicked() {
                                picked = Some(entity);
                            }
                            if moved && k == search.highlighted {
                                response.scroll_to_me(None);
                            }
                        }
                        if search.matches.len() > MAX_LISTED {
                            ui.weak(format!(
                                "… {} more, refine the search",
                                search.matches.len() - MAX_LISTED
                            ));
                        }
                    });
            });
        });

    if let Some(entity) = picked {
        selection.entity = Some(entity);
        selection.focus = true;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::score;

    #[test]
    fn ranks_exact_then_prefix_then_substring_then_summary() {
        assert_eq!(score("bash", "bash", None), Some(0));
        assert_eq!(score("bash", "bash-completion", None), Some(1));
        assert_eq!(score("ssh", "openssh", None), Some(2));
        assert_eq!(
            score("shell", "zsh", Some("Powerful interactive Shell")),
            Some(3)
        );
        assert_eq!(score("sh", "zsh", Some("Shell")), Some(2));
        assert_eq!(
            score("xyz", "bash", Some("The GNU Bourne Again shell")),
            None
        );
    }

    #[test]
    fn short_queries_ignore_summary() {
        assert_eq!(
            score("gn", "bash", Some("The GNU Bourne Again shell")),
            None
        );
    }

    #[test]
    fn is_case_insensitive_on_names() {
        assert_eq!(score("networkmanager", "NetworkManager", None), Some(0));
    }
}
