//! Inspection d'une image distante : trouve la base rpm dans ses couches et
//! produit les paquets (format de `packages-map.json`).
//!
//! Chaque couche qui modifie la base rpm en contient une copie complète. La dernière
//! donne les paquets de l'image ; les précédentes indiquent dans quelle couche chaque
//! paquet est apparu (`--all-layers`, il faut alors tout télécharger).
//!
//! Le résultat de chaque couche (base extraite, ou « pas de base ») est mis en cache
//! par digest : une couche inchangée d'un build à l'autre n'est plus téléchargée.

use super::cache_dir;
use super::registry::{Descriptor, ImageRef, Registry};
use super::rpmdb::{self, RPMDB_PATHS};
use crate::model::Node;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Couches téléchargées en parallèle.
const PARALLEL_DOWNLOADS: usize = 4;

pub struct Options {
    /// Parcourir toutes les couches pour savoir où chaque paquet est apparu.
    pub all_layers: bool,
    /// `os/architecture`, par défaut celle de la machine (linux/amd64…).
    pub platform: Option<String>,
}

pub struct Inspection {
    pub nodes: Vec<Node>,
    /// Image résolue (avec le digest du manifeste), pour l'affichage.
    pub image: String,
    pub reference: String,
    /// Dernier élément du dépôt : `niri-nvidia`.
    pub name: String,
    pub digest: String,
    pub created: Option<String>,
    /// Fedora `F45`, d'après les versions des paquets.
    pub release: Option<String>,
    pub title: Option<String>,
    pub version: Option<String>,
}

/// Messages de progression (affichés dans le terminal ou dans l'interface).
pub type Progress<'a> = &'a (dyn Fn(String) + Sync);

pub fn default_platform() -> (String, String) {
    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        "x86" => "386",
        "powerpc64" => "ppc64le",
        other => other,
    };
    ("linux".into(), arch.into())
}

enum LayerResult {
    Rpmdb(PathBuf),
    NoRpmdb,
}

pub fn inspect(input: &str, options: &Options, progress: Progress) -> Result<Inspection, String> {
    let reference = ImageRef::parse(input)?;
    let (os, arch) = match &options.platform {
        Some(p) => {
            let (os, arch) = p
                .split_once('/')
                .ok_or("platform must be os/arch, e.g. linux/amd64")?;
            (os.to_owned(), arch.to_owned())
        }
        None => default_platform(),
    };
    progress(format!("Resolving {reference} ({os}/{arch})"));
    let registry = Registry::new(reference.clone());
    let image = registry.resolve(&os, &arch)?;
    let total_size: u64 = image.layers.iter().map(|l| l.size).sum();
    progress(format!(
        "{} layers, {}",
        image.layers.len(),
        human_size(total_size)
    ));

    let cache = cache_dir().join("layers");
    std::fs::create_dir_all(&cache).map_err(|e| format!("{}: {e}", cache.display()))?;

    // Nom court d'une couche : début de son digest non compressé, comme l'ancien
    // script (`podman save`), sinon du digest compressé.
    let short = |i: usize| -> String {
        let id = image
            .config
            .rootfs
            .diff_ids
            .get(i)
            .unwrap_or(&image.layers[i].digest);
        id.trim_start_matches("sha256:").chars().take(7).collect()
    };

    // Couches à examiner, de la plus haute à la plus basse.
    let order: Vec<usize> = (0..image.layers.len()).rev().collect();
    let mut found: Vec<(usize, PathBuf)> = Vec::new();
    let done = AtomicUsize::new(0);
    for batch in order.chunks(if options.all_layers {
        order.len().max(1)
    } else {
        PARALLEL_DOWNLOADS
    }) {
        let results = scan_layers(&registry, &image.layers, batch, &cache, &done, progress)?;
        found.extend(batch.iter().zip(results).filter_map(|(&i, r)| match r {
            LayerResult::Rpmdb(path) => Some((i, path)),
            LayerResult::NoRpmdb => None,
        }));
        if !options.all_layers && !found.is_empty() {
            break;
        }
    }
    found.sort_by_key(|(i, _)| *i);
    let Some((last, final_db)) = found.last().cloned() else {
        return Err(format!(
            "{reference}: no rpm database ({}) in any layer",
            RPMDB_PATHS.join(", ")
        ));
    };
    progress(format!(
        "rpm database found in {} layer(s), last one in layer {} ({})",
        found.len(),
        last + 1,
        short(last)
    ));

    let packages = rpmdb::read_packages(&final_db)?;
    // Couche d'apparition : la première dont la base contient ce paquet (même NEVRA).
    let mut introduced: HashMap<String, String> = HashMap::new();
    for (i, db) in &found[..found.len() - 1] {
        for p in rpmdb::read_packages(db)? {
            if let Some(fullname) = p.node.fullname {
                introduced.entry(fullname).or_insert_with(|| short(*i));
            }
        }
    }
    let mut nodes = rpmdb::resolve_dependencies(packages);
    for node in &mut nodes {
        node.introduced_in = node
            .fullname
            .as_ref()
            .and_then(|f| introduced.get(f).cloned())
            .unwrap_or_else(|| short(last));
    }
    if found.len() > 1 {
        let earlier: std::collections::HashSet<_> = introduced.keys().collect();
        let removed = earlier.len()
            - nodes
                .iter()
                .filter(|n| n.fullname.as_ref().is_some_and(|f| earlier.contains(f)))
                .count();
        progress(format!(
            "{removed} packages of earlier layers were removed or updated"
        ));
    }
    progress(format!("{} packages", nodes.len()));

    let labels = image.config.config.labels.unwrap_or_default();
    Ok(Inspection {
        release: guess_release(&nodes),
        image: if image.digest.is_empty() {
            reference.to_string()
        } else {
            format!("{reference} ({})", &image.digest)
        },
        reference: reference.to_string(),
        name: reference
            .repository
            .rsplit('/')
            .next()
            .unwrap_or(&reference.repository)
            .to_owned(),
        digest: image.digest.clone(),
        created: labels.get("org.opencontainers.image.created").cloned(),
        title: labels.get("org.opencontainers.image.title").cloned(),
        version: labels.get("org.opencontainers.image.version").cloned(),
        nodes,
    })
}

/// Examine plusieurs couches en parallèle (résultats dans l'ordre de `batch`).
fn scan_layers(
    registry: &Registry,
    layers: &[Descriptor],
    batch: &[usize],
    cache: &Path,
    done: &AtomicUsize,
    progress: Progress,
) -> Result<Vec<LayerResult>, String> {
    let next = AtomicUsize::new(0);
    let collected: Vec<(usize, Result<LayerResult, String>)> = std::thread::scope(|s| {
        let workers: Vec<_> = (0..PARALLEL_DOWNLOADS.min(batch.len()))
            .map(|_| {
                s.spawn(|| {
                    let mut out = Vec::new();
                    loop {
                        let k = next.fetch_add(1, Ordering::Relaxed);
                        let Some(&i) = batch.get(k) else { break };
                        let result = scan_layer(registry, &layers[i], cache, |msg| {
                            progress(format!("layer {}/{}: {msg}", i + 1, layers.len()))
                        });
                        let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                        if let Ok(r) = &result {
                            let what = match r {
                                LayerResult::Rpmdb(_) => "rpm database",
                                LayerResult::NoRpmdb => "no rpm database",
                            };
                            progress(format!(
                                "layer {}/{}: {what} ({n} scanned)",
                                i + 1,
                                layers.len()
                            ));
                        }
                        out.push((k, result));
                    }
                    out
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|w| w.join().expect("layer worker panicked"))
            .collect()
    });
    let mut results: Vec<Option<LayerResult>> = (0..batch.len()).map(|_| None).collect();
    for (k, result) in collected {
        results[k] = Some(result?);
    }
    Ok(results
        .into_iter()
        .map(|r| r.expect("every layer is scanned"))
        .collect())
}

fn scan_layer(
    registry: &Registry,
    layer: &Descriptor,
    cache: &Path,
    progress: impl Fn(String),
) -> Result<LayerResult, String> {
    let key = layer.digest.replace(':', "-");
    let db_path = cache.join(format!("{key}.rpmdb.sqlite"));
    let none_marker = cache.join(format!("{key}.none"));
    if db_path.exists() {
        return Ok(LayerResult::Rpmdb(db_path));
    }
    if none_marker.exists() {
        return Ok(LayerResult::NoRpmdb);
    }

    progress(format!("downloading {}", human_size(layer.size)));
    let reader = registry.layer(layer)?;
    let mut archive = tar::Archive::new(reader);
    let entries = archive
        .entries()
        .map_err(|e| format!("{}: {e}", layer.digest))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| format!("{}: {e}", layer.digest))?;
        let path = entry.path().map_err(|e| e.to_string())?.into_owned();
        let path = path.to_string_lossy();
        let path = path.trim_start_matches("./").trim_start_matches('/');
        if entry.header().entry_type().is_file() && RPMDB_PATHS.contains(&path) {
            // On s'arrête là : inutile de lire (et télécharger) le reste de la couche.
            let tmp = cache.join(format!("{key}.part"));
            let mut out =
                std::fs::File::create(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
            std::io::copy(&mut entry, &mut out).map_err(|e| format!("{}: {e}", layer.digest))?;
            std::fs::rename(&tmp, &db_path).map_err(|e| e.to_string())?;
            return Ok(LayerResult::Rpmdb(db_path));
        }
    }
    std::fs::write(&none_marker, b"").map_err(|e| e.to_string())?;
    Ok(LayerResult::NoRpmdb)
}

/// Lit une base locale, par exemple celle du système : `/usr/lib/sysimage/rpm/rpmdb.sqlite`.
pub fn inspect_rpmdb(path: &Path) -> Result<Vec<Node>, String> {
    let mut nodes = rpmdb::resolve_dependencies(rpmdb::read_packages(path)?);
    for node in &mut nodes {
        node.introduced_in = "local".into();
    }
    Ok(nodes)
}

/// Release Fedora la plus fréquente parmi les paquets (`…fc45` → `F45`).
pub fn guess_release(nodes: &[Node]) -> Option<String> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for release in nodes.iter().filter_map(|n| n.Release.as_deref()) {
        if let Some(i) = release.find(".fc") {
            let digits = &release[i + 3..];
            let end = digits
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(digits.len());
            if end > 0 {
                *counts.entry(&digits[..end]).or_default() += 1;
            }
        }
    }
    counts
        .into_iter()
        .max_by_key(|&(v, c)| (c, v.to_owned()))
        .map(|(v, _)| format!("F{v}"))
}

pub fn human_size(bytes: u64) -> String {
    let mut size = bytes as f64;
    for unit in ["B", "KiB", "MiB", "GiB"] {
        if size < 1024.0 {
            return format!("{size:.1} {unit}");
        }
        size /= 1024.0;
    }
    format!("{size:.1} TiB")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guesses_fedora_release() {
        let node = |r: &str| -> Node {
            serde_json::from_value(
                serde_json::json!({"Name": "x", "Release": r, "introduced_in": ""}),
            )
            .unwrap()
        };
        let nodes = [node("1.fc45"), node("3.fc45.1"), node("2.fc44"), node("1")];
        assert_eq!(guess_release(&nodes).as_deref(), Some("F45"));
        assert_eq!(guess_release(&[]), None);
    }
}
