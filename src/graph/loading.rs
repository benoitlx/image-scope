//! Chargement des données produites par `image-scope inspect`.
//!
//! - natif : `packages-map.json` et `security-advisories.json` du dossier courant,
//!   ou les fichiers / images passés en argument ou saisis dans la fenêtre « Data »
//!   (une image comme `ghcr.io/owner/image` est analysée en arrière-plan) ;
//! - web : `?data=<url>&advisories=<url>`, sinon l'index `builds.json` du site publié
//!   par la CI (`?image=…&build=…` choisit le build), sinon les deux fichiers à côté
//!   de la page, ou une URL saisie dans la fenêtre « Data » ;
//! - partout : glisser-déposer un ou plusieurs fichiers JSON sur la fenêtre.
//!
//! Le type d'un fichier est reconnu à son contenu (tableau = paquets, objet = avis
//! ou index des builds).
//! Les lectures sont asynchrones sur le web : elles déposent leur résultat dans une
//! boîte aux lettres, vidée à chaque frame par `drain_inbox`.

use super::graph::Node;
use super::security::{ADVISORIES_FILE, AdvisoryFile};
use crate::model::{BuildEntry, BuildIndex};
use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};
use std::sync::Mutex;

pub const PACKAGES_FILE: &str = "packages-map.json";

/// Paquets de l'image affichée. Le graphe est reconstruit quand elle change.
#[derive(Resource)]
pub(super) struct PackagesData {
    pub nodes: Vec<Node>,
    pub source: String,
}

/// Avis de sécurité. Seul l'index de sécurité est recalculé quand elle change.
#[derive(Resource)]
pub(super) struct AdvisoriesData {
    pub file: AdvisoryFile,
    pub source: String,
}

/// Index des builds publiés (`builds.json`) et build affiché.
#[derive(Resource)]
struct Builds {
    /// Préfixe des chemins de l'index (dossier de `builds.json`).
    base: String,
    index: BuildIndex,
    /// Image choisie dans la liste.
    image: String,
    selected: Option<usize>,
}

impl Builds {
    fn images(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self
            .index
            .builds
            .iter()
            .map(|b| b.info.name.as_str())
            .collect();
        names.dedup();
        names
    }

    /// Charge les fichiers d'un build (les avis seulement s'il y en a).
    fn load(&mut self, i: usize, commands: &mut Commands) {
        let build = &self.index.builds[i];
        let dir = format!("{}{}", self.base, build.path);
        platform::fetch_build_file(&format!("{dir}/{PACKAGES_FILE}"), false);
        if build.has_advisories {
            platform::fetch_build_file(&format!("{dir}/{ADVISORIES_FILE}"), true);
        } else {
            commands.remove_resource::<AdvisoriesData>();
        }
        platform::remember_build(&build.info.name, &build.id);
        self.image = build.info.name.clone();
        self.selected = Some(i);
    }
}

#[derive(Resource, Default)]
struct LoadStatus {
    /// Lectures en cours.
    pending: usize,
    errors: Vec<String>,
    /// Dernier message de progression (analyse d'une image).
    progress: Option<String>,
    /// Champ « URL / image » de la fenêtre Data.
    url: String,
}

enum Incoming {
    Started,
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    Progress(String),
    Loaded {
        source: String,
        content: String,
        /// Chargé depuis une URL : on la garde dans la barre d'adresse.
        from_url: bool,
    },
    Failed {
        source: String,
        error: String,
        /// Fichier par défaut absent : pas une erreur à afficher.
        optional: bool,
    },
}

static INBOX: Mutex<Vec<Incoming>> = Mutex::new(Vec::new());

fn push(message: Incoming) {
    INBOX
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(message);
}

enum Parsed {
    Packages(Vec<Node>),
    Advisories(AdvisoryFile),
    Builds(BuildIndex),
}

fn parse(content: &str) -> Result<Parsed, String> {
    match content.trim_start().as_bytes().first() {
        Some(b'[') => {
            let nodes: Vec<Node> =
                serde_json::from_str(content).map_err(|e| format!("invalid packages map: {e}"))?;
            if nodes.is_empty() {
                return Err("the packages map is empty".into());
            }
            Ok(Parsed::Packages(nodes))
        }
        Some(b'{') => match serde_json::from_str::<BuildIndex>(content) {
            Ok(index) if index.builds.is_empty() => Err("the builds index is empty".into()),
            Ok(index) => Ok(Parsed::Builds(index)),
            Err(_) => serde_json::from_str(content)
                .map(Parsed::Advisories)
                .map_err(|e| format!("invalid advisories file: {e}")),
        },
        _ => Err("not a JSON file produced by image-scope".into()),
    }
}

pub struct LoadingPlugin;

impl Plugin for LoadingPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LoadStatus>()
            .add_systems(Startup, start_loading)
            .add_systems(PreUpdate, drain_inbox)
            .add_systems(EguiPrimaryContextPass, data_window);
        #[cfg(not(target_arch = "wasm32"))]
        app.add_systems(PreUpdate, platform::dropped_files.before(drain_inbox));
    }
}

fn start_loading() {
    platform::initial_requests();
    platform::listen_for_drops();
}

fn drain_inbox(
    mut commands: Commands,
    mut status: ResMut<LoadStatus>,
    packages: Option<Res<PackagesData>>,
) {
    let messages = std::mem::take(&mut *INBOX.lock().unwrap_or_else(|e| e.into_inner()));
    let had_messages = !messages.is_empty();
    for message in messages {
        match message {
            Incoming::Started => status.pending += 1,
            Incoming::Progress(message) => status.progress = Some(message),
            Incoming::Loaded {
                source,
                content,
                from_url,
            } => {
                status.pending = status.pending.saturating_sub(1);
                match parse(&content) {
                    Ok(Parsed::Packages(nodes)) => {
                        info!("Loaded {} packages from {source}", nodes.len());
                        if from_url {
                            platform::remember_url("data", &source);
                        }
                        commands.insert_resource(PackagesData { nodes, source });
                    }
                    Ok(Parsed::Advisories(file)) => {
                        info!("Loaded {} from {source}", file.summary());
                        if from_url {
                            platform::remember_url("advisories", &source);
                        }
                        commands.insert_resource(AdvisoriesData { file, source });
                    }
                    Ok(Parsed::Builds(index)) => {
                        info!("Loaded {} builds from {source}", index.builds.len());
                        let base = source.rfind('/').map_or("", |i| &source[..=i]).to_owned();
                        let mut builds = Builds {
                            base,
                            image: index.builds[0].info.name.clone(),
                            index,
                            selected: None,
                        };
                        // Rien d'affiché : build demandé dans l'URL, sinon le plus récent.
                        if packages.is_none() {
                            let requested = platform::requested_build().and_then(|(image, id)| {
                                builds.index.builds.iter().position(|b| {
                                    b.info.name == image && id.as_ref().is_none_or(|id| *id == b.id)
                                })
                            });
                            builds.load(requested.unwrap_or(0), &mut commands);
                        }
                        commands.insert_resource(builds);
                    }
                    Err(e) => {
                        warn!("{source}: {e}");
                        status.errors.push(format!("{source}: {e}"));
                    }
                }
            }
            Incoming::Failed {
                source,
                error,
                optional,
            } => {
                status.pending = status.pending.saturating_sub(1);
                if optional {
                    info!("{source} not loaded: {error}");
                } else {
                    warn!("{source}: {error}");
                    status.errors.push(format!("{source}: {error}"));
                }
            }
        }
    }
    if had_messages && status.pending == 0 {
        status.progress = None;
    }
}

fn loading_ui(ui: &mut egui::Ui, status: &LoadStatus) {
    ui.horizontal(|ui| {
        ui.spinner();
        ui.label(status.progress.as_deref().unwrap_or("Loading…"));
    });
}

/// Liste des builds publiés : choix de l'image, puis du build.
fn builds_ui(ui: &mut egui::Ui, builds: &mut Builds, commands: &mut Commands) {
    let mut chosen = None;
    let images: Vec<String> = builds.images().into_iter().map(str::to_owned).collect();
    ui.horizontal(|ui| {
        ui.label("Image");
        egui::ComboBox::from_id_salt("build_image")
            .selected_text(&builds.image)
            .show_ui(ui, |ui| {
                for name in &images {
                    if ui.selectable_label(builds.image == *name, name).clicked()
                        && builds.image != *name
                    {
                        builds.image = name.clone();
                        // Dernier build de cette image.
                        chosen = builds
                            .index
                            .builds
                            .iter()
                            .position(|b| b.info.name == *name);
                    }
                }
            });
    });
    egui::ScrollArea::vertical()
        .id_salt("builds_list")
        .max_height(180.0)
        .show(ui, |ui| {
            for (i, build) in builds.index.builds.iter().enumerate() {
                if build.info.name != builds.image {
                    continue;
                }
                let fixes = match build.info.pending_fixes {
                    Some(0) => String::new(),
                    Some(n) => format!(" · {n} fixes to pick up"),
                    None => String::new(),
                };
                let text = format!(
                    "{} · {} · {} pkgs{fixes}",
                    build.id,
                    build.info.version.as_deref().unwrap_or("?"),
                    build.info.packages
                );
                if ui
                    .selectable_label(builds.selected == Some(i), text)
                    .on_hover_text(build_tooltip(build))
                    .clicked()
                {
                    chosen = Some(i);
                }
            }
        });
    if let Some(i) = chosen.filter(|&i| builds.selected != Some(i)) {
        builds.load(i, commands);
    }
}

fn build_tooltip(build: &BuildEntry) -> String {
    format!(
        "{}
{}
created {}
inspected {}",
        build.info.image,
        build.info.digest,
        build.info.created.as_deref().unwrap_or("?"),
        build.info.inspected_at
    )
}

fn data_window(
    mut contexts: EguiContexts,
    mut commands: Commands,
    mut status: ResMut<LoadStatus>,
    builds: Option<ResMut<Builds>>,
    packages: Option<Res<PackagesData>>,
    advisories: Option<Res<AdvisoriesData>>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    let status = status.as_mut();
    let mut builds = builds;

    let Some(packages) = packages else {
        // Écran d'accueil tant qu'aucune image n'est chargée.
        egui::Window::new("image-scope")
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.set_width(420.0);
                if status.pending > 0 {
                    loading_ui(ui, status);
                } else {
                    ui.label("No package data loaded.");
                }
                ui.add_space(4.0);
                if let Some(builds) = builds.as_deref_mut() {
                    builds_ui(ui, builds, &mut commands);
                    ui.separator();
                }
                ui.label(platform::LOAD_HELP);
                ui.label(format!(
                    "Or drop {PACKAGES_FILE} (and optionally {ADVISORIES_FILE}) onto this window."
                ));
                sources_ui(ui, status);
            });
        return Ok(());
    };

    egui::Window::new("Data")
        .default_open(builds.is_some())
        .default_width(380.0)
        .show(ctx, |ui| {
            if let Some(builds) = builds.as_deref_mut() {
                builds_ui(ui, builds, &mut commands);
                ui.separator();
            }
            egui::Grid::new("data_sources")
                .num_columns(2)
                .show(ui, |ui| {
                    ui.label("Packages");
                    ui.label(format!(
                        "{} ({} packages)",
                        packages.source,
                        packages.nodes.len()
                    ));
                    ui.end_row();
                    ui.label("Advisories");
                    match &advisories {
                        Some(a) => ui.label(format!("{}\n{}", a.source, a.file.summary())),
                        None => ui.weak("none"),
                    };
                    ui.end_row();
                });
            ui.separator();
            ui.weak("Drop JSON files onto the window to load another image or other advisories.");
            if status.pending > 0 {
                loading_ui(ui, status);
            }
            sources_ui(ui, status);
        });
    Ok(())
}

/// Champ URL / image et erreurs de chargement.
fn sources_ui(ui: &mut egui::Ui, status: &mut LoadStatus) {
    if platform::CAN_FETCH {
        ui.horizontal(|ui| {
            let edit = ui.add(
                egui::TextEdit::singleline(&mut status.url)
                    .hint_text(platform::URL_HINT)
                    .desired_width(300.0),
            );
            let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let url = status.url.trim();
            if (ui.button("Load").clicked() || enter) && !url.is_empty() {
                platform::fetch(url, false);
            }
        });
        ui.weak(platform::URL_NOTE);
    }
    if !status.errors.is_empty() {
        ui.separator();
        for error in &status.errors {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        if ui.small_button("Dismiss").clicked() {
            status.errors.clear();
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod platform {
    use super::{ADVISORIES_FILE, Incoming, PACKAGES_FILE, push};
    use crate::scan::{advisories, inspect, registry::ImageRef};
    use bevy::prelude::*;
    use bevy::window::FileDragAndDrop;
    use std::path::Path;

    pub const CAN_FETCH: bool = true;
    pub const URL_HINT: &str = "ghcr.io/owner/image:tag";
    pub const URL_NOTE: &str =
        "An image reference (analyzed here, cached per layer), a JSON URL or a local file.";
    pub const LOAD_HELP: &str = "Enter an image below, e.g. ghcr.io/benoitlx/niri-nvidia.";

    /// Fichiers ou images passés en argument, sinon les fichiers du dossier courant.
    pub fn initial_requests() {
        let args: Vec<String> = std::env::args().skip(1).collect();
        if args.is_empty() {
            read(Path::new(PACKAGES_FILE), true);
            read(Path::new(ADVISORIES_FILE), true);
        } else {
            for arg in args {
                fetch(&arg, false);
            }
        }
    }

    /// Fichier local, URL d'un JSON, ou image à analyser.
    pub fn fetch(input: &str, optional: bool) {
        let input = input.trim().to_owned();
        if Path::new(&input).exists() {
            read(Path::new(&input), optional);
        } else if input.starts_with("http") && input.to_lowercase().ends_with(".json") {
            download(input);
        } else if ImageRef::looks_like_image(&input) {
            inspect_image(input);
        } else {
            read(Path::new(&input), optional);
        }
    }

    fn download(url: String) {
        push(Incoming::Started);
        std::thread::spawn(move || {
            let result = ureq::get(&url)
                .call()
                .map_err(|e| e.to_string())
                .and_then(|mut r| {
                    r.body_mut()
                        .with_config()
                        .limit(512 * 1024 * 1024)
                        .read_to_string()
                        .map_err(|e| e.to_string())
                });
            push(match result {
                Ok(content) => Incoming::Loaded {
                    source: url,
                    content,
                    from_url: true,
                },
                Err(error) => Incoming::Failed {
                    source: url,
                    error,
                    optional: false,
                },
            });
        });
    }

    /// Analyse l'image dans un thread (téléchargement des couches), puis récupère
    /// les avis de sécurité de sa release Fedora.
    fn inspect_image(image: String) {
        push(Incoming::Started);
        std::thread::spawn(move || {
            let progress = |message: String| {
                info!("{message}");
                push(Incoming::Progress(message));
            };
            let options = inspect::Options {
                all_layers: false,
                platform: None,
            };
            let result = inspect::inspect(&image, &options, &progress).and_then(|r| {
                serde_json::to_string(&r.nodes)
                    .map(|json| (r, json))
                    .map_err(|e| e.to_string())
            });
            let release = match result {
                Ok((inspection, content)) => {
                    let release = inspection.release.clone();
                    push(Incoming::Loaded {
                        source: inspection.image,
                        content,
                        from_url: false,
                    });
                    release
                }
                Err(error) => {
                    push(Incoming::Failed {
                        source: image,
                        error,
                        optional: false,
                    });
                    return;
                }
            };
            let Some(release) = release else { return };
            push(Incoming::Started);
            push(
                match advisories::fetch(&release, false, &progress)
                    .and_then(|f| serde_json::to_string(&f).map_err(|e| e.to_string()))
                {
                    Ok(content) => Incoming::Loaded {
                        source: format!("Bodhi {release}"),
                        content,
                        from_url: false,
                    },
                    Err(error) => Incoming::Failed {
                        source: format!("Bodhi {release}"),
                        error,
                        optional: false,
                    },
                },
            );
        });
    }

    fn read(path: &Path, optional: bool) {
        let source = path.display().to_string();
        push(Incoming::Started);
        push(match std::fs::read_to_string(path) {
            Ok(content) => Incoming::Loaded {
                source,
                content,
                from_url: false,
            },
            Err(e) => Incoming::Failed {
                source,
                error: e.to_string(),
                optional,
            },
        });
    }

    /// Le glisser-déposer natif passe par les événements de Bevy (`dropped_files`).
    pub fn listen_for_drops() {}

    pub fn dropped_files(mut events: EventReader<FileDragAndDrop>) {
        for event in events.read() {
            if let FileDragAndDrop::DroppedFile { path_buf, .. } = event {
                read(path_buf, false);
            }
        }
    }

    pub fn remember_url(_key: &str, _url: &str) {}

    pub fn fetch_build_file(path: &str, optional: bool) {
        read(Path::new(path), optional);
    }

    pub fn remember_build(_image: &str, _id: &str) {}

    pub fn requested_build() -> Option<(String, Option<String>)> {
        None
    }
}

#[cfg(target_arch = "wasm32")]
mod platform {
    use super::{ADVISORIES_FILE, Incoming, PACKAGES_FILE, push};
    use wasm_bindgen::JsCast;

    const INDEX_FILE: &str = "builds.json";
    use wasm_bindgen::prelude::*;
    use wasm_bindgen_futures::{JsFuture, spawn_local};

    pub const CAN_FETCH: bool = true;
    pub const URL_HINT: &str = "https://…/packages-map.json";
    pub const URL_NOTE: &str = "The server must allow cross-origin requests (GitHub Pages and raw.githubusercontent.com do). Images are analyzed with `image-scope inspect <image>`.";
    pub const LOAD_HELP: &str = "Generate the files with `image-scope inspect ghcr.io/owner/image`, then load them by URL below.";

    fn params() -> Option<web_sys::UrlSearchParams> {
        web_sys::window()
            .and_then(|w| w.location().search().ok())
            .and_then(|s| web_sys::UrlSearchParams::new_with_str(&s).ok())
    }

    /// `?image=niri-nvidia&build=20261112-2228`.
    pub fn requested_build() -> Option<(String, Option<String>)> {
        let params = params()?;
        Some((params.get("image")?, params.get("build")))
    }

    /// `?data=<url>&advisories=<url>`, sinon l'index des builds, sinon les fichiers
    /// à côté de la page.
    pub fn initial_requests() {
        let params = params();
        let get = |key: &str| params.as_ref().and_then(|p| p.get(key));
        match get("data") {
            Some(url) => {
                fetch(&url, false);
                if let Some(url) = get("advisories") {
                    fetch(&url, false);
                }
            }
            None => {
                push(Incoming::Started);
                spawn_local(async move {
                    match fetch_text(INDEX_FILE).await {
                        Ok(content) => push(Incoming::Loaded {
                            source: INDEX_FILE.into(),
                            content,
                            from_url: false,
                        }),
                        Err(error) => {
                            push(Incoming::Failed {
                                source: INDEX_FILE.into(),
                                error,
                                optional: true,
                            });
                            fetch(PACKAGES_FILE, true);
                            fetch(ADVISORIES_FILE, true);
                        }
                    }
                });
            }
        }
    }

    pub fn fetch(url: &str, optional: bool) {
        // Les fichiers par défaut ne vont pas dans la barre d'adresse.
        request(url, optional, !optional);
    }

    /// Fichier d'un build de l'index : le build est noté dans l'URL à la place.
    pub fn fetch_build_file(url: &str, optional: bool) {
        request(url, optional, false);
    }

    /// `?image=…&build=…`, sans `data` ni `advisories` qui primeraient au rechargement.
    pub fn remember_build(image: &str, id: &str) {
        let Some(window) = web_sys::window() else {
            return;
        };
        let Ok(url) = window
            .location()
            .href()
            .and_then(|href| web_sys::Url::new(&href))
        else {
            return;
        };
        let params = url.search_params();
        params.delete("data");
        params.delete("advisories");
        params.set("image", image);
        params.set("build", id);
        if let Ok(history) = window.history() {
            let _ = history.replace_state_with_url(&JsValue::NULL, "", Some(&url.href()));
        }
    }

    fn request(url: &str, optional: bool, remember: bool) {
        push(Incoming::Started);
        let url = url.to_owned();
        spawn_local(async move {
            push(match fetch_text(&url).await {
                Ok(content) => Incoming::Loaded {
                    from_url: remember,
                    source: url,
                    content,
                },
                Err(error) => Incoming::Failed {
                    source: url,
                    error,
                    optional,
                },
            });
        });
    }

    async fn fetch_text(url: &str) -> Result<String, String> {
        let window = web_sys::window().ok_or("no window")?;
        let response: web_sys::Response = JsFuture::from(window.fetch_with_str(url))
            .await
            .map_err(|e| {
                format!(
                    "{} (network error or cross-origin request refused)",
                    js_error(e)
                )
            })?
            .dyn_into()
            .map_err(js_error)?;
        if !response.ok() {
            return Err(format!(
                "HTTP {} {}",
                response.status(),
                response.status_text()
            ));
        }
        JsFuture::from(response.text().map_err(js_error)?)
            .await
            .map_err(js_error)?
            .as_string()
            .ok_or_else(|| "response is not text".into())
    }

    fn js_error(value: JsValue) -> String {
        if let Some(e) = value.dyn_ref::<js_sys::Error>() {
            return String::from(e.message());
        }
        value.as_string().unwrap_or_else(|| format!("{value:?}"))
    }

    /// Glisser-déposer : le navigateur ne donne pas de chemin, on lit le contenu
    /// des fichiers déposés n'importe où sur la page.
    pub fn listen_for_drops() {
        let Some(document) = web_sys::window().and_then(|w| w.document()) else {
            return;
        };
        // Sans `preventDefault` sur `dragover`, le navigateur ouvrirait le fichier.
        let over = Closure::<dyn FnMut(web_sys::DragEvent)>::new(|e: web_sys::DragEvent| {
            e.prevent_default();
        });
        let drop = Closure::<dyn FnMut(web_sys::DragEvent)>::new(|e: web_sys::DragEvent| {
            e.prevent_default();
            let Some(files) = e.data_transfer().and_then(|d| d.files()) else {
                return;
            };
            for i in 0..files.length() {
                let Some(file) = files.get(i) else {
                    continue;
                };
                push(Incoming::Started);
                spawn_local(async move {
                    let source = file.name();
                    push(match JsFuture::from(file.text()).await {
                        Ok(text) => match text.as_string() {
                            Some(content) => Incoming::Loaded {
                                source,
                                content,
                                from_url: false,
                            },
                            None => Incoming::Failed {
                                source,
                                error: "not a text file".into(),
                                optional: false,
                            },
                        },
                        Err(e) => Incoming::Failed {
                            source,
                            error: js_error(e),
                            optional: false,
                        },
                    });
                });
            }
        });
        for (event, handler) in [("dragover", &over), ("drop", &drop)] {
            let _ =
                document.add_event_listener_with_callback(event, handler.as_ref().unchecked_ref());
        }
        // Les écouteurs vivent aussi longtemps que la page.
        over.forget();
        drop.forget();
    }

    /// Met l'URL chargée dans la barre d'adresse, pour partager le lien.
    pub fn remember_url(key: &str, value: &str) {
        let Some(window) = web_sys::window() else {
            return;
        };
        let Ok(href) = window.location().href() else {
            return;
        };
        let Ok(url) = web_sys::Url::new(&href) else {
            return;
        };
        url.search_params().set(key, value);
        if let Ok(history) = window.history() {
            let _ = history.replace_state_with_url(&JsValue::NULL, "", Some(&url.href()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Parsed, parse};

    #[test]
    fn recognizes_files_by_content() {
        let packages = r#" [{"Name": "a", "introduced_in": "0"}] "#;
        assert!(matches!(parse(packages), Ok(Parsed::Packages(n)) if n.len() == 1));
        let advisories = r#"{"release": "F45", "fetched_at": "now", "advisories": []}"#;
        assert!(matches!(parse(advisories), Ok(Parsed::Advisories(_))));
        assert!(parse("[]").is_err());
        assert!(parse("hello").is_err());
        assert!(parse(r#"{"foo": 1}"#).is_err());
        let index = r#"{"generated_at": "now", "builds": [{"id": "20261112-2228",
            "path": "niri/20261112-2228", "has_advisories": true, "image": "ghcr.io/a/niri",
            "name": "niri", "digest": "sha256:x", "version": null, "created": null,
            "inspected_at": "now", "release": "F45", "packages": 3, "size": 9,
            "pending_fixes": null}]}"#;
        assert!(matches!(parse(index), Ok(Parsed::Builds(i)) if i.builds[0].info.name == "niri"));
    }
}
