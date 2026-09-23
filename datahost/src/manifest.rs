//! What a 1.12.1 `Data` folder must hold and how the host describes the copy it offers.
//!
//! This is the server half of the iNilla client's data contract (its `data` module): the client
//! fetches [`Manifest`] as `GET <base>/manifest.json`, refuses one whose `format` or `build` it does
//! not know or that lacks a [`REQUIRED`] archive, downloads each file from `<base>/files/<name>`,
//! and checks it against `sha256`. Change the shape here only together with the client.
//!
//! Only the MPQs at the root of `Data/` matter: that is all the client mounts (the ten base
//! archives, `patch.MPQ`, `patch-?.MPQ`, `speech2.MPQ`). Nothing here reads inside an archive.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The archives a 1.12.1 client cannot start without.
pub const REQUIRED: &[&str] = &[
    "dbc.MPQ",
    "fonts.MPQ",
    "interface.MPQ",
    "misc.MPQ",
    "model.MPQ",
    "patch.MPQ",
    "sound.MPQ",
    "speech.MPQ",
    "terrain.MPQ",
    "texture.MPQ",
    "wmo.MPQ",
];

/// Archives a stock 1.12.1 install also has; missing them is worth a warning, not a refusal.
pub const EXPECTED: &[&str] = &["patch-2.MPQ"];

/// The manifest layout this host writes (the client refuses any other).
pub const MANIFEST_FORMAT: u32 = 1;

/// The client build the archives belong to: 1.12.1.
pub const CLIENT_BUILD: u32 = 5875;

/// Whether `name` is an archive the client's chain would mount (case-insensitive).
pub fn is_mounted_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if REQUIRED.iter().any(|r| r.eq_ignore_ascii_case(&lower)) || lower == "speech2.mpq" {
        return true;
    }
    lower
        .strip_prefix("patch-")
        .and_then(|r| r.strip_suffix(".mpq"))
        .is_some_and(|mid| mid.chars().count() == 1)
}

/// What a `Data` folder holds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DataStatus {
    pub dir: PathBuf,
    /// `(name as found, size)` for every mountable archive present, sorted by name.
    pub present: Vec<(String, u64)>,
    pub missing_required: Vec<&'static str>,
    pub missing_expected: Vec<&'static str>,
}

impl DataStatus {
    pub fn total_bytes(&self) -> u64 {
        self.present.iter().map(|(_, s)| s).sum()
    }
}

/// Look at `dir` (it need not exist).
pub fn status(dir: &Path) -> DataStatus {
    let mut present = Vec::new();
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            let Ok(name) = e.file_name().into_string() else {
                continue;
            };
            if !is_mounted_name(&name) {
                continue;
            }
            if let Ok(meta) = e.metadata() {
                if meta.is_file() && meta.len() > 0 {
                    present.push((name, meta.len()));
                }
            }
        }
    }
    present.sort();
    let has = |want: &str| present.iter().any(|(n, _)| n.eq_ignore_ascii_case(want));
    DataStatus {
        dir: dir.to_path_buf(),
        missing_required: REQUIRED.iter().copied().filter(|r| !has(r)).collect(),
        missing_expected: EXPECTED.iter().copied().filter(|r| !has(r)).collect(),
        present,
    }
}

/// One file the host offers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestFile {
    pub name: String,
    pub size: u64,
    /// Lower-case hex SHA-256 of the whole file.
    pub sha256: String,
}

/// What the host offers: `GET <base>/manifest.json`, files at `GET <base>/files/<name>`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub format: u32,
    /// The client build the files belong to (5875 = 1.12.1).
    pub build: u32,
    /// `enUS`, `deDE`, … as the operator declared it.
    #[serde(default)]
    pub locale: Option<String>,
    pub files: Vec<ManifestFile>,
}

impl Manifest {
    pub fn total_bytes(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }

    /// Hash every mountable archive in `dir`. `progress(name, bytes_done, bytes_total)`.
    pub fn build_from(
        dir: &Path,
        locale: Option<String>,
        mut progress: impl FnMut(&str, u64, u64),
    ) -> io::Result<Self> {
        let st = status(dir);
        let total = st.total_bytes();
        let mut done = 0;
        let mut files = Vec::new();
        for (name, size) in &st.present {
            let sha = sha256_file(&dir.join(name), |n| progress(name, done + n, total))?;
            done += size;
            files.push(ManifestFile {
                name: name.clone(),
                size: *size,
                sha256: sha,
            });
        }
        Ok(Manifest {
            format: MANIFEST_FORMAT,
            build: CLIENT_BUILD,
            locale,
            files,
        })
    }
}

/// SHA-256 of a file, reporting bytes hashed so far.
pub fn sha256_file(path: &Path, mut progress: impl FnMut(u64)) -> io::Result<String> {
    let mut f = fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut n_total = 0u64;
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        n_total += n as u64;
        progress(n_total);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("inilla-datahost-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    pub(crate) fn fill(dir: &Path, names: &[&str]) {
        for n in names {
            fs::write(dir.join(n), format!("mpq:{n}")).unwrap();
        }
    }

    #[test]
    fn mounted_names_follow_the_chain_law() {
        assert!(is_mounted_name("DBC.mpq"));
        assert!(is_mounted_name("patch-3.MPQ"));
        assert!(is_mounted_name("speech2.MPQ"));
        assert!(!is_mounted_name("patch-10.MPQ"));
        assert!(!is_mounted_name("base.MPQ"));
        assert!(!is_mounted_name("realmlist.wtf"));
    }

    #[test]
    fn status_reports_what_is_missing() {
        let d = tmp("status");
        fill(&d, &REQUIRED[..5]);
        let st = status(&d);
        assert_eq!(st.missing_required.len(), REQUIRED.len() - 5);
        fill(&d, REQUIRED);
        fill(&d, &["backup.MPQ", "Credits.html"]);
        let st = status(&d);
        assert!(st.missing_required.is_empty());
        assert_eq!(st.missing_expected, vec!["patch-2.MPQ"]);
        assert_eq!(
            st.present.len(),
            REQUIRED.len(),
            "only mountable archives are offered"
        );
    }

    #[test]
    fn the_manifest_hashes_what_it_lists() {
        let d = tmp("manifest");
        fill(&d, REQUIRED);
        let m = Manifest::build_from(&d, Some("enUS".into()), |_, _, _| {}).unwrap();
        assert_eq!((m.format, m.build), (1, 5875));
        assert_eq!(m.files.len(), REQUIRED.len());
        let dbc = m.files.iter().find(|f| f.name == "dbc.MPQ").unwrap();
        assert_eq!(dbc.size, "mpq:dbc.MPQ".len() as u64);
        assert_eq!(dbc.sha256.len(), 64);
        assert!(dbc
            .sha256
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
        let json = serde_json::to_value(&m).unwrap();
        assert_eq!(json["files"][0]["name"], "dbc.MPQ");
        assert_eq!(json["locale"], "enUS");
    }
}
