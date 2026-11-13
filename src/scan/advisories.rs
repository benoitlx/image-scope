//! Avis de sécurité Fedora depuis Bodhi : mises à jour de type « security » de la
//! release, stables et en test, mises en cache une journée.

use super::cache_dir;
use super::inspect::Progress;
use crate::model::{Advisory, AdvisoryFile};
use serde::Deserialize;
use std::time::{Duration, SystemTime};

const BODHI: &str = "https://bodhi.fedoraproject.org";
const CACHE_LIFETIME: Duration = Duration::from_secs(24 * 3600);
const PARALLEL_PAGES: usize = 6;

#[derive(Deserialize)]
struct Page {
    updates: Vec<Update>,
    #[serde(default)]
    pages: usize,
    #[serde(default)]
    total: usize,
}

#[derive(Deserialize)]
struct Update {
    alias: String,
    status: String,
    #[serde(default)]
    severity: String,
    date_stable: Option<String>,
    date_submitted: Option<String>,
    #[serde(default)]
    builds: Vec<Build>,
    #[serde(default)]
    notes: Option<String>,
    #[serde(default)]
    bugs: Vec<Bug>,
}

#[derive(Deserialize)]
struct Build {
    nvr: String,
}

#[derive(Deserialize)]
struct Bug {
    title: Option<String>,
}

/// `release` : `F45`. Le cache (un jour) est ignoré avec `refresh`.
pub fn fetch(release: &str, refresh: bool, progress: Progress) -> Result<AdvisoryFile, String> {
    let cache = cache_dir().join(format!("bodhi-{release}.json"));
    let fresh = std::fs::metadata(&cache)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .is_some_and(|age| age < CACHE_LIFETIME);
    if fresh && !refresh {
        if let Some(file) = std::fs::read_to_string(&cache)
            .ok()
            .and_then(|s| serde_json::from_str::<AdvisoryFile>(&s).ok())
        {
            progress(format!("Using cached advisories: {}", cache.display()));
            return Ok(file);
        }
    }

    let agent: ureq::Agent = ureq::Agent::config_builder()
        .user_agent("image-scope")
        .timeout_global(Some(Duration::from_secs(120)))
        .build()
        .into();
    let mut advisories = Vec::new();
    for status in ["stable", "testing"] {
        for update in fetch_updates(&agent, release, status, progress)? {
            advisories.push(normalize(update));
        }
    }
    let file = AdvisoryFile::new(release.to_owned(), now_iso8601(), advisories);
    if let Some(dir) = cache.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&cache, serde_json::to_string(&file).unwrap_or_default());
    Ok(file)
}

fn fetch_updates(
    agent: &ureq::Agent,
    release: &str,
    status: &str,
    progress: Progress,
) -> Result<Vec<Update>, String> {
    let get = |page: usize| -> Result<Page, String> {
        let url = format!(
            "{BODHI}/updates/?type=security&releases={release}&status={status}&rows_per_page=100&page={page}"
        );
        agent
            .get(&url)
            .call()
            .map_err(|e| format!("{url}: {e}"))?
            .body_mut()
            .with_config()
            .limit(256 * 1024 * 1024)
            .read_json()
            .map_err(|e| format!("{url}: {e}"))
    };
    let first = get(1)?;
    progress(format!(
        "Bodhi {release} {status}: {} updates, {} pages",
        first.total, first.pages
    ));
    let mut updates = first.updates;
    let rest: Vec<usize> = (2..=first.pages).collect();
    for chunk in rest.chunks(PARALLEL_PAGES) {
        let pages: Vec<Result<Page, String>> = std::thread::scope(|s| {
            let handles: Vec<_> = chunk.iter().map(|&p| s.spawn(move || get(p))).collect();
            handles
                .into_iter()
                .map(|h| h.join().expect("Bodhi worker panicked"))
                .collect()
        });
        for page in pages {
            updates.extend(page?.updates);
        }
    }
    Ok(updates)
}

fn normalize(u: Update) -> Advisory {
    let text: Vec<&str> = u
        .notes
        .as_deref()
        .into_iter()
        .chain(u.bugs.iter().filter_map(|b| b.title.as_deref()))
        .collect();
    Advisory {
        url: format!("{BODHI}/updates/{}", u.alias),
        alias: u.alias,
        status: u.status,
        severity: u.severity,
        date: u.date_stable.or(u.date_submitted),
        builds: u.builds.into_iter().map(|b| b.nvr).collect(),
        cves: find_cves(&text.join("\n")),
    }
}

/// Identifiants `CVE-AAAA-NNNN…` du texte, sans doublons, dans l'ordre.
fn find_cves(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let bytes = text.as_bytes();
    let mut from = 0;
    while let Some(i) = text[from..].find("CVE-").map(|i| i + from) {
        let year = &bytes[i + 4..(i + 8).min(bytes.len())];
        let mut end = i + 9;
        if year.len() == 4 && year.iter().all(u8::is_ascii_digit) && bytes.get(i + 8) == Some(&b'-')
        {
            while bytes.get(end).is_some_and(u8::is_ascii_digit) {
                end += 1;
            }
            if end > i + 9 {
                let id = &text[i..end];
                if !out.iter().any(|c| c == id) {
                    out.push(id.to_owned());
                }
            }
        }
        from = i + 4;
    }
    out
}

/// Date courante, `2026-11-13T12:00:00+0000` (UTC).
pub fn now_iso8601() -> String {
    let secs = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (days, rem) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    // Jours depuis 1970 → date civile (algorithme de H. Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}+0000",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_cve_identifiers() {
        assert_eq!(
            find_cves(
                "Fixes CVE-2026-1234 and CVE-2025-98765.\nCVE-2026-1234 again, CVE-20x, CVE-2026-"
            ),
            ["CVE-2026-1234", "CVE-2025-98765"]
        );
    }

    #[test]
    fn formats_current_date() {
        let now = now_iso8601();
        assert_eq!(now.len(), 24);
        assert!(now.starts_with("20") && now.ends_with("+0000"));
    }
}
