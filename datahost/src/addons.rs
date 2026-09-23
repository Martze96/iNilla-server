//! **Addons** the host hands out with the game data: every folder under an addons directory that
//! holds its own `<Folder>.toc` (the layout 1.12 itself loads from `Interface/AddOns/`), listed
//! with each file's size and SHA-256 so the app can install and update them the way it downloads
//! the archives.
//!
//! ```text
//! GET <base>/addons.json          {"format":1,"locale":"deDE","addons":[{"name","version","files":[{"path","size","sha256"}]}]}
//! GET <base>/addons/<path>        one listed file, e.g. addons/pfQuest/pfQuest.toc
//! ```
//!
//! Several directories can feed one list (the image's bundled addons, then the operator's own);
//! a later directory's addon replaces an earlier one of the same name.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::manifest::sha256_file;

pub const ADDON_FORMAT: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddonFile {
    /// `pfQuest/db/deDE/quests.lua`: forward slashes, starting with the addon's folder.
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Addon {
    /// The folder name, which is also the `.toc`'s name.
    pub name: String,
    /// The `.toc`'s `## Version:`, when it has one.
    #[serde(default)]
    pub version: Option<String>,
    pub files: Vec<AddonFile>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddonManifest {
    pub format: u32,
    pub locale: String,
    pub addons: Vec<Addon>,
}

/// A data set's addons: the manifest, and where each listed file is on disk.
#[derive(Clone, Debug, Default)]
pub struct AddonSet {
    pub manifest_json: String,
    pub files: BTreeMap<String, (PathBuf, u64)>,
    pub count: usize,
}

/// The addons under `dirs`, in order: each immediate sub-folder with a `.toc` of its own name.
pub fn scan(dirs: &[PathBuf], locale: &str) -> Result<AddonSet, String> {
    let mut found: BTreeMap<String, (PathBuf, Addon)> = BTreeMap::new();
    for dir in dirs {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        let mut folders: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        folders.sort();
        for folder in folders {
            let Some(name) = folder
                .file_name()
                .and_then(|n| n.to_str())
                .map(str::to_string)
            else {
                continue;
            };
            if name.starts_with('.') {
                continue;
            }
            let Some(toc) = find_toc(&folder, &name) else {
                continue;
            };
            let version = fs::read_to_string(&toc).ok().and_then(|t| toc_version(&t));
            let mut files = Vec::new();
            walk(&folder, &name, &mut files)?;
            files.sort_by(|a, b| a.path.cmp(&b.path));
            found.insert(
                name.to_ascii_lowercase(),
                (
                    folder,
                    Addon {
                        name,
                        version,
                        files,
                    },
                ),
            );
        }
    }
    let mut set = AddonSet::default();
    let mut addons = Vec::new();
    for (folder, addon) in found.into_values() {
        let parent = folder.parent().unwrap_or(&folder).to_path_buf();
        for f in &addon.files {
            set.files
                .insert(f.path.clone(), (parent.join(&f.path), f.size));
        }
        addons.push(addon);
    }
    set.count = addons.len();
    set.manifest_json = serde_json::to_string(&AddonManifest {
        format: ADDON_FORMAT,
        locale: locale.to_string(),
        addons,
    })
    .map_err(|e| e.to_string())?;
    Ok(set)
}

fn find_toc(folder: &Path, name: &str) -> Option<PathBuf> {
    let want = format!("{name}.toc").to_ascii_lowercase();
    fs::read_dir(folder)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.is_file()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.to_ascii_lowercase() == want)
        })
}

fn toc_version(toc: &str) -> Option<String> {
    toc.lines().find_map(|l| {
        let l = l.trim_start_matches('\u{feff}').trim();
        let rest = l.strip_prefix("##")?.trim();
        let (key, value) = rest.split_once(':')?;
        (key.trim().eq_ignore_ascii_case("version")).then(|| value.trim().to_string())
    })
}

/// Every regular file under `dir`, as `prefix/relative/path`. Hidden entries and symlinks are
/// skipped: a link could point anywhere on the disk.
fn walk(dir: &Path, prefix: &str, out: &mut Vec<AddonFile>) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for e in entries.flatten() {
        let Ok(name) = e.file_name().into_string() else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        let Ok(kind) = e.file_type() else {
            continue;
        };
        let rel = format!("{prefix}/{name}");
        if kind.is_dir() {
            walk(&e.path(), &rel, out)?;
        } else if kind.is_file() {
            let size = e.metadata().map(|m| m.len()).unwrap_or(0);
            let sha256 = sha256_file(&e.path(), |_| {}).map_err(|err| format!("{}: {err}", rel))?;
            out.push(AddonFile {
                path: rel,
                size,
                sha256,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::tests::tmp;

    #[test]
    fn folders_with_their_own_toc_are_addons() {
        let bundled = tmp("addons-bundled");
        let custom = tmp("addons-custom");
        fs::create_dir_all(bundled.join("pfQuest/db/enUS")).unwrap();
        fs::write(
            bundled.join("pfQuest/pfQuest.toc"),
            "## Interface: 11200\n## Version: 7.0.1\npfQuest.lua\n",
        )
        .unwrap();
        fs::write(bundled.join("pfQuest/db/enUS/quests.lua"), "q").unwrap();
        fs::write(bundled.join("pfQuest/.hidden"), "x").unwrap();
        fs::create_dir_all(bundled.join("NotAnAddon")).unwrap();
        fs::write(bundled.join("NotAnAddon/readme.txt"), "x").unwrap();
        // The operator's own copy of an addon replaces the bundled one.
        fs::create_dir_all(custom.join("PFQUEST")).unwrap();
        fs::write(custom.join("PFQUEST/pfquest.TOC"), "## Version: 9\n").unwrap();
        let set = scan(std::slice::from_ref(&bundled), "enUS").unwrap();
        assert_eq!(set.count, 1);
        let m: AddonManifest = serde_json::from_str(&set.manifest_json).unwrap();
        assert_eq!(m.addons[0].name, "pfQuest");
        assert_eq!(m.addons[0].version.as_deref(), Some("7.0.1"));
        let paths: Vec<&str> = m.addons[0].files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["pfQuest/db/enUS/quests.lua", "pfQuest/pfQuest.toc"]);
        assert!(set.files.contains_key("pfQuest/db/enUS/quests.lua"));
        let both = scan(&[bundled, custom], "enUS").unwrap();
        let m: AddonManifest = serde_json::from_str(&both.manifest_json).unwrap();
        assert_eq!(m.addons.len(), 1);
        assert_eq!(m.addons[0].name, "PFQUEST");
        assert_eq!(m.addons[0].version.as_deref(), Some("9"));
    }
}
