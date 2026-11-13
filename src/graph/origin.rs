//! Origine d'un paquet : Fedora, COPR, autre fournisseur (negativo17, NVIDIA, …).

use super::graph::Node;
use bevy::prelude::Component;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum OriginKind {
    Fedora,
    Copr,
    Vendor,
    Unknown,
    /// Pseudo-paquets `gpg-pubkey` (clés des dépôts).
    Key,
}

#[derive(Component, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Origin {
    pub kind: OriginKind,
    pub label: String,
}

impl Origin {
    pub fn is_third_party(&self) -> bool {
        matches!(
            self.kind,
            OriginKind::Copr | OriginKind::Vendor | OriginKind::Unknown
        )
    }

    pub fn classify(node: &Node) -> Self {
        let known = |s: &Option<String>| {
            s.as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty() && !matches!(*s, "none" | "(none)"))
                .map(str::to_owned)
        };

        let (kind, label) = if node.Name == "gpg-pubkey" {
            (OriginKind::Key, "GPG keys".to_owned())
        } else if let Some(vendor) = known(&node.Vendor) {
            if vendor == "Fedora Project" {
                (OriginKind::Fedora, "Fedora".to_owned())
            } else if let Some(user) = vendor.strip_prefix("Fedora Copr - user ") {
                (OriginKind::Copr, format!("COPR {user}"))
            } else {
                (OriginKind::Vendor, vendor)
            }
        } else if let Some(name) = known(&node.Packager)
            .map(|p| p.split('<').next().unwrap_or("").trim().to_owned())
            .filter(|p| !p.is_empty())
        {
            // Pas de Vendor, mais un packager nommé (ex. « Tailscale Inc »).
            (OriginKind::Vendor, name)
        } else {
            (OriginKind::Unknown, "Unknown (local build?)".to_owned())
        };
        Origin { kind, label }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(name: &str, vendor: Option<&str>, packager: Option<&str>) -> Node {
        serde_json::from_value(serde_json::json!({
            "Name": name,
            "Vendor": vendor,
            "Packager": packager,
            "introduced_in": "0",
        }))
        .unwrap()
    }

    fn classify(name: &str, vendor: Option<&str>, packager: Option<&str>) -> (OriginKind, String) {
        let o = Origin::classify(&node(name, vendor, packager));
        (o.kind, o.label)
    }

    #[test]
    fn classifies_real_packages() {
        assert_eq!(
            classify("bash", Some("Fedora Project"), Some("Fedora Project")),
            (OriginKind::Fedora, "Fedora".into())
        );
        assert_eq!(
            classify("yazi", Some("Fedora Copr - user lihaohong"), None),
            (OriginKind::Copr, "COPR lihaohong".into())
        );
        assert_eq!(
            classify("ffmpeg", Some("negativo17.org"), Some("negativo17.org")),
            (OriginKind::Vendor, "negativo17.org".into())
        );
        assert_eq!(
            classify(
                "tailscale",
                None,
                Some("Tailscale Inc <info@tailscale.com>")
            ),
            (OriginKind::Vendor, "Tailscale Inc".into())
        );
        assert_eq!(
            classify(
                "appimage-thumbnailer",
                Some("none"),
                Some("<runner@runnervm3jd5f>")
            )
            .0,
            OriginKind::Unknown
        );
        assert_eq!(classify("kmod-nvidia", None, None).0, OriginKind::Unknown);
        assert_eq!(
            classify("gpg-pubkey", None, Some("Fedora (45)")).0,
            OriginKind::Key
        );
    }
}
