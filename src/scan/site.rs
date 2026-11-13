//! Site publié (GitHub Pages) : un dossier par image et par build, un index
//! `builds.json` lu par la visionneuse web, et un rapport markdown pour la CI.
//!
//! ```text
//! site/
//! ├── builds.json
//! └── niri-nvidia/
//!     └── 20261112-2228/
//!         ├── image.json
//!         ├── packages-map.json
//!         └── security-advisories.json
//! ```

use super::advisories::now_iso8601;
use super::inspect::{Progress, human_size};
use crate::model::{BuildEntry, BuildIndex, BuildInfo, Node};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

pub const INDEX_FILE: &str = "builds.json";

/// Identifiant d'un build, triable : `20261112-2228` (date de création de l'image,
/// sinon date de l'analyse).
pub fn build_id(created: Option<&str>, inspected_at: &str) -> String {
    let digits: String = created
        .unwrap_or(inspected_at)
        .chars()
        .filter(char::is_ascii_digit)
        .take(12)
        .collect();
    if digits.len() == 12 {
        format!("{}-{}", &digits[..8], &digits[8..])
    } else {
        digits
    }
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

fn subdirs(dir: &Path) -> Result<Vec<String>, String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| !n.starts_with('.'))
        .collect();
    names.sort();
    Ok(names)
}

/// Écrit `builds.json` ; avec `keep`, supprime les builds au-delà des N plus récents.
pub fn index(dir: &Path, keep: Option<usize>, progress: Progress) -> Result<BuildIndex, String> {
    let mut builds = Vec::new();
    for name in subdirs(dir)? {
        let mut ids: Vec<String> = subdirs(&dir.join(&name))?
            .into_iter()
            .filter(|id| dir.join(&name).join(id).join("image.json").exists())
            .collect();
        ids.sort_by(|a, b| b.cmp(a));
        if let Some(keep) = keep {
            for old in ids.split_off(keep.min(ids.len())) {
                let path = dir.join(&name).join(&old);
                std::fs::remove_dir_all(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                progress(format!("Removed old build {name}/{old}"));
            }
        }
        for id in ids {
            let build_dir = dir.join(&name).join(&id);
            let info: BuildInfo = read_json(&build_dir.join("image.json"))?;
            builds.push(BuildEntry {
                path: format!("{name}/{id}"),
                has_advisories: build_dir.join("security-advisories.json").exists(),
                id,
                info,
            });
        }
    }
    let index = BuildIndex {
        generated_at: now_iso8601(),
        builds,
    };
    let path = dir.join(INDEX_FILE);
    let json = serde_json::to_string_pretty(&index).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| format!("{}: {e}", path.display()))?;
    progress(format!(
        "Saved {} builds to {}",
        index.builds.len(),
        path.display()
    ));
    Ok(index)
}

/// Différences de paquets entre deux builds (clé : nom + architecture).
#[derive(Default, Debug, PartialEq)]
pub struct Diff {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    /// `nom : ancienne → nouvelle version`.
    pub updated: Vec<String>,
    pub size_delta: i64,
}

pub fn diff(old: &[Node], new: &[Node]) -> Diff {
    let key = |n: &Node| format!("{}.{}", n.Name, n.Arch.as_deref().unwrap_or(""));
    let version = |n: &Node| {
        format!(
            "{}-{}",
            n.Version.as_deref().unwrap_or("?"),
            n.Release.as_deref().unwrap_or("?")
        )
    };
    // gpg-pubkey peut apparaître plusieurs fois : on garde des listes.
    let index = |nodes: &[Node]| {
        let mut map: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for n in nodes {
            map.entry(key(n)).or_default().push(version(n));
        }
        map
    };
    let (before, after) = (index(old), index(new));
    let mut d = Diff::default();
    for (k, versions) in &after {
        match before.get(k) {
            None => d.added.push(format!("{k} {}", versions.join(", "))),
            Some(previous) if previous != versions => d.updated.push(format!(
                "{k}: {} → {}",
                previous.join(", "),
                versions.join(", ")
            )),
            Some(_) => {}
        }
    }
    for (k, versions) in &before {
        if !after.contains_key(k) {
            d.removed.push(format!("{k} {}", versions.join(", ")));
        }
    }
    let size = |nodes: &[Node]| nodes.iter().filter_map(|n| n.Size).sum::<u64>() as i64;
    d.size_delta = size(new) - size(old);
    d
}

fn signed_size(delta: i64) -> String {
    let sign = if delta < 0 { "−" } else { "+" };
    format!("{sign}{}", human_size(delta.unsigned_abs()))
}

fn details(out: &mut String, title: &str, items: &[String]) {
    const SHOWN: usize = 100;
    if items.is_empty() {
        return;
    }
    out.push_str(&format!(
        "<details><summary>{title} ({})</summary>\n\n",
        items.len()
    ));
    for item in items.iter().take(SHOWN) {
        out.push_str(&format!("- `{item}`\n"));
    }
    if items.len() > SHOWN {
        out.push_str(&format!("- … {} more\n", items.len() - SHOWN));
    }
    out.push_str("\n</details>\n\n");
}

/// Rapport markdown : dernier build de chaque image et changements depuis le précédent.
pub fn summary(dir: &Path, index: &BuildIndex, url: Option<&str>) -> Result<String, String> {
    let mut by_image: Vec<(&str, Vec<&BuildEntry>)> = Vec::new();
    for b in &index.builds {
        match by_image.last_mut() {
            Some((name, list)) if *name == b.info.name => list.push(b),
            _ => by_image.push((&b.info.name, vec![b])),
        }
    }
    let link = |b: &BuildEntry| {
        url.map(|u| {
            format!(
                "[open]({}/?image={}&build={})",
                u.trim_end_matches('/'),
                b.info.name,
                b.id
            )
        })
        .unwrap_or_default()
    };

    let mut table = String::from(
        "## image-scope\n\n| Image | Version | Packages | Size | Since previous build | Security fixes to pick up | |\n|---|---|---|---|---|---|---|\n",
    );
    let mut sections = String::new();
    let mut cache: HashMap<String, Vec<Node>> = HashMap::new();
    let mut load = |b: &BuildEntry| -> Result<Vec<Node>, String> {
        if let Some(nodes) = cache.get(&b.path) {
            return Ok(nodes.clone());
        }
        let nodes: Vec<Node> = read_json(&dir.join(&b.path).join("packages-map.json"))?;
        cache.insert(b.path.clone(), nodes.clone());
        Ok(nodes)
    };
    for (name, builds) in &by_image {
        let latest = builds[0];
        let changes = match builds.get(1) {
            Some(previous) => {
                let d = diff(&load(previous)?, &load(latest)?);
                details(&mut sections, &format!("{name}: added"), &d.added);
                details(&mut sections, &format!("{name}: removed"), &d.removed);
                details(&mut sections, &format!("{name}: updated"), &d.updated);
                format!(
                    "+{} −{}, {} updated, {}",
                    d.added.len(),
                    d.removed.len(),
                    d.updated.len(),
                    signed_size(d.size_delta)
                )
            }
            None => "first build".into(),
        };
        let fixes = match latest.info.pending_fixes {
            Some(0) => "none".to_owned(),
            Some(n) => format!("**{n} packages**"),
            None => "unknown".to_owned(),
        };
        table.push_str(&format!(
            "| {name} | {} | {} | {} | {changes} | {fixes} | {} |\n",
            latest.info.version.as_deref().unwrap_or(&latest.id),
            latest.info.packages,
            human_size(latest.info.size),
            link(latest)
        ));
    }
    table.push('\n');
    if by_image
        .iter()
        .any(|(_, b)| b[0].info.pending_fixes.unwrap_or(0) > 0)
    {
        table.push_str("Security fixes to pick up: Fedora security updates published after the packages of the image were installed; the next build should include them.\n\n");
    }
    Ok(table + &sections)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(name: &str, version: &str, size: u64) -> Node {
        serde_json::from_value(serde_json::json!({
            "Name": name, "Version": version, "Release": "1.fc45", "Arch": "x86_64",
            "Size": size, "introduced_in": ""
        }))
        .unwrap()
    }

    #[test]
    fn builds_sortable_ids() {
        assert_eq!(
            build_id(Some("2026-11-12T22:28:36.596618957+00:00"), "x"),
            "20261112-2228"
        );
        assert_eq!(build_id(None, "2026-11-13T12:21:17+0000"), "20261113-1221");
    }

    #[test]
    fn diffs_packages() {
        let old = [
            node("bash", "5.2", 100),
            node("htop", "3.3", 10),
            node("vim", "9.1", 50),
        ];
        let new = [
            node("bash", "5.3", 120),
            node("vim", "9.1", 50),
            node("helix", "25.07", 300),
        ];
        let d = diff(&old, &new);
        assert_eq!(d.added, ["helix.x86_64 25.07-1.fc45"]);
        assert_eq!(d.removed, ["htop.x86_64 3.3-1.fc45"]);
        assert_eq!(d.updated, ["bash.x86_64: 5.2-1.fc45 → 5.3-1.fc45"]);
        assert_eq!(d.size_delta, 310);
    }
}
