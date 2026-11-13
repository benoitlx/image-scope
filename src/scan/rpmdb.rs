//! Lecture d'une base rpm sqlite (`rpmdb.sqlite`) sans le binaire `rpm`.
//!
//! La table `Packages(hnum, blob)` contient un en-tête rpm par paquet installé :
//!
//! ```text
//! il: u32 BE | dl: u32 BE | il × (tag, type, offset, count): 4 × u32 BE | données (dl octets)
//! ```

use crate::model::Node;
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Chemins possibles de la base dans une couche d'image (sans `/` initial).
pub const RPMDB_PATHS: [&str; 3] = [
    "usr/lib/sysimage/rpm/rpmdb.sqlite",
    "usr/share/rpm/rpmdb.sqlite",
    "var/lib/rpm/rpmdb.sqlite",
];

mod tag {
    pub const NAME: u32 = 1000;
    pub const VERSION: u32 = 1001;
    pub const RELEASE: u32 = 1002;
    pub const SUMMARY: u32 = 1004;
    pub const DESCRIPTION: u32 = 1005;
    pub const BUILDTIME: u32 = 1006;
    pub const BUILDHOST: u32 = 1007;
    pub const INSTALLTIME: u32 = 1008;
    pub const SIZE: u32 = 1009;
    pub const VENDOR: u32 = 1011;
    pub const LICENSE: u32 = 1014;
    pub const PACKAGER: u32 = 1015;
    pub const GROUP: u32 = 1016;
    pub const URL: u32 = 1020;
    pub const ARCH: u32 = 1022;
    pub const SOURCERPM: u32 = 1044;
    pub const PROVIDENAME: u32 = 1047;
    pub const REQUIRENAME: u32 = 1049;
    pub const DIRINDEXES: u32 = 1116;
    pub const BASENAMES: u32 = 1117;
    pub const DIRNAMES: u32 = 1118;
    pub const LONGSIZE: u32 = 5009;
    pub const BUGURL: u32 = 5012;
}

mod kind {
    pub const CHAR: u32 = 1;
    pub const INT8: u32 = 2;
    pub const INT16: u32 = 3;
    pub const INT32: u32 = 4;
    pub const INT64: u32 = 5;
    pub const STRING: u32 = 6;
    pub const STRING_ARRAY: u32 = 8;
    pub const I18NSTRING: u32 = 9;
}

/// En-tête rpm décodé paresseusement : index des entrées + zone de données.
pub struct Header<'a> {
    entries: HashMap<u32, (u32, usize, usize)>,
    data: &'a [u8],
}

fn be32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

impl<'a> Header<'a> {
    pub fn parse(blob: &'a [u8]) -> Result<Self, String> {
        let il = be32(blob, 0).ok_or("truncated header")? as usize;
        let dl = be32(blob, 4).ok_or("truncated header")? as usize;
        let start = 8 + 16 * il;
        let data = blob
            .get(start..start + dl)
            .ok_or("header shorter than announced")?;
        let mut entries = HashMap::with_capacity(il);
        for i in 0..il {
            let at = 8 + 16 * i;
            let (Some(tag), Some(ty), Some(offset), Some(count)) = (
                be32(blob, at),
                be32(blob, at + 4),
                be32(blob, at + 8),
                be32(blob, at + 12),
            ) else {
                return Err("truncated index".into());
            };
            entries.insert(tag, (ty, offset as usize, count as usize));
        }
        Ok(Header { entries, data })
    }

    /// `count` chaînes terminées par NUL à partir de `offset`.
    fn read_strings(&self, offset: usize, count: usize) -> Vec<String> {
        let mut out = Vec::with_capacity(count);
        let mut at = offset;
        for _ in 0..count {
            let Some(rest) = self.data.get(at..) else {
                break;
            };
            let Some(len) = rest.iter().position(|&b| b == 0) else {
                break;
            };
            out.push(String::from_utf8_lossy(&rest[..len]).into_owned());
            at += len + 1;
        }
        out
    }

    pub fn string(&self, tag: u32) -> Option<String> {
        let &(ty, offset, _) = self.entries.get(&tag)?;
        match ty {
            kind::STRING | kind::STRING_ARRAY | kind::I18NSTRING => {
                self.read_strings(offset, 1).into_iter().next()
            }
            _ => None,
        }
    }

    /// Chaîne traduite : la première valeur est celle de la locale « C », celle
    /// qu'affiche `rpm` (les suivantes sont des traductions).
    pub fn i18n(&self, tag: u32) -> Option<String> {
        self.string(tag)
    }

    pub fn strings(&self, tag: u32) -> Vec<String> {
        match self.entries.get(&tag) {
            Some(&(kind::STRING_ARRAY | kind::I18NSTRING, offset, count)) => {
                self.read_strings(offset, count)
            }
            Some(&(kind::STRING, offset, _)) => self.read_strings(offset, 1),
            _ => Vec::new(),
        }
    }

    pub fn ints(&self, tag: u32) -> Vec<u64> {
        let Some(&(ty, offset, count)) = self.entries.get(&tag) else {
            return Vec::new();
        };
        let width = match ty {
            kind::CHAR | kind::INT8 => 1,
            kind::INT16 => 2,
            kind::INT32 => 4,
            kind::INT64 => 8,
            _ => return Vec::new(),
        };
        (0..count)
            .map_while(|i| {
                let bytes = self
                    .data
                    .get(offset + i * width..offset + (i + 1) * width)?;
                Some(bytes.iter().fold(0u64, |acc, &b| (acc << 8) | b as u64))
            })
            .collect()
    }

    pub fn int(&self, tag: u32) -> Option<u64> {
        self.ints(tag).first().copied()
    }

    /// Fichiers du paquet : `DIRNAMES[DIRINDEXES[i]] + BASENAMES[i]`.
    pub fn files(&self) -> impl Iterator<Item = String> + '_ {
        let dirs = self.strings(tag::DIRNAMES);
        let indexes = self.ints(tag::DIRINDEXES);
        self.strings(tag::BASENAMES)
            .into_iter()
            .zip(indexes)
            .filter_map(move |(base, i)| Some(format!("{}{base}", dirs.get(i as usize)?)))
    }
}

/// Paquet lu dans la base, avec ce qu'il faut pour résoudre les dépendances.
pub struct Package {
    pub node: Node,
    requires: Vec<String>,
    provides: Vec<String>,
    files: Vec<String>,
}

impl Package {
    fn from_header(h: &Header) -> Result<Self, String> {
        let name = h.string(tag::NAME).ok_or("package without name")?;
        let version = h.string(tag::VERSION);
        let release = h.string(tag::RELEASE);
        let arch = h.string(tag::ARCH);
        // Comme `rpm -qa` : name-version-release[.arch].
        let mut fullname = format!(
            "{name}-{}-{}",
            version.as_deref().unwrap_or(""),
            release.as_deref().unwrap_or("")
        );
        if let Some(arch) = &arch {
            fullname = format!("{fullname}.{arch}");
        }
        let node = Node {
            Name: name,
            Version: version,
            Release: release,
            Arch: arch,
            Installtime: h.int(tag::INSTALLTIME),
            Group: h.i18n(tag::GROUP),
            Size: h.int(tag::LONGSIZE).or_else(|| h.int(tag::SIZE)),
            License: h.string(tag::LICENSE),
            Sourcerpm: h.string(tag::SOURCERPM),
            Buildtime: h.int(tag::BUILDTIME),
            Buildhost: h.string(tag::BUILDHOST),
            Packager: h.string(tag::PACKAGER),
            Vendor: h.string(tag::VENDOR),
            Url: h.string(tag::URL),
            Bugurl: h.string(tag::BUGURL),
            Summary: h.i18n(tag::SUMMARY),
            Description: h.i18n(tag::DESCRIPTION),
            introduced_in: String::new(),
            dep: Vec::new(),
            dropped: false,
            fullname: Some(fullname),
        };
        Ok(Package {
            node,
            requires: h.strings(tag::REQUIRENAME),
            provides: h.strings(tag::PROVIDENAME),
            // Pour les dépendances de type fichier (« /bin/sh »).
            files: h.files().collect(),
        })
    }
}

/// Lit tous les paquets d'une base, dans l'ordre de la base (comme `rpm -qa`).
pub fn read_packages(path: &Path) -> Result<Vec<Package>, String> {
    // `immutable` : lecture seule stricte, sans créer de fichiers -wal / -shm à côté.
    let uri = format!("file:{}?immutable=1", path.display());
    let db = rusqlite::Connection::open_with_flags(
        uri,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    let mut statement = db
        .prepare("SELECT blob FROM Packages ORDER BY hnum")
        .map_err(|e| format!("{}: not an rpm database? {e}", path.display()))?;
    let blobs = statement
        .query_map([], |row| row.get::<_, Vec<u8>>(0))
        .map_err(|e| e.to_string())?;
    let mut packages = Vec::new();
    for blob in blobs {
        let blob = blob.map_err(|e| e.to_string())?;
        packages.push(Package::from_header(&Header::parse(&blob)?)?);
    }
    Ok(packages)
}

/// Remplit `dep` comme `rpm -qR` puis `rpm -q --whatprovides` pour chaque besoin :
/// paquets qui fournissent ce nom (provides) ou ce fichier.
pub fn resolve_dependencies(packages: Vec<Package>) -> Vec<Node> {
    let wanted_files: HashSet<&str> = packages
        .iter()
        .flat_map(|p| &p.requires)
        .filter(|r| r.starts_with('/'))
        .map(String::as_str)
        .collect();

    let mut providers: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, p) in packages.iter().enumerate() {
        let files = p.files.iter().filter(|f| wanted_files.contains(f.as_str()));
        for capability in p.provides.iter().chain(files) {
            let list = providers.entry(capability).or_default();
            if list.last() != Some(&i) {
                list.push(i);
            }
        }
    }

    let deps: Vec<Vec<String>> = packages
        .iter()
        .map(|p| {
            let mut seen = HashSet::new();
            p.requires
                .iter()
                .flat_map(|r| providers.get(r.as_str()).into_iter().flatten())
                .filter(|&&i| seen.insert(i))
                .map(|&i| packages[i].node.Name.clone())
                .collect::<Vec<_>>()
        })
        .collect();
    // Plusieurs fournisseurs peuvent porter le même nom (multilib) : dédoublonner.
    packages
        .into_iter()
        .zip(deps)
        .map(|(p, mut dep)| {
            let mut seen = HashSet::new();
            dep.retain(|d| seen.insert(d.clone()));
            Node { dep, ..p.node }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Construit un en-tête minimal : (tag, type, données brutes, count).
    fn header(entries: &[(u32, u32, Vec<u8>, u32)]) -> Vec<u8> {
        let mut index = Vec::new();
        let mut data: Vec<u8> = Vec::new();
        for (tag, ty, bytes, count) in entries {
            for v in [*tag, *ty, data.len() as u32, *count] {
                index.extend(v.to_be_bytes());
            }
            data.extend(bytes);
        }
        let mut blob = Vec::new();
        blob.extend((entries.len() as u32).to_be_bytes());
        blob.extend((data.len() as u32).to_be_bytes());
        blob.extend(index);
        blob.extend(data);
        blob
    }

    #[test]
    fn parses_header_entries() {
        let blob = header(&[
            (tag::NAME, kind::STRING, b"bash\0".to_vec(), 1),
            (tag::SUMMARY, kind::I18NSTRING, b"The shell\0".to_vec(), 1),
            (
                tag::REQUIRENAME,
                kind::STRING_ARRAY,
                b"/bin/sh\0libc.so.6\0".to_vec(),
                2,
            ),
            (
                tag::SIZE,
                kind::INT32,
                8_000_000u32.to_be_bytes().to_vec(),
                1,
            ),
            (
                tag::DIRINDEXES,
                kind::INT32,
                [0u32, 1].iter().flat_map(|v| v.to_be_bytes()).collect(),
                2,
            ),
            (
                tag::BASENAMES,
                kind::STRING_ARRAY,
                b"bash\0sh\0".to_vec(),
                2,
            ),
            (
                tag::DIRNAMES,
                kind::STRING_ARRAY,
                b"/usr/bin/\0/bin/\0".to_vec(),
                2,
            ),
        ]);
        let h = Header::parse(&blob).unwrap();
        assert_eq!(h.string(tag::NAME).as_deref(), Some("bash"));
        assert_eq!(h.i18n(tag::SUMMARY).as_deref(), Some("The shell"));
        assert_eq!(h.strings(tag::REQUIRENAME), ["/bin/sh", "libc.so.6"]);
        assert_eq!(h.int(tag::SIZE), Some(8_000_000));
        assert_eq!(h.files().collect::<Vec<_>>(), ["/usr/bin/bash", "/bin/sh"]);
        assert_eq!(h.string(tag::VENDOR), None);
        assert!(Header::parse(&blob[..blob.len() - 1]).is_err());
    }

    fn package(name: &str, requires: &[&str], provides: &[&str], files: &[&str]) -> Package {
        let s = |v: &[&str]| v.iter().map(|s| s.to_string()).collect();
        Package {
            node: serde_json::from_value(serde_json::json!({"Name": name, "introduced_in": ""}))
                .unwrap(),
            requires: s(requires),
            provides: s(provides),
            files: s(files),
        }
    }

    #[test]
    fn resolves_names_files_and_multiple_providers() {
        let nodes = resolve_dependencies(vec![
            package(
                "app",
                &["libfoo.so", "/bin/sh", "rpmlib(X)", "mta"],
                &["app"],
                &[],
            ),
            package("foo", &[], &["libfoo.so"], &[]),
            package("bash", &[], &["bash"], &["/bin/sh", "/usr/bin/bash"]),
            package("postfix", &[], &["mta"], &[]),
            package("sendmail", &[], &["mta"], &[]),
        ]);
        assert_eq!(nodes[0].dep, ["foo", "bash", "postfix", "sendmail"]);
        assert!(nodes[1].dep.is_empty());
    }
}
