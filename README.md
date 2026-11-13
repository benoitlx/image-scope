# image-scope

Inspect rpm packages inside the various layers of your OCI image

# Dependencies

- Rust toolchain, plus the Bevy system libraries (`systemd-devel`, `alsa-lib-devel` on Fedora) and a C compiler (sqlite and zstd are built in)
- For the web version: the `wasm32-unknown-unknown` target (`rustup target add wasm32-unknown-unknown`) and [`trunk`](https://trunkrs.dev/) (`cargo install trunk`)

No `podman`, `rpm` or `nu` needed: images are read directly from the registry and the rpm database is parsed by image-scope.

# Usage

```sh
# Inspect an image and explore it
cargo run -- ghcr.io/benoitlx/niri-nvidia

# Or write the JSON files only (no window), e.g. in CI
cargo run -- inspect ghcr.io/benoitlx/niri-nvidia:latest
#   → packages-map.json and security-advisories.json, then:
cargo run                                   # opens them from the current directory
cargo run -- a/packages-map.json b/security-advisories.json

cargo run -- help                           # all commands and options
```

How `inspect` works:

- the manifest for this machine's platform is fetched from the registry (anonymous token, or the credentials of `podman login` / `docker login` for a private image);
- every layer that changes the rpm database holds a full copy of it (`usr/lib/sysimage/rpm/rpmdb.sqlite`, `usr/share/rpm/rpmdb.sqlite`). Layers are scanned from the top, so only the top ones are downloaded, down to the one holding the last database (about 2 minutes for `niri-nvidia`), and each download stops as soon as the database is found;
- `--all-layers` scans the whole image (4.3 GiB for `niri-nvidia`) to find in which layer each package appeared;
- the result of each layer (its database, or "no database") is cached by digest in `~/.cache/image-scope`: unchanged layers are never downloaded again, and a second run takes a second. `image-scope clear` empties the cache;
- dependencies are resolved like `rpm -qR` + `rpm -q --whatprovides` (provides and files); the output was checked against `rpm` on all 1705 packages of `niri-nvidia` (same fields, same dependencies);
- the Fedora security advisories of the image's release are fetched from Bodhi (cached for a day). `advisories` fetches them alone, `--no-advisories` skips them.

Other commands: `local [RPMDB]` reads a local rpm database (by default the running system's), `advisories --release F45` fetches advisories for a given release.

## Web version

The viewer also runs in the browser (WebAssembly + WebGL2). A browser cannot read ghcr.io (no cross-origin headers), so images are inspected with `image-scope inspect` and the web page displays the JSON files it produces.

```sh
trunk serve                                   # development, http://127.0.0.1:8080
trunk build --release --cargo-profile web     # optimized build in dist/ (~19 MB wasm, ~6 MB gzipped)
```

On startup, the page loads, in this order:

- `builds.json` next to the page: the index of a published site (see [Continuous integration](#continuous-integration)). The **Data** window then lists the images and their builds, and `?image=niri-nvidia&build=20261112-2228` selects one (the address bar follows the selection, so links can be shared);
- otherwise `packages-map.json` and `security-advisories.json`, when present (they are copied next to the page by `trunk`).

Data can also come from:

- the URL: `index.html?data=<url of packages-map.json>&advisories=<url of security-advisories.json>`, so a link can point to a given build. The server must allow cross-origin requests (GitHub Pages and `raw.githubusercontent.com` do);
- the URL field of the **Data** window (the address bar is updated so the link can be shared);
- drag and drop of one or both JSON files onto the page.

`dist/` is a static site: it can be published as is on GitHub Pages.

## Continuous integration

`image-scope` can run after each build of an image and publish its history:

```sh
# Command line only (no viewer, no Bevy): fast to build, for CI
cargo build --release --no-default-features

image-scope inspect ghcr.io/owner/image:latest --site site   # → site/image/<build>/
image-scope index site --keep 30 --summary "$GITHUB_STEP_SUMMARY" --url https://owner.github.io/repo
```

- `inspect --site DIR` writes `DIR/<image>/<build>/` (`packages-map.json`, `security-advisories.json`, and `image.json`: version, digest, size, number of packages with a published security fix). The build id is the image creation date (`20261112-2228`);
- `index DIR` writes `DIR/builds.json` for the web viewer, keeps the `--keep` latest builds of each image, and appends a markdown report to `--summary`: latest build of each image, packages added / removed / updated since the previous build, size delta, security fixes to pick up, links to the viewer.

Pushing a `v*` tag runs `.github/workflows/release.yml`, which publishes `image-scope-x86_64-linux.tar.gz` (command line) and `image-scope-web.tar.gz` (web viewer, with relative paths) as release assets.

[custom-desktop-image](https://github.com/benoitlx/custom-desktop-image) uses them in `.github/workflows/image-scope.yml`: after each `bluebuild` run on `main`, it inspects `niri` and `niri-nvidia`, keeps the analyses of the 30 latest builds in the `image-scope-data` branch (a single commit, rewritten each time), and deploys the viewer with this history on GitHub Pages. The per-layer cache is kept between runs with `actions/cache`, so unchanged layers are not downloaded again.

In the graph view:

- the **Data** window loads another image (`ghcr.io/owner/image:tag`), a JSON URL or a local file, and shows what is loaded;
- **drop** `packages-map.json` and / or `security-advisories.json` onto the window to load another image or other advisories (X11 / XWayland only for the native window: winit does not support file drops on Wayland);
- **middle click + drag** to pan, **scroll** to zoom;
- **Ctrl+F** to search a package by name (or summary): **↑ / ↓** to choose, **Enter** to select it and center the camera, **Esc** to clear; matches are circled in cyan in the graph;
- **hover** a package for a quick summary (size, dependencies, exclusive size) and to highlight its direct edges;
- **left click** on a package to open its details (dependencies and reverse dependencies are clickable);
- **left drag** a package to move it: it stays pinned where dropped, **right click** unpins it (or **Unpin all** in the Forces window);
- **Esc** to close the details panel;
- the **Insights** window chooses the node coloring (layer, origin, security) and lists root packages, third-party sources and pending security fixes;
- the **Forces** window tunes the layout; the simulation runs continuously, **Lock** freezes it and **Reheat** gives it a new impulse.

# Features

- One node per package, colored by layer, origin or security status (**Insights** window)
- Force-directed layout with Barnes-Hut repulsion (quadtree, O(n log n)) and adaptive step size (Yifan Hu), runs continuously until locked
- Transitive reduction: dependencies implied by another path (`a → c` when `a → b → c`) are hidden and ignored by the layout (toggle in the Forces window)
- Package details panel: version, layer, size, license, vendor, build info, links, dependencies / required by
- Root packages (nothing depends on them) drawn bigger and in gold, listed in the **Insights** window sorted by exclusive size
- "Why is it installed?": shortest chain from a root package to the selected one (highlighted in pink in the graph), and every root package pulling it in
- Exclusive size: the package plus the dependencies only it needs, i.e. what removing it would free (dominator tree)
- Origin of each package: Fedora, COPR repository, other vendor (negativo17, NVIDIA, Tailscale, ublue-os…) or unknown (local builds), from the `Vendor` / `Packager` rpm tags
- Security: Fedora security advisories from [Bodhi](https://bodhi.fedoraproject.org/) are matched against the source rpm of each Fedora package (rpm version comparison, `~` and `^` included), which tells whether a fix is published (rebuild the image), only in testing, or already included. Each advisory links to Bodhi and to its CVE records. Third-party packages are not covered by Bodhi, and [OSV](https://osv.dev/) has no Fedora ecosystem

# Roadmap

## Readability

- [x] Transitive reduction of the dependency graph
- [x] Select a package to highlight its dependencies and dependents
- [ ] Hide edges to hub packages (e.g. "packages required by more than N others") with a slider
- [ ] Node size proportional to the number of dependents (or to installed size)
- [ ] Show labels only for important nodes, or when zoomed in / hovered
- [ ] LinLog / degree-weighted repulsion (ForceAtlas2 style) to separate clusters better
- [ ] Lay out isolated packages separately (grid) instead of letting them drift to the border
- [x] Search a package by name and center the camera on it
- [ ] Filter by layer (show / hide a layer)

## Making sense of the image

- [x] **Root packages**: packages nothing depends on, i.e. what was actually installed on purpose (shown in gold)
- [x] **"Why is this package here?"**: dependency path(s) from a root package to the selected one
- [x] **Exclusive weight**: space freed by removing a package together with the dependencies only it uses (dominator tree, using `Size`)
- [ ] **Compare two images** (e.g. `niri` vs `niri-nvidia`): added / removed / updated packages, size delta, per layer
- [ ] **Per-layer view**: package count and size per layer, dependencies crossing layers
- [ ] **Detect removed / updated packages** between layers (fill the `dropped` field)
- [ ] **Clusters**: community detection (Louvain) to color stacks such as GNOME, Python, Mesa
- [ ] **Group subpackages** of the same `Sourcerpm` into a single node
- [x] **Third-party packages**: highlight vendors other than "Fedora Project" (RPM Fusion, NVIDIA, COPR, …)
- [ ] Highlight dependency cycles (strongly connected components)

## Data from the web

- [x] **CVEs** affecting Fedora packages, from Fedora security advisories ([Bodhi](https://bodhi.fedoraproject.org/))
- [ ] **CVEs for third-party packages**: COPR builds of Rust / Go tools through their upstream ecosystem in OSV (crates.io for `zellij`, `yazi`, `starship`; Go for `tailscale.com`), NVIDIA security bulletins for the driver, negativo17 builds compared with the matching Fedora / RPM Fusion versions
- [ ] **Available updates** compared with the Fedora repositories ([Bodhi](https://bodhi.fedoraproject.org/), repo metadata)
- [ ] **Upstream releases**: how far behind upstream each package is ([Anitya / release-monitoring.org](https://release-monitoring.org/), [Repology](https://repology.org/))
- [ ] Build information from [Koji](https://koji.fedoraproject.org/)

## Image inspection (`image-scope inspect`)

- [x] Fetch images directly from the registry instead of `podman save` (the old `layers.nu` script)
- [x] Parse the rpm database directly instead of one `rpm` call per requirement
- [ ] Detect rechunked images (bootc / ostree, `chunkah` layers), where layers do not follow the build order and `introduced_in` is meaningless; attribute packages to layers from the files each layer contains instead
- [ ] Label layers with the BlueBuild module that created them (config `history[].created_by`)
- [ ] Progress in bytes for big layers, and verification of the layer digests
- [ ] Older rpm databases (BerkeleyDB, `ndb`) for non-Fedora images

## Integration with [custom-desktop-image](https://github.com/benoitlx/custom-desktop-image)

The image is built daily by BlueBuild from `recipes/recipe.yml` (`niri`) and `recipes/recipe-nvidia.yml` (`niri-nvidia`), on top of `ghcr.io/blue-build/base-images/fedora-base(-nvidia)`, with the shared modules of `recipes/common.yml`.

### Recipe ↔ image

- [ ] **Read the recipe** (`dnf` module: `install`, `remove`, `exclude`, `repos`) and tag each package as *requested by the recipe*, *pulled as a dependency* or *coming from the base image*
- [ ] **Recipe lint**:
  - packages listed in `install` that are already pulled in by another package (could be dropped from the recipe)
  - packages listed in `install` but missing from the image
  - packages listed in `remove` / `exclude` that are still present (e.g. `firefox`, `htop`, `waybar`)
  - root packages that the recipe never asked for (where do they come from?)
- [ ] **Repository audit**: configured COPRs / repo files vs packages actually installed from them. For example, `niri` and `gum` are installed from Fedora although the recipe adds the `yalter/niri` COPR and `charm.repo`, so those repositories may be useless
- [ ] **Base image vs custom image**: inspect `fedora-base(-nvidia)` too and diff, to see exactly what (and how many MiB) the recipe adds
- [ ] **Layer → recipe module**: label layers with the BlueBuild module that created them (OCI history `created_by`: `dnf`, `files`, `fonts`, `initramfs`…) instead of a hash
- [ ] **`niri` vs `niri-nvidia`**: diff the two images of the build matrix (the NVIDIA stack, negativo17 multimedia packages, size)

### Content outside rpm

- [ ] **Files not owned by any rpm**: `ironbar` and `matugen` (built in `shared-stages.yml` and copied into `/usr/bin`), fonts from the `fonts` module, justfiles, systemd units from `files/`. Show them as extra nodes with their size
- [ ] **CVEs of the Rust binaries built from source** (`ironbar`, `matugen`): audit their `Cargo.lock` at the built commit against OSV (crates.io), or build them with `cargo auditable`
- [ ] **Flatpaks** declared in `default-flatpaks` (installed at first boot, not in the image): list them and their runtimes for a complete picture of the system
- [ ] **systemd units** enabled / disabled / masked by the recipe: link each unit to the package that ships it

### Build pipeline

- [x] **Viewer on GitHub Pages**: the web version with the analyses of the latest builds (`builds.json`, `?image=…&build=…`)
- [x] **Report in CI**: markdown summary in `$GITHUB_STEP_SUMMARY` (package diff vs the previous build, size delta, pending security fixes)
- [ ] Report: new third-party packages / sources, and a PR comment for pull requests (needs the PR image, which BlueBuild does not push)
- [ ] **Compare two builds in the viewer**: highlight added / updated packages in the graph
- [ ] **Alerts**: warn when the image grows by more than N MiB, when a new third-party source appears, or when a published security fix is older than the last build (the daily cron build should have picked it up)
- [ ] **History of the daily builds**: inspect past tags from GHCR with `skopeo` and show a timeline (packages added / removed / updated per build, size over time, when a CVE fix landed)
- [ ] **Consistency checks**: `kmod-nvidia` built for the same kernel as `kernel-core`, initramfs rebuilt after the last change (`initramfs` module), Fedora version of the base image matching `image-version`
- [ ] **Signature**: verify the image with the repository's `cosign.pub` and show the result
