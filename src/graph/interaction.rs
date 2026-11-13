//! Survol (infobulle + mise en évidence) et glisser-déposer des nœuds.
//!
//! - Survol : infobulle avec les infos essentielles, anneau blanc autour du nœud et
//!   arêtes directes éclaircies, curseur « main ».
//! - Glisser (clic gauche) : le nœud suit la souris, la simulation reprend de l'élan
//!   pour que les voisins s'adaptent (sauf si la mise en page est verrouillée). Une fois lâché, le nœud reste épinglé à sa place.
//! - Clic droit sur un nœud épinglé : le libère. « Unpin all » dans la fenêtre Forces.

use super::analysis::GraphAnalysis;
use super::coloring::{security_color, to_egui};
use super::graph::{Node, ROOT_COLOR};
use super::graph_ui::{Parameters, Simulation};
use super::origin::Origin;
use super::package_info::human_size;
use super::security::{SecurityIndex, SecurityStatus};
use bevy::prelude::*;
use bevy::window::{PrimaryWindow, SystemCursorIcon};
use bevy::winit::cursor::CursorIcon;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};

/// Nœud fixé par l'utilisateur : la simulation ne le déplace plus.
#[derive(Component)]
pub struct Pinned;

#[derive(Resource, Default)]
pub struct Hovered(pub Option<Entity>);

#[derive(Resource, Default)]
pub struct DragState {
    /// Nœud en cours de déplacement.
    pub entity: Option<Entity>,
    /// Écart entre le centre du nœud et la souris au moment de la saisie.
    offset: Vec2,
    /// Le dernier appui a-t-il donné lieu à un déplacement ? Sert à ne pas
    /// interpréter la fin d'un glisser comme un clic de sélection.
    pub moved: bool,
}

pub struct InteractionPlugin;

impl Plugin for InteractionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Hovered>()
            .init_resource::<DragState>()
            .add_observer(on_over)
            .add_observer(on_out)
            .add_observer(on_pressed)
            .add_observer(on_drag_start)
            .add_observer(on_drag)
            .add_observer(on_drag_end)
            .add_observer(on_right_click)
            .add_systems(Update, update_cursor)
            .add_systems(EguiPrimaryContextPass, hover_tooltip);
    }
}

fn on_over(
    trigger: Trigger<Pointer<Over>>,
    nodes: Query<(), With<Node>>,
    mut hovered: ResMut<Hovered>,
) {
    if nodes.contains(trigger.target()) {
        hovered.0 = Some(trigger.target());
    }
}

fn on_out(trigger: Trigger<Pointer<Out>>, mut hovered: ResMut<Hovered>) {
    if hovered.0 == Some(trigger.target()) {
        hovered.0 = None;
    }
}

fn on_pressed(trigger: Trigger<Pointer<Pressed>>, mut drag: ResMut<DragState>) {
    if trigger.event().button == PointerButton::Primary {
        drag.moved = false;
    }
}

fn pointer_world(cameras: &Query<(&Camera, &GlobalTransform)>, screen: Vec2) -> Option<Vec2> {
    let (camera, transform) = cameras.iter().next()?;
    camera.viewport_to_world_2d(transform, screen).ok()
}

fn on_drag_start(
    trigger: Trigger<Pointer<DragStart>>,
    mut commands: Commands,
    nodes: Query<&Transform, With<Node>>,
    cameras: Query<(&Camera, &GlobalTransform)>,
    mut drag: ResMut<DragState>,
    mut simulation: ResMut<Simulation>,
    params: Res<Parameters>,
) {
    if trigger.event().button != PointerButton::Primary {
        return;
    }
    let entity = trigger.target();
    let Ok(transform) = nodes.get(entity) else {
        return;
    };
    let Some(pointer) = pointer_world(&cameras, trigger.pointer_location.position) else {
        return;
    };
    drag.entity = Some(entity);
    drag.offset = transform.translation.truncate() - pointer;
    commands.entity(entity).insert(Pinned);
    simulation.reheat(&params);
}

fn on_drag(
    trigger: Trigger<Pointer<Drag>>,
    mut nodes: Query<&mut Transform, With<Node>>,
    cameras: Query<(&Camera, &GlobalTransform)>,
    mut drag: ResMut<DragState>,
) {
    let entity = trigger.target();
    if drag.entity != Some(entity) {
        return;
    }
    let Some(pointer) = pointer_world(&cameras, trigger.pointer_location.position) else {
        return;
    };
    if let Ok(mut transform) = nodes.get_mut(entity) {
        let z = transform.translation.z;
        transform.translation = (pointer + drag.offset).extend(z);
        drag.moved = true;
    }
}

fn on_drag_end(trigger: Trigger<Pointer<DragEnd>>, mut drag: ResMut<DragState>) {
    if drag.entity == Some(trigger.target()) {
        drag.entity = None;
    }
}

fn on_right_click(
    trigger: Trigger<Pointer<Click>>,
    mut commands: Commands,
    pinned: Query<(), (With<Node>, With<Pinned>)>,
    mut simulation: ResMut<Simulation>,
    params: Res<Parameters>,
) {
    if trigger.event().button == PointerButton::Secondary && pinned.contains(trigger.target()) {
        commands.entity(trigger.target()).remove::<Pinned>();
        simulation.reheat(&params);
    }
}

fn update_cursor(
    mut commands: Commands,
    window: Query<Entity, With<PrimaryWindow>>,
    hovered: Res<Hovered>,
    drag: Res<DragState>,
    mut current: Local<Option<SystemCursorIcon>>,
) {
    let wanted = if drag.entity.is_some() {
        SystemCursorIcon::Grabbing
    } else if hovered.0.is_some() {
        SystemCursorIcon::Pointer
    } else {
        SystemCursorIcon::Default
    };
    if *current == Some(wanted) {
        return;
    }
    if let Ok(window) = window.single() {
        commands.entity(window).insert(CursorIcon::System(wanted));
        *current = Some(wanted);
    }
}

fn hover_tooltip(
    mut contexts: EguiContexts,
    hovered: Res<Hovered>,
    drag: Res<DragState>,
    nodes: Query<(&Node, Has<Pinned>, &Origin)>,
    analysis: Option<Res<GraphAnalysis>>,
    security: Option<Res<SecurityIndex>>,
) -> Result {
    let Some(entity) = hovered.0 else {
        return Ok(());
    };
    if drag.entity.is_some() {
        return Ok(());
    }
    let Ok((node, pinned, origin)) = nodes.get(entity) else {
        return Ok(());
    };
    let ctx = contexts.ctx_mut()?;
    if ctx.is_pointer_over_area() {
        return Ok(());
    }

    let stats = analysis.as_ref().and_then(|a| {
        let i = *a.index.get(&entity)?;
        let dependents = a.dependents(false)[i].len();
        Some((
            a.is_root[i],
            a.dependencies[i].len(),
            dependents,
            a.exclusive[i],
        ))
    });

    egui::Tooltip::always_open(
        ctx.clone(),
        egui::LayerId::background(),
        egui::Id::new("node_hover"),
        egui::PopupAnchor::Pointer,
    )
    .gap(12.0)
    .show(|ui| {
        ui.set_max_width(320.0);
        ui.horizontal(|ui| {
            ui.strong(&node.Name);
            if let Some(version) = &node.Version {
                ui.weak(version);
            }
        });
        if let Some(summary) = &node.Summary {
            ui.label(summary);
        }
        ui.separator();
        if let Some(size) = node.Size {
            ui.label(format!("Size: {}", human_size(size)));
        }
        if let Some((is_root, deps, dependents, (excl_size, excl_count))) = stats {
            if is_root {
                ui.colored_label(to_egui(ROOT_COLOR), "● Root package");
            }
            ui.label(format!("{deps} dependencies · required by {dependents}"));
            ui.label(format!(
                "Exclusive: {} ({excl_count} packages)",
                human_size(excl_size)
            ));
        }
        if origin.is_third_party() {
            ui.label(format!("Origin: {}", origin.label));
        }
        if let Some(security) = security.as_deref().filter(|s| s.is_loaded()) {
            let status = security.status(entity);
            let text = match status {
                SecurityStatus::FixAvailable => Some("Security fix published"),
                SecurityStatus::FixInTesting => Some("Security fix in testing"),
                _ => None,
            };
            if let Some(text) = text {
                ui.colored_label(to_egui(security_color(status)), format!("● {text}"));
            }
        }
        ui.weak(if pinned {
            "Click: details · drag: move · right click: unpin"
        } else {
            "Click: details · drag: move"
        });
    });
    Ok(())
}
