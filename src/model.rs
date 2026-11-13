//! Données échangées entre l'analyse (`image-scope inspect`) et la visionneuse :
//! paquets (`packages-map.json`), avis de sécurité (`security-advisories.json`),
//! et correspondance entre les deux (comparaison de versions rpm).
//!
//! Sans Bevy : utilisé aussi par la ligne de commande seule (`--no-default-features`).
#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::HashMap;

/// Un paquet installé.
#[derive(Deserialize, Serialize, Clone, Debug)]
#[cfg_attr(feature = "viewer", derive(bevy::prelude::Component))]
#[allow(non_snake_case)]
pub struct Node {
    pub Name: String,
    pub Version: Option<String>,
    pub Release: Option<String>,
    pub Arch: Option<String>,
    pub Installtime: Option<u64>,
    pub Group: Option<String>,
    pub Size: Option<u64>,
    pub License: Option<String>,
    pub Sourcerpm: Option<String>,
    pub Buildtime: Option<u64>,
    pub Buildhost: Option<String>,
    pub Packager: Option<String>,
    pub Vendor: Option<String>,
    pub Url: Option<String>,
    pub Bugurl: Option<String>,
    pub Summary: Option<String>,
    pub Description: Option<String>,
    pub introduced_in: String,
    #[serde(default)]
    pub dep: Vec<String>,
    #[serde(default)]
    pub dropped: bool,
    pub fullname: Option<String>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Advisory {
    pub alias: String,
    pub status: String,
    pub severity: String,
    pub date: Option<String>,
    pub url: String,
    pub builds: Vec<String>,
    pub cves: Vec<String>,
}

impl Advisory {
    pub fn is_stable(&self) -> bool {
        self.status == "stable"
    }

    /// Rang pour trier : plus petit = plus grave.
    pub fn severity_rank(&self) -> u8 {
        match self.severity.as_str() {
            "urgent" => 0,
            "high" => 1,
            "medium" => 2,
            "low" => 3,
            _ => 4,
        }
    }
}

/// Contenu de `security-advisories.json` (voir `image-scope advisories`).
#[derive(Deserialize, Serialize, Debug)]
pub struct AdvisoryFile {
    pub release: String,
    pub fetched_at: String,
    pub advisories: Vec<Advisory>,
}

impl AdvisoryFile {
    #[allow(dead_code)]
    pub fn new(release: String, fetched_at: String, advisories: Vec<Advisory>) -> Self {
        AdvisoryFile {
            release,
            fetched_at,
            advisories,
        }
    }

    pub fn summary(&self) -> String {
        format!(
            "Fedora {}, {} advisories, fetched {}",
            self.release,
            self.advisories.len(),
            self.fetched_at
        )
    }
}

#[derive(Default, Debug, Clone)]
pub struct PackageSecurity {
    /// Avis dont le build est plus récent que la version installée.
    pub pending: Vec<usize>,
    /// Avis déjà corrigés dans la version installée.
    pub fixed: Vec<usize>,
    pub covered: bool,
}

/// Associe les avis aux paquets. `entries` : (clé, RPM source, paquet Fedora ?).
pub fn match_advisories<K: Copy + Eq + std::hash::Hash>(
    advisories: &[Advisory],
    entries: &[(K, Option<&str>, bool)],
) -> HashMap<K, PackageSecurity> {
    // Nom du paquet source → (avis, version, release) de chaque build concerné.
    let mut by_source: HashMap<&str, Vec<(usize, &str, &str)>> = HashMap::new();
    for (i, advisory) in advisories.iter().enumerate() {
        for build in &advisory.builds {
            if let Some((name, version, release)) = split_nvr(build) {
                by_source
                    .entry(name)
                    .or_default()
                    .push((i, version, release));
            }
        }
    }

    entries
        .iter()
        .map(|&(key, sourcerpm, is_fedora)| {
            let mut security = PackageSecurity::default();
            let installed = sourcerpm
                .map(|s| s.trim_end_matches(".src.rpm"))
                .and_then(split_nvr)
                .filter(|_| is_fedora);
            if let Some((name, version, release)) = installed {
                security.covered = true;
                for &(i, v, r) in by_source.get(name).into_iter().flatten() {
                    if compare_evr((v, r), (version, release)) == Ordering::Greater {
                        security.pending.push(i);
                    } else {
                        security.fixed.push(i);
                    }
                }
                security.pending.dedup();
                security.fixed.dedup();
            }
            (key, security)
        })
        .collect()
}

/// `nom-version-release` → (nom, version, release). Le nom peut contenir des tirets.
pub fn split_nvr(nvr: &str) -> Option<(&str, &str, &str)> {
    let (rest, release) = nvr.rsplit_once('-')?;
    let (name, version) = rest.rsplit_once('-')?;
    Some((name, version, release))
}

fn compare_evr(a: (&str, &str), b: (&str, &str)) -> Ordering {
    rpmvercmp(a.0, b.0).then_with(|| rpmvercmp(a.1, b.1))
}

/// Comparaison de versions RPM (port de `rpmvercmp` de rpm ≥ 4.15, `~` et `^` compris).
pub fn rpmvercmp(a: &str, b: &str) -> Ordering {
    if a == b {
        return Ordering::Equal;
    }
    let (mut one, mut two) = (a.as_bytes(), b.as_bytes());
    let is_sep = |c: u8| !c.is_ascii_alphanumeric() && c != b'~' && c != b'^';

    loop {
        while one.first().is_some_and(|&c| is_sep(c)) {
            one = &one[1..];
        }
        while two.first().is_some_and(|&c| is_sep(c)) {
            two = &two[1..];
        }

        // `~` trie avant tout le reste (pré-versions : 1.0~rc1 < 1.0).
        if one.first() == Some(&b'~') || two.first() == Some(&b'~') {
            if one.first() != Some(&b'~') {
                return Ordering::Greater;
            }
            if two.first() != Some(&b'~') {
                return Ordering::Less;
            }
            one = &one[1..];
            two = &two[1..];
            continue;
        }

        // `^` trie après la version de base, mais avant toute suite « normale »
        // (snapshots : 1.0 < 1.0^git1 < 1.0.1).
        if one.first() == Some(&b'^') || two.first() == Some(&b'^') {
            if one.is_empty() {
                return Ordering::Less;
            }
            if two.is_empty() {
                return Ordering::Greater;
            }
            if one.first() != Some(&b'^') {
                return Ordering::Greater;
            }
            if two.first() != Some(&b'^') {
                return Ordering::Less;
            }
            one = &one[1..];
            two = &two[1..];
            continue;
        }

        if one.is_empty() || two.is_empty() {
            break;
        }

        let is_num = one[0].is_ascii_digit();
        let take = |s: &[u8]| {
            let len = s
                .iter()
                .take_while(|c| {
                    if is_num {
                        c.is_ascii_digit()
                    } else {
                        c.is_ascii_alphabetic()
                    }
                })
                .count();
            len
        };
        let (len_one, len_two) = (take(one), take(two));
        let (seg_one, seg_two) = (&one[..len_one], &two[..len_two]);
        one = &one[len_one..];
        two = &two[len_two..];

        // Segments de types différents : le numérique est le plus récent.
        if seg_two.is_empty() {
            return if is_num {
                Ordering::Greater
            } else {
                Ordering::Less
            };
        }

        let ordering = if is_num {
            let trim = |s: &[u8]| {
                let zeros = s.iter().take_while(|&&c| c == b'0').count();
                s[zeros..].to_vec()
            };
            let (n1, n2) = (trim(seg_one), trim(seg_two));
            n1.len().cmp(&n2.len()).then_with(|| n1.cmp(&n2))
        } else {
            seg_one.cmp(seg_two)
        };
        if ordering != Ordering::Equal {
            return ordering;
        }
    }

    match (one.is_empty(), two.is_empty()) {
        (true, true) => Ordering::Equal,
        (false, _) => Ordering::Greater,
        (true, false) => Ordering::Less,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Ordering::*;

    #[test]
    fn rpmvercmp_matches_rpm() {
        // Cas tirés de la suite de tests de rpm (rpmvercmp.at).
        let cases = [
            ("1.0", "1.0", Equal),
            ("1.0", "2.0", Less),
            ("2.0.1", "2.0", Greater),
            ("5.5p1", "5.5p2", Less),
            ("5.5p10", "5.5p1", Greater),
            ("10xyz", "10.1xyz", Less),
            ("xyz10", "xyz10.1", Less),
            ("1.0aa", "1.0a", Greater),
            ("10.0001", "10.1", Equal),
            ("10.0001", "10.0039", Less),
            ("4.999.9", "5.0", Less),
            ("20101121", "20101122", Less),
            ("2_0", "2_0", Equal),
            ("2.0", "2_0", Equal),
            ("a", "1", Less),
            ("1", "a", Greater),
            ("a+", "a_", Equal),
            ("+", "_", Equal),
            ("1.0~rc1", "1.0", Less),
            ("1.0~rc1", "1.0~rc2", Less),
            ("1.0~rc1~git123", "1.0~rc1", Less),
            ("1.0^", "1.0", Greater),
            ("1.0^git1", "1.0", Greater),
            ("1.0^git1", "1.01", Less),
            ("1.0^20160101", "1.0.1", Less),
            ("1.0^20160101^git1", "1.0^20160101", Greater),
            ("1.0~rc1^git1", "1.0~rc1", Greater),
            ("1.0^git1~pre", "1.0^git1", Less),
        ];
        for (a, b, expected) in cases {
            assert_eq!(rpmvercmp(a, b), expected, "{a} vs {b}");
            assert_eq!(rpmvercmp(b, a), expected.reverse(), "{b} vs {a}");
        }
    }

    #[test]
    fn splits_nvr_with_dashes_in_name() {
        assert_eq!(
            split_nvr("NetworkManager-l2tp-1.52.6-1.fc45"),
            Some(("NetworkManager-l2tp", "1.52.6", "1.fc45"))
        );
    }

    fn advisory(alias: &str, status: &str, build: &str) -> Advisory {
        Advisory {
            alias: alias.into(),
            status: status.into(),
            severity: "high".into(),
            date: None,
            url: String::new(),
            builds: vec![build.into()],
            cves: vec![],
        }
    }

    #[test]
    fn matches_pending_and_fixed_advisories() {
        let advisories = [
            advisory("NEW", "stable", "bind-9.18.51^git20261102.973d8646-1.fc45"),
            advisory("OLD", "stable", "bind-9.18.50-1.fc45"),
            advisory("OTHER", "testing", "rsync-3.5.2-2.fc45"),
        ];
        let entries = [
            (
                0,
                Some("bind-9.18.51^git20261003.d0a87358-12.fc45.src.rpm"),
                true,
            ),
            (1, Some("rsync-3.5.2-2.fc45.src.rpm"), true),
            (2, Some("rsync-3.5.1-1.fc45.src.rpm"), false),
        ];
        let m = match_advisories(&advisories, &entries);
        assert_eq!(m[&0].pending, vec![0]);
        assert_eq!(m[&0].fixed, vec![1]);
        assert!(m[&1].pending.is_empty() && m[&1].fixed == vec![2]);
        assert!(!m[&2].covered, "third-party packages are not covered");
    }
}

/// Fiche d'un build analysé (`image.json`, à côté de `packages-map.json`).
#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct BuildInfo {
    /// Référence analysée, ex. `ghcr.io/benoitlx/niri-nvidia:latest`.
    pub image: String,
    /// Nom court (dernier élément du dépôt), ex. `niri-nvidia`.
    pub name: String,
    pub digest: String,
    /// Label `org.opencontainers.image.version`.
    pub version: Option<String>,
    /// Label `org.opencontainers.image.created`.
    pub created: Option<String>,
    pub inspected_at: String,
    /// Release Fedora (`F45`).
    pub release: Option<String>,
    pub packages: usize,
    pub size: u64,
    /// Paquets avec un correctif de sécurité publié (stable), si les avis sont connus.
    pub pending_fixes: Option<usize>,
}

/// Un build dans l'index : sa fiche et son dossier (relatif à `builds.json`).
#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct BuildEntry {
    pub id: String,
    pub path: String,
    pub has_advisories: bool,
    #[serde(flatten)]
    pub info: BuildInfo,
}

/// `builds.json` : builds de chaque image, du plus récent au plus ancien.
#[derive(Deserialize, Serialize, Debug, Clone, Default)]
pub struct BuildIndex {
    pub generated_at: String,
    pub builds: Vec<BuildEntry>,
}

/// Paquets d'un build ayant un correctif publié (stable) plus récent que l'installé.
pub fn pending_fixes(nodes: &[Node], advisories: &AdvisoryFile) -> usize {
    let entries: Vec<(usize, Option<&str>, bool)> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let fedora = n.Vendor.as_deref() == Some("Fedora Project");
            (i, n.Sourcerpm.as_deref(), fedora)
        })
        .collect();
    match_advisories(&advisories.advisories, &entries)
        .values()
        .filter(|p| {
            p.pending
                .iter()
                .any(|&a| advisories.advisories[a].is_stable())
        })
        .count()
}
