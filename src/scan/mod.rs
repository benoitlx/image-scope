//! Analyse d'images (remplace `layers.nu`) : registre OCI, base rpm, avis Bodhi.
//! Natif seulement : un navigateur ne peut pas lire ghcr.io (pas de CORS).

pub mod advisories;
pub mod inspect;
pub mod registry;
pub mod rpmdb;
pub mod site;

use std::path::{Path, PathBuf};

/// `$XDG_CACHE_HOME/image-scope`, sinon `~/.cache/image-scope`.
pub fn cache_dir() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir)
        .join("image-scope")
}

const USAGE: &str = "\
image-scope: explore the rpm packages of an OCI image

Viewer:
  image-scope                          open ./packages-map.json and ./security-advisories.json
  image-scope <IMAGE | FILE>...        inspect an image (and fetch its advisories), or open JSON files
                                       e.g. image-scope ghcr.io/benoitlx/niri-nvidia

Commands (no window):
  inspect <IMAGE>    write packages-map.json and security-advisories.json
      -o, --output <FILE>      packages file (default: packages-map.json)
      --all-layers             scan every layer to know where each package appeared
                               (downloads the whole image; by default only the top
                               layers, down to the last rpm database)
      --platform <OS/ARCH>     default: this machine (linux/amd64…)
      --no-advisories          do not fetch the security advisories
      --site <DIR>             write into DIR/<image>/<build>/ for a published site
                               (with image.json), instead of the current directory
  advisories         write security-advisories.json (Fedora security updates from Bodhi)
      --release <F45>          default: guessed from the packages file
      -i, --input <FILE>       packages file used to guess the release (default: packages-map.json)
      -o, --output <FILE>      default: security-advisories.json
      --refresh                ignore the cache (kept one day)
  local [RPMDB]      packages of a local rpm database (default: this system's)
      -o, --output <FILE>      default: packages-map.json
  index <SITE_DIR>   write SITE_DIR/builds.json, the list of builds read by the web viewer
      --keep <N>               keep only the N latest builds of each image (deletes the others)
      --summary <FILE>         append a markdown report (latest build of each image, changes
                               since the previous one), e.g. $GITHUB_STEP_SUMMARY
      --url <URL>              address of the published viewer, for links in the report
  clear              delete the cache (downloaded rpm databases, advisories)

IMAGE: registry/owner/name[:tag|@digest], e.g. ghcr.io/benoitlx/niri-nvidia:latest.
Private images use the credentials of `podman login` / `docker login`.
Cache: ";

/// Exécute une sous-commande. `None` : pas de sous-commande, lancer la visionneuse.
pub fn run_cli(args: &[String]) -> Option<i32> {
    let command = args.first()?.as_str();
    let rest = &args[1..];
    let result = match command {
        "inspect" => cmd_inspect(rest),
        "advisories" => cmd_advisories(rest),
        "local" => cmd_local(rest),
        "index" => cmd_index(rest),
        "clear" => cmd_clear(),
        "help" | "-h" | "--help" => {
            println!("{USAGE}{}", cache_dir().display());
            Ok(())
        }
        _ => return None,
    };
    Some(match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    })
}

fn print_progress(message: String) {
    eprintln!("{message}");
}

/// Petit analyseur d'options : `--flag`, `--opt valeur`, arguments positionnels.
struct Args {
    positional: Vec<String>,
    options: Vec<(String, Option<String>)>,
}

impl Args {
    fn parse(args: &[String], with_value: &[&str], flags: &[&str]) -> Result<Self, String> {
        let mut positional = Vec::new();
        let mut options = Vec::new();
        let mut it = args.iter();
        while let Some(arg) = it.next() {
            if with_value.contains(&arg.as_str()) {
                let value = it.next().ok_or_else(|| format!("{arg} needs a value"))?;
                options.push((arg.clone(), Some(value.clone())));
            } else if flags.contains(&arg.as_str()) {
                options.push((arg.clone(), None));
            } else if arg.starts_with('-') {
                return Err(format!("unknown option {arg} (see `image-scope help`)"));
            } else {
                positional.push(arg.clone());
            }
        }
        Ok(Args {
            positional,
            options,
        })
    }

    fn value(&self, names: &[&str]) -> Option<&str> {
        self.options
            .iter()
            .rev()
            .find(|(k, _)| names.contains(&k.as_str()))
            .and_then(|(_, v)| v.as_deref())
    }

    fn flag(&self, name: &str) -> bool {
        self.options.iter().any(|(k, _)| k == name)
    }
}

fn write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let json = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| format!("{}: {e}", path.display()))
}

fn cmd_inspect(args: &[String]) -> Result<(), String> {
    let args = Args::parse(
        args,
        &["-o", "--output", "--platform", "--site"],
        &["--all-layers", "--no-advisories"],
    )?;
    let [image] = args.positional.as_slice() else {
        return Err("usage: image-scope inspect <IMAGE> (see `image-scope help`)".into());
    };
    let options = inspect::Options {
        all_layers: args.flag("--all-layers"),
        platform: args.value(&["--platform"]).map(str::to_owned),
    };
    let result = inspect::inspect(image, &options, &print_progress)?;
    let inspected_at = advisories::now_iso8601();

    // `--site DIR` : DIR/<image>/<build>/{packages-map,security-advisories,image}.json
    let site_dir = args.value(&["--site"]).map(|dir| {
        Path::new(dir)
            .join(&result.name)
            .join(site::build_id(result.created.as_deref(), &inspected_at))
    });
    let output = match &site_dir {
        Some(dir) => {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            dir.join("packages-map.json")
        }
        None => PathBuf::from(
            args.value(&["-o", "--output"])
                .unwrap_or("packages-map.json"),
        ),
    };
    write_json(&output, &result.nodes)?;
    let name = match (&result.title, &result.version) {
        (Some(t), Some(v)) => format!("{t} {v}, "),
        (Some(t), None) => format!("{t}, "),
        _ => String::new(),
    };
    eprintln!(
        "Saved {} packages of {name}{} to {}",
        result.nodes.len(),
        result.image,
        output.display()
    );

    let mut pending_fixes = None;
    if !args.flag("--no-advisories") {
        match &result.release {
            Some(release) => match advisories::fetch(release, false, &print_progress) {
                Ok(file) => {
                    let output = output.with_file_name("security-advisories.json");
                    write_json(&output, &file)?;
                    eprintln!("Saved {} to {}", file.summary(), output.display());
                    pending_fixes = Some(crate::model::pending_fixes(&result.nodes, &file));
                }
                // Les paquets sont là : un échec de Bodhi n'est pas fatal.
                Err(e) => eprintln!("warning: no security advisories: {e}"),
            },
            None => eprintln!("Not a Fedora image: no security advisories"),
        }
    }

    if let Some(dir) = site_dir {
        let info = crate::model::BuildInfo {
            image: result.reference,
            name: result.name,
            digest: result.digest,
            version: result.version,
            created: result.created,
            inspected_at,
            release: result.release,
            packages: result.nodes.len(),
            size: result.nodes.iter().filter_map(|n| n.Size).sum(),
            pending_fixes,
        };
        write_json(&dir.join("image.json"), &info)?;
    }
    Ok(())
}

fn cmd_index(args: &[String]) -> Result<(), String> {
    let args = Args::parse(args, &["--keep", "--summary", "--url"], &[])?;
    let [dir] = args.positional.as_slice() else {
        return Err(
            "usage: image-scope index <SITE_DIR> [--keep N] [--summary FILE] [--url URL]".into(),
        );
    };
    let keep = match args.value(&["--keep"]) {
        Some(n) => Some(n.parse::<usize>().map_err(|_| "--keep needs a number")?),
        None => None,
    };
    let index = site::index(Path::new(dir), keep, &print_progress)?;
    if let Some(summary) = args.value(&["--summary"]) {
        let markdown = site::summary(Path::new(dir), &index, args.value(&["--url"]))?;
        // Ajout (et non écrasement) : `$GITHUB_STEP_SUMMARY` peut déjà contenir du texte.
        use std::io::Write;
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(summary)
            .and_then(|mut f| f.write_all(markdown.as_bytes()))
            .map_err(|e| format!("{summary}: {e}"))?;
    }
    Ok(())
}

fn cmd_advisories(args: &[String]) -> Result<(), String> {
    let args = Args::parse(
        args,
        &["--release", "-i", "--input", "-o", "--output"],
        &["--refresh"],
    )?;
    let release = match args.value(&["--release"]) {
        Some(r) => r.to_owned(),
        None => {
            let input = args
                .value(&["-i", "--input"])
                .unwrap_or("packages-map.json");
            let nodes: Vec<crate::model::Node> = serde_json::from_str(
                &std::fs::read_to_string(input).map_err(|e| format!("{input}: {e}"))?,
            )
            .map_err(|e| format!("{input}: {e}"))?;
            inspect::guess_release(&nodes).ok_or_else(|| {
                format!("cannot guess the Fedora release from {input}, use --release")
            })?
        }
    };
    let file = advisories::fetch(&release, args.flag("--refresh"), &print_progress)?;
    let output = Path::new(
        args.value(&["-o", "--output"])
            .unwrap_or("security-advisories.json"),
    );
    write_json(output, &file)?;
    eprintln!("Saved {} to {}", file.summary(), output.display());
    Ok(())
}

fn cmd_local(args: &[String]) -> Result<(), String> {
    let args = Args::parse(args, &["-o", "--output"], &[])?;
    let db = match args.positional.as_slice() {
        [] => rpmdb::RPMDB_PATHS
            .iter()
            .map(|p| Path::new("/").join(p))
            .find(|p| p.exists())
            .ok_or("no rpm database found on this system")?,
        [db] => PathBuf::from(db),
        _ => return Err("usage: image-scope local [RPMDB]".into()),
    };
    let nodes = inspect::inspect_rpmdb(&db)?;
    let output = Path::new(
        args.value(&["-o", "--output"])
            .unwrap_or("packages-map.json"),
    );
    write_json(output, &nodes)?;
    eprintln!(
        "Saved {} packages of {} to {}",
        nodes.len(),
        db.display(),
        output.display()
    );
    Ok(())
}

fn cmd_clear() -> Result<(), String> {
    let dir = cache_dir();
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    eprintln!("Removed {}", dir.display());
    Ok(())
}
