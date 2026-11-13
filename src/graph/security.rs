//! Avis de sécurité Fedora (Bodhi) appliqués aux paquets de l'image.
//!
//! `image-scope inspect` (ou `advisories`) télécharge les mises à jour de sécurité de la release
//! dans `security-advisories.json`. Pour chaque paquet Fedora, on compare la version de
//! son RPM source à celle des builds de chaque avis :
//! - build plus récent que l'installé → correctif disponible (stable ou en test) ;
//! - sinon → avis déjà corrigé dans la version installée.
//!
//! Bodhi ne suit que les paquets Fedora : les paquets tiers ne sont pas couverts.

use super::graph::Node;
use super::loading::AdvisoriesData;
use super::origin::{Origin, OriginKind};
pub use crate::model::{Advisory, AdvisoryFile, PackageSecurity, match_advisories};
use bevy::prelude::*;
use std::collections::HashMap;

pub const ADVISORIES_FILE: &str = "security-advisories.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SecurityStatus {
    /// Correctif publié (stable) plus récent que la version installée.
    FixAvailable,
    /// Correctif en test seulement.
    FixInTesting,
    /// Aucun correctif en attente.
    UpToDate,
    /// Paquet tiers ou sans RPM source : Bodhi ne le suit pas.
    NotCovered,
}

#[derive(Resource, Default)]
pub struct SecurityIndex {
    /// `None` si `security-advisories.json` est absent ou illisible.
    pub source: Option<(String, String)>,
    pub advisories: Vec<Advisory>,
    pub packages: HashMap<Entity, PackageSecurity>,
}

impl SecurityIndex {
    pub fn is_loaded(&self) -> bool {
        self.source.is_some()
    }

    pub fn status(&self, entity: Entity) -> SecurityStatus {
        let Some(p) = self.packages.get(&entity).filter(|p| p.covered) else {
            return SecurityStatus::NotCovered;
        };
        if p.pending.iter().any(|&i| self.advisories[i].is_stable()) {
            SecurityStatus::FixAvailable
        } else if !p.pending.is_empty() {
            SecurityStatus::FixInTesting
        } else {
            SecurityStatus::UpToDate
        }
    }
}

pub(super) fn load_security(
    mut commands: Commands,
    nodes: Query<(Entity, &Node, &Origin)>,
    data: Option<Res<AdvisoriesData>>,
) {
    let Some(data) = data else {
        info!("No {ADVISORIES_FILE}: run `image-scope advisories` to get CVE data");
        commands.insert_resource(SecurityIndex::default());
        return;
    };
    let file = &data.file;

    let entries: Vec<_> = nodes
        .iter()
        .map(|(e, n, o)| (e, n.Sourcerpm.as_deref(), o.kind == OriginKind::Fedora))
        .collect();
    let packages = match_advisories(&file.advisories, &entries);

    let n_pending = packages.values().filter(|p| !p.pending.is_empty()).count();
    info!(
        "{} advisories for {}: {n_pending} packages with a pending security fix",
        file.advisories.len(),
        file.release
    );
    commands.insert_resource(SecurityIndex {
        source: Some((file.release.clone(), file.fetched_at.clone())),
        advisories: file.advisories.clone(),
        packages,
    });
}
