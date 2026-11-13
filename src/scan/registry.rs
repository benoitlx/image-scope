//! Client minimal pour un registre OCI (ghcr.io, quay.io, docker.io…) :
//! référence d'image, jeton anonyme ou identifiants de `podman login`, manifestes, blobs.

use serde::Deserialize;
use std::io::Read;
use std::sync::Mutex;

/// `registre/dépôt:tag` ou `registre/dépôt@sha256:…`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageRef {
    pub registry: String,
    pub repository: String,
    pub reference: String,
}

impl ImageRef {
    /// Accepte `ghcr.io/owner/image[:tag|@digest]`, avec un éventuel préfixe
    /// `docker://` ou `https://`, ainsi que les noms Docker Hub courts (`fedora:42`).
    pub fn parse(input: &str) -> Result<Self, String> {
        let s = input.trim();
        let s = ["docker://", "oci://", "https://", "http://"]
            .iter()
            .find_map(|p| s.strip_prefix(p))
            .unwrap_or(s)
            .trim_end_matches('/');
        if s.is_empty() || s.contains(char::is_whitespace) {
            return Err(format!("invalid image reference: {input:?}"));
        }
        let (name, reference) = if let Some((name, digest)) = s.split_once('@') {
            (name, digest.to_owned())
        } else {
            match s.rsplit_once(':') {
                // Un « : » après le dernier « / » est un tag, avant c'est un port.
                Some((name, tag)) if !tag.contains('/') => (name, tag.to_owned()),
                _ => (s, "latest".to_owned()),
            }
        };
        let (first, rest) = name.split_once('/').unwrap_or(("", name));
        let (registry, repository) =
            if first.contains('.') || first.contains(':') || first == "localhost" {
                (first.to_owned(), rest.to_owned())
            } else if name.contains('/') {
                ("docker.io".to_owned(), name.to_owned())
            } else {
                ("docker.io".to_owned(), format!("library/{name}"))
            };
        if repository.is_empty() || reference.is_empty() {
            return Err(format!("invalid image reference: {input:?}"));
        }
        Ok(ImageRef {
            registry,
            repository,
            reference,
        })
    }

    /// Ressemble à une image plutôt qu'à un fichier local ?
    #[cfg_attr(not(feature = "viewer"), allow(dead_code))]
    pub fn looks_like_image(input: &str) -> bool {
        let lower = input.to_lowercase();
        !lower.ends_with(".json")
            && !lower.ends_with(".sqlite")
            && input.contains('/')
            && Self::parse(input)
                .is_ok_and(|r| r.registry != "docker.io" || !input.starts_with('.'))
    }

    fn host(&self) -> &str {
        if self.registry == "docker.io" {
            "registry-1.docker.io"
        } else {
            &self.registry
        }
    }
}

impl std::fmt::Display for ImageRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let sep = if self.reference.contains(':') {
            '@'
        } else {
            ':'
        };
        write!(
            f,
            "{}/{}{sep}{}",
            self.registry, self.repository, self.reference
        )
    }
}

const MANIFEST_TYPES: &str = "application/vnd.oci.image.index.v1+json, \
    application/vnd.oci.image.manifest.v1+json, \
    application/vnd.docker.distribution.manifest.list.v2+json, \
    application/vnd.docker.distribution.manifest.v2+json";

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Descriptor {
    #[serde(default)]
    pub media_type: String,
    pub digest: String,
    #[serde(default)]
    pub size: u64,
    pub platform: Option<Platform>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Platform {
    pub architecture: String,
    pub os: String,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    #[serde(default)]
    media_type: String,
    #[serde(default)]
    manifests: Vec<Descriptor>,
    config: Option<Descriptor>,
    #[serde(default)]
    layers: Vec<Descriptor>,
}

#[derive(Deserialize, Debug, Default)]
pub struct ImageConfig {
    #[serde(default)]
    pub rootfs: RootFs,
    #[serde(default)]
    pub config: ContainerConfig,
}

#[derive(Deserialize, Debug, Default)]
pub struct RootFs {
    #[serde(default)]
    pub diff_ids: Vec<String>,
}

#[derive(Deserialize, Debug, Default)]
#[serde(rename_all = "PascalCase")]
pub struct ContainerConfig {
    #[serde(default)]
    pub labels: Option<std::collections::HashMap<String, String>>,
}

/// Image résolue pour une plateforme : ses couches (de la base vers le haut).
pub struct Image {
    pub digest: String,
    pub layers: Vec<Descriptor>,
    pub config: ImageConfig,
}

pub struct Registry {
    agent: ureq::Agent,
    image: ImageRef,
    token: Mutex<Option<String>>,
}

impl Registry {
    pub fn new(image: ImageRef) -> Self {
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .user_agent("image-scope")
            .build()
            .into();
        Registry {
            agent,
            image,
            token: Mutex::new(None),
        }
    }

    fn url(&self, path: &str) -> String {
        format!(
            "https://{}/v2/{}/{path}",
            self.image.host(),
            self.image.repository
        )
    }

    /// GET avec le jeton courant ; sur 401, obtient un jeton (`WWW-Authenticate`) et réessaie.
    fn get(&self, path: &str, accept: &str) -> Result<ureq::http::Response<ureq::Body>, String> {
        let url = self.url(path);
        for attempt in 0..2 {
            let mut request = self.agent.get(&url).header("Accept", accept);
            if let Some(token) = self.token.lock().unwrap().as_deref() {
                request = request.header("Authorization", format!("Bearer {token}"));
            }
            let response = request.call().map_err(|e| format!("{url}: {e}"))?;
            match response.status().as_u16() {
                200 => return Ok(response),
                401 if attempt == 0 => {
                    let challenge = response
                        .headers()
                        .get("www-authenticate")
                        .and_then(|v| v.to_str().ok())
                        .ok_or_else(|| format!("{url}: 401 without authentication challenge"))?
                        .to_owned();
                    let token = self.fetch_token(&challenge)?;
                    *self.token.lock().unwrap() = Some(token);
                }
                401 | 403 => {
                    return Err(format!(
                        "{}: access denied. For a private image, run `podman login {}` first.",
                        self.image, self.image.registry
                    ));
                }
                404 => return Err(format!("{}: not found ({url})", self.image)),
                status => return Err(format!("{url}: HTTP {status}")),
            }
        }
        unreachable!()
    }

    /// Jeton Bearer selon le défi `realm="…",service="…",scope="…"`, avec les
    /// identifiants de `podman login` / `docker login` s'il y en a.
    fn fetch_token(&self, challenge: &str) -> Result<String, String> {
        let params = parse_challenge(challenge);
        let realm = params
            .iter()
            .find(|(k, _)| k == "realm")
            .map(|(_, v)| v.clone())
            .ok_or_else(|| format!("unsupported authentication: {challenge}"))?;
        let mut request = self.agent.get(&realm);
        for (k, v) in &params {
            if k == "service" || k == "scope" {
                request = request.query(k, v);
            }
        }
        if !params.iter().any(|(k, _)| k == "scope") {
            request = request.query(
                "scope",
                format!("repository:{}:pull", self.image.repository),
            );
        }
        if let Some(auth) = stored_credentials(&self.image.registry) {
            request = request.header("Authorization", format!("Basic {auth}"));
        }
        #[derive(Deserialize)]
        struct Token {
            token: Option<String>,
            access_token: Option<String>,
        }
        let mut response = request.call().map_err(|e| format!("{realm}: {e}"))?;
        if response.status() != 200 {
            return Err(format!(
                "{}: access refused (HTTP {}): the image does not exist, or it is private (run `podman login {}`).",
                self.image,
                response.status(),
                self.image.registry
            ));
        }
        let token: Token = response
            .body_mut()
            .read_json()
            .map_err(|e| format!("{realm}: {e}"))?;
        token
            .token
            .or(token.access_token)
            .ok_or_else(|| format!("{realm}: no token in response"))
    }

    fn json<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        accept: &str,
    ) -> Result<(T, Option<String>), String> {
        let mut response = self.get(path, accept)?;
        let digest = response
            .headers()
            .get("docker-content-digest")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let value = response
            .body_mut()
            .with_config()
            .limit(64 * 1024 * 1024)
            .read_json()
            .map_err(|e| format!("{path}: {e}"))?;
        Ok((value, digest))
    }

    /// Manifeste de l'image (en choisissant la plateforme dans un index), puis sa config.
    pub fn resolve(&self, os: &str, architecture: &str) -> Result<Image, String> {
        let (mut manifest, mut digest): (Manifest, _) = self.json(
            &format!("manifests/{}", self.image.reference),
            MANIFEST_TYPES,
        )?;
        if !manifest.manifests.is_empty()
            || manifest.media_type.contains("index")
            || manifest.media_type.contains("list")
        {
            let available: Vec<String> = manifest
                .manifests
                .iter()
                .filter_map(|m| m.platform.as_ref())
                .map(|p| format!("{}/{}", p.os, p.architecture))
                .collect();
            let chosen = manifest
                .manifests
                .iter()
                .find(|m| {
                    m.platform
                        .as_ref()
                        .is_some_and(|p| p.os == os && p.architecture == architecture)
                })
                .ok_or_else(|| {
                    format!(
                        "{}: no {os}/{architecture} image (available: {})",
                        self.image,
                        available.join(", ")
                    )
                })?
                .digest
                .clone();
            (manifest, _) = self.json(&format!("manifests/{chosen}"), MANIFEST_TYPES)?;
            digest = Some(chosen);
        }
        let config_descriptor = manifest
            .config
            .ok_or_else(|| format!("{}: manifest without config", self.image))?;
        let (config, _): (ImageConfig, _) = self.json(
            &format!("blobs/{}", config_descriptor.digest),
            "application/json, */*",
        )?;
        Ok(Image {
            digest: digest.unwrap_or_default(),
            layers: manifest.layers,
            config,
        })
    }

    /// Contenu d'une couche, décompressé à la volée (tar).
    pub fn layer(&self, layer: &Descriptor) -> Result<Box<dyn Read + Send>, String> {
        let response = self.get(&format!("blobs/{}", layer.digest), "*/*")?;
        let body = response
            .into_body()
            .into_with_config()
            .limit(u64::MAX)
            .reader();
        let media = layer.media_type.as_str();
        Ok(if media.ends_with("gzip") {
            Box::new(flate2::read::MultiGzDecoder::new(body))
        } else if media.ends_with("zstd") {
            Box::new(zstd::stream::read::Decoder::new(body).map_err(|e| e.to_string())?)
        } else if media.ends_with("tar") || media.is_empty() {
            Box::new(body)
        } else {
            return Err(format!("unsupported layer type {media}"));
        })
    }
}

/// `Bearer realm="https://ghcr.io/token",service="ghcr.io",scope="…"` → paires clé/valeur.
fn parse_challenge(challenge: &str) -> Vec<(String, String)> {
    let rest = challenge
        .split_once(' ')
        .map(|(_, r)| r)
        .unwrap_or(challenge);
    let mut out = Vec::new();
    let mut chars = rest.chars().peekable();
    loop {
        let key: String = chars
            .by_ref()
            .skip_while(|c| *c == ',' || c.is_whitespace())
            .take_while(|c| *c != '=')
            .collect();
        if key.is_empty() {
            break;
        }
        let value: String = if chars.peek() == Some(&'"') {
            chars.next();
            chars.by_ref().take_while(|c| *c != '"').collect()
        } else {
            chars.by_ref().take_while(|c| *c != ',').collect()
        };
        out.push((key.trim().to_owned(), value));
    }
    out
}

/// Identifiants (base64 « user:mot de passe ») enregistrés par `podman login` ou `docker login`.
fn stored_credentials(registry: &str) -> Option<String> {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let mut files = Vec::new();
    if let Some(f) = std::env::var_os("REGISTRY_AUTH_FILE") {
        files.push(f.into());
    }
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") {
        files.push(std::path::PathBuf::from(dir).join("containers/auth.json"));
    }
    if let Some(home) = &home {
        files.push(home.join(".config/containers/auth.json"));
        files.push(home.join(".docker/config.json"));
    }
    let keys: Vec<&str> = if registry == "docker.io" {
        vec![
            "docker.io",
            "https://index.docker.io/v1/",
            "index.docker.io",
        ]
    } else {
        vec![registry]
    };
    #[derive(Deserialize)]
    struct AuthFile {
        #[serde(default)]
        auths: std::collections::HashMap<String, Entry>,
    }
    #[derive(Deserialize)]
    struct Entry {
        auth: Option<String>,
    }
    files.iter().find_map(|f| {
        let file: AuthFile = serde_json::from_str(&std::fs::read_to_string(f).ok()?).ok()?;
        keys.iter()
            .find_map(|k| file.auths.get(*k).and_then(|e| e.auth.clone()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> (String, String, String) {
        let r = ImageRef::parse(s).unwrap();
        (r.registry, r.repository, r.reference)
    }

    #[test]
    fn parses_references() {
        let ghcr = |r: &str| ("ghcr.io".into(), "benoitlx/niri-nvidia".into(), r.into());
        assert_eq!(parse("ghcr.io/benoitlx/niri-nvidia"), ghcr("latest"));
        assert_eq!(
            parse("ghcr.io/benoitlx/niri-nvidia:20261112"),
            ghcr("20261112")
        );
        assert_eq!(
            parse("docker://ghcr.io/benoitlx/niri-nvidia:latest"),
            ghcr("latest")
        );
        assert_eq!(
            parse("https://ghcr.io/benoitlx/niri-nvidia/"),
            ghcr("latest")
        );
        assert_eq!(
            parse("ghcr.io/benoitlx/niri-nvidia@sha256:abc"),
            ghcr("sha256:abc")
        );
        assert_eq!(
            parse("localhost:5000/img"),
            ("localhost:5000".into(), "img".into(), "latest".into())
        );
        assert_eq!(
            parse("fedora:42"),
            ("docker.io".into(), "library/fedora".into(), "42".into())
        );
        assert!(ImageRef::parse("").is_err());
        assert!(ImageRef::looks_like_image("ghcr.io/benoitlx/niri"));
        assert!(!ImageRef::looks_like_image("data/packages-map.json"));
        assert!(!ImageRef::looks_like_image("packages-map.json"));
    }

    #[test]
    fn parses_bearer_challenge() {
        assert_eq!(
            parse_challenge(
                r#"Bearer realm="https://ghcr.io/token",service="ghcr.io",scope="repository:a/b:pull""#
            ),
            [
                ("realm".to_owned(), "https://ghcr.io/token".to_owned()),
                ("service".to_owned(), "ghcr.io".to_owned()),
                ("scope".to_owned(), "repository:a/b:pull".to_owned()),
            ]
        );
    }
}
