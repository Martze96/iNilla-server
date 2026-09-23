//! **inilla-datahost** — hand your own World of Warcraft 1.12.1 `Data` folder to your own iPhone,
//! once, with the addons that go with it. The iNilla app's setup screen downloads both from here
//! (resumable, SHA-256 verified).
//!
//! ```text
//! inilla-datahost serve --data "/games/WoW 1.12.1/Data"
//!     → http://<this machine>:8099/   (enter it in iNilla's setup)
//! inilla-datahost serve --data enUS=/games/enUS/Data --data deDE=/games/deDE/Data \
//!                       --addons /addons/all --addons deDE=/addons/deDE --token s3cret
//!     → the same, in two languages, with addons, behind a token
//! inilla-datahost manifest --data "/games/WoW 1.12.1/Data"
//!     → prints the manifest (and caches it) without serving
//! ```
//!
//! It serves exactly what the client mounts — the MPQs at the root of each `Data/` — and the
//! addons it was given, nothing else:
//!
//! | request | answer |
//! |---|---|
//! | `GET /locales.json` | the languages this host has game data for, the first one the default |
//! | `GET [/<locale>]/manifest.json` | the archives: names, sizes, SHA-256 |
//! | `GET\|HEAD [/<locale>]/files/<name>` | one archive, with `Range`, so a download resumes |
//! | `GET [/<locale>]/addons.json` | the addons for that language, every file hashed |
//! | `GET\|HEAD [/<locale>]/addons/<path>` | one addon file |
//!
//! Without a `/<locale>` prefix a request means the first `--data`, so an app that knows only
//! `/manifest.json` keeps working. Hashing 5 GB takes a minute or two; each manifest is cached
//! (`.inilla-manifest-<locale>.json`, beside the data or in `--cache-dir`) and rebuilt when an
//! archive's size or mtime changes.
//!
//! The files are Blizzard's. Serve them only to yourself and people who own the game, and prefer
//! a LAN or a VPN (Tailscale, WireGuard) to the open internet — with a token there.

mod addons;
mod http;
mod manifest;

use std::fs;
use std::io::{Seek, SeekFrom};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use clap::{Parser, Subcommand};

use crate::addons::AddonSet;
use crate::http::{Head, Request};
use crate::manifest::Manifest;

#[derive(Parser)]
#[command(name = "inilla-datahost", version, about)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Serve the Data folder(s) and addons.
    Serve {
        /// A client's `Data` folder, as `DIR` or `LOCALE=DIR` (e.g. `deDE=/games/de/Data`).
        /// Repeat for more languages; the first is the default. A folder without archives is
        /// skipped, so an optional language can stay mounted empty.
        #[arg(long = "data", required = true)]
        data: Vec<String>,
        /// A folder of addons (`<Addon>/<Addon>.toc`), as `DIR` for every language or
        /// `LOCALE=DIR` for one. Repeatable; later folders replace an addon of the same name.
        #[arg(long = "addons")]
        addons: Vec<String>,
        /// Address to listen on.
        #[arg(long, env = "INILLA_DATA_BIND", default_value = "0.0.0.0:8099")]
        bind: String,
        /// Require `Authorization: Bearer <token>` (or `?token=`). Without one the host is open
        /// to everyone who can reach it: fine on a home LAN, not on the internet.
        #[arg(long, env = "INILLA_DATA_TOKEN")]
        token: Option<String>,
        /// Accepted for compatibility: no token is now simply the default.
        #[arg(long, hide = true)]
        no_token: bool,
        /// The locale of a `--data` given without one.
        #[arg(long, env = "INILLA_DATA_LOCALE", default_value = "enUS")]
        locale: String,
        /// Where to cache the manifests, when the Data folders are read-only.
        #[arg(long, env = "INILLA_DATA_CACHE_DIR")]
        cache_dir: Option<PathBuf>,
    },
    /// Build (or refresh) the manifest and print it.
    Manifest {
        #[arg(long, env = "INILLA_DATA_DIR")]
        data: PathBuf,
        #[arg(long, env = "INILLA_DATA_LOCALE", default_value = "enUS")]
        locale: String,
        #[arg(long, env = "INILLA_DATA_CACHE_DIR")]
        cache_dir: Option<PathBuf>,
    },
}

fn main() {
    let cli = Cli::parse();
    let result = match cli.cmd {
        Cmd::Serve {
            data,
            addons,
            bind,
            token,
            no_token: _,
            locale,
            cache_dir,
        } => serve(&data, &addons, &bind, token, &locale, cache_dir.as_deref()),
        Cmd::Manifest {
            data,
            locale,
            cache_dir,
        } => manifest(&data, &locale, cache_dir.as_deref()).map(|m| {
            println!("{}", serde_json::to_string_pretty(&m).unwrap_or_default());
        }),
    };
    if let Err(e) = result {
        eprintln!("inilla-datahost: {e}");
        std::process::exit(1);
    }
}

/// `deDE=/path` → `(Some("deDE"), "/path")`; a plain path has no locale. A locale is four ASCII
/// letters, so a Windows drive (`C:\…`) or a path with `=` further in is still just a path.
fn split_locale(arg: &str) -> (Option<&str>, &str) {
    match arg.split_once('=') {
        Some((l, rest)) if l.len() == 4 && l.chars().all(|c| c.is_ascii_alphabetic()) => {
            (Some(l), rest)
        }
        _ => (None, arg),
    }
}

/// The cache's key: every archive's name, size and mtime, and the locale it was built for.
fn fingerprint(dir: &Path, locale: &str) -> String {
    let st = manifest::status(dir);
    let files = st
        .present
        .iter()
        .map(|(n, size)| {
            let mtime = fs::metadata(dir.join(n))
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs());
            format!("{n}:{size}:{mtime}")
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("{locale}|{files}")
}

fn manifest(dir: &Path, locale: &str, cache_dir: Option<&Path>) -> Result<Manifest, String> {
    let st = manifest::status(dir);
    if st.present.is_empty() {
        return Err(format!("no 1.12.1 archives in {}", dir.display()));
    }
    if !st.missing_required.is_empty() {
        eprintln!(
            "inilla-datahost: warning — {} is missing {} (the app will refuse this copy)",
            dir.display(),
            st.missing_required.join(", ")
        );
    }
    if !st.missing_expected.is_empty() {
        eprintln!(
            "inilla-datahost: note — {} has no {}",
            dir.display(),
            st.missing_expected.join(", ")
        );
    }
    let cache = cache_dir
        .unwrap_or(dir)
        .join(format!(".inilla-manifest-{locale}.json"));
    let fp = fingerprint(dir, locale);
    if let Ok(text) = fs::read_to_string(&cache) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
            if v["fingerprint"].as_str() == Some(fp.as_str()) {
                if let Ok(m) = serde_json::from_value::<Manifest>(v["manifest"].clone()) {
                    eprintln!(
                        "inilla-datahost: {locale} manifest from cache {}",
                        cache.display()
                    );
                    return Ok(m);
                }
            }
        }
    }
    eprintln!(
        "inilla-datahost: hashing {} {locale} archives ({:.1} GB)…",
        st.present.len(),
        st.total_bytes() as f64 / 1e9
    );
    let mut last = u64::MAX;
    let m = Manifest::build_from(dir, Some(locale.to_string()), |name, done, total| {
        // Whole percent steps, one line each: this usually runs under `docker logs`, not a TTY.
        let pct = done * 100 / total.max(1);
        if pct != last && pct % 5 == 0 {
            last = pct;
            eprintln!("  {pct:3}%  {name}");
        }
    })
    .map_err(|e| e.to_string())?;
    let body = serde_json::json!({ "fingerprint": fp, "manifest": m });
    // A read-only install with no cache dir just goes uncached.
    if let Err(e) = fs::write(&cache, body.to_string()) {
        eprintln!(
            "inilla-datahost: not caching the manifest ({}: {e})",
            cache.display()
        );
    }
    Ok(m)
}

/// One language's game data and addons.
struct DataSet {
    locale: String,
    dir: PathBuf,
    manifest: Manifest,
    manifest_json: String,
    addons: AddonSet,
}

struct Host {
    /// The first is the default: what a request without a `/<locale>` prefix gets.
    sets: Vec<DataSet>,
    locales_json: String,
    token: Option<String>,
}

impl Host {
    /// The data set a path names, and the rest of the path: `/deDE/files/x` → (deDE, `/files/x`).
    fn route<'a>(&self, path: &'a str) -> (&DataSet, &'a str) {
        if let Some(rest) = path.strip_prefix('/') {
            let (first, tail) = rest.split_once('/').unwrap_or((rest, ""));
            if let Some(set) = self
                .sets
                .iter()
                .find(|s| s.locale.eq_ignore_ascii_case(first))
            {
                return (
                    set,
                    if tail.is_empty() {
                        "/"
                    } else {
                        &path[first.len() + 1..]
                    },
                );
            }
        }
        (&self.sets[0], path)
    }
}

fn load(
    data: &[String],
    addons: &[String],
    default_locale: &str,
    cache_dir: Option<&Path>,
) -> Result<Vec<DataSet>, String> {
    let mut sets = Vec::new();
    for arg in data {
        let (locale, dir) = split_locale(arg);
        let locale = locale.unwrap_or(default_locale).to_string();
        let dir = PathBuf::from(dir);
        if manifest::status(&dir).present.is_empty() {
            eprintln!(
                "inilla-datahost: no archives in {} — {locale} is not offered",
                dir.display()
            );
            continue;
        }
        if sets
            .iter()
            .any(|s: &DataSet| s.locale.eq_ignore_ascii_case(&locale))
        {
            return Err(format!(
                "two --data folders with archives are both {locale}"
            ));
        }
        let m = manifest(&dir, &locale, cache_dir)?;
        let addon_dirs: Vec<PathBuf> = addons
            .iter()
            .filter_map(|a| match split_locale(a) {
                (None, d) => Some(PathBuf::from(d)),
                (Some(l), d) if l.eq_ignore_ascii_case(&locale) => Some(PathBuf::from(d)),
                _ => None,
            })
            .collect();
        let addons = addons::scan(&addon_dirs, &locale)?;
        sets.push(DataSet {
            manifest_json: serde_json::to_string(&m).map_err(|e| e.to_string())?,
            manifest: m,
            locale,
            dir,
            addons,
        });
    }
    if sets.is_empty() {
        return Err("no --data folder holds 1.12.1 archives".into());
    }
    Ok(sets)
}

fn locales_json(sets: &[DataSet]) -> String {
    serde_json::json!({
        "format": 1,
        "default": sets[0].locale,
        "locales": sets.iter().map(|s| serde_json::json!({
            "locale": s.locale,
            "path": format!("{}/", s.locale),
            "bytes": s.manifest.total_bytes(),
            "addons": s.addons.count,
        })).collect::<Vec<_>>(),
    })
    .to_string()
}

fn serve(
    data: &[String],
    addons: &[String],
    bind: &str,
    token: Option<String>,
    locale: &str,
    cache_dir: Option<&Path>,
) -> Result<(), String> {
    let token = token.filter(|t| !t.trim().is_empty());
    let sets = load(data, addons, locale, cache_dir)?;
    let host = Arc::new(Host {
        locales_json: locales_json(&sets),
        sets,
        token,
    });
    let listener = TcpListener::bind(bind).map_err(|e| format!("cannot listen on {bind}: {e}"))?;
    for s in &host.sets {
        eprintln!(
            "inilla-datahost: {} — {} archives ({:.1} GB) from {}, {} addon(s)",
            s.locale,
            s.manifest.files.len(),
            s.manifest.total_bytes() as f64 / 1e9,
            s.dir.display(),
            s.addons.count
        );
    }
    eprintln!(
        "inilla-datahost: serving on http://{bind}/  {}",
        if host.token.is_some() {
            "(token required)"
        } else {
            "(open: anyone who can reach this port can download)"
        }
    );
    http::serve(listener, move |req, stream| handle(&host, req, stream));
    Ok(())
}

fn handle(host: &Host, req: Request, w: &mut TcpStream) {
    let authorized = match &host.token {
        None => true,
        Some(t) => req.token().is_some_and(|given| http::token_eq(given, t)),
    };
    let head_only = req.method == "HEAD";
    if req.method != "GET" && !head_only {
        let _ = http::respond_text(w, 405, "GET or HEAD", false);
        return;
    }
    if req.path == "/" {
        let _ = http::respond_text(
            w,
            200,
            "inilla-datahost: enter this address in iNilla → Game data.\n",
            head_only,
        );
        return;
    }
    if !authorized {
        let _ = http::respond_text(w, 401, "token required", head_only);
        return;
    }
    if req.path == "/locales.json" {
        let _ = http::respond(
            w,
            200,
            "application/json",
            host.locales_json.as_bytes(),
            head_only,
        );
        return;
    }
    let (set, path) = host.route(&req.path);
    match path {
        "/manifest.json" => {
            eprintln!("{} {} manifest", req.peer, set.locale);
            let _ = http::respond(
                w,
                200,
                "application/json",
                set.manifest_json.as_bytes(),
                head_only,
            );
        }
        "/addons.json" => {
            eprintln!("{} {} addons", req.peer, set.locale);
            let _ = http::respond(
                w,
                200,
                "application/json",
                set.addons.manifest_json.as_bytes(),
                head_only,
            );
        }
        p if p.starts_with("/files/") => {
            let name = &p["/files/".len()..];
            // Only what the manifest lists — never a path into the rest of the disk.
            let Some(file) = set.manifest.files.iter().find(|f| f.name == name) else {
                let _ = http::respond_text(w, 404, "not an archive this host serves", head_only);
                return;
            };
            send_file(&set.dir.join(&file.name), file.size, &req, head_only, w);
        }
        p if p.starts_with("/addons/") => {
            let rel = &p["/addons/".len()..];
            let Some((path, size)) = set.addons.files.get(rel) else {
                let _ = http::respond_text(w, 404, "not an addon file this host serves", head_only);
                return;
            };
            send_file(path, *size, &req, head_only, w);
        }
        _ => {
            let _ = http::respond_text(w, 404, "not found", head_only);
        }
    }
}

fn send_file(path: &Path, size: u64, req: &Request, head_only: bool, w: &mut TcpStream) {
    let Ok(mut f) = fs::File::open(path) else {
        let _ = http::respond_text(w, 500, "the file vanished", head_only);
        return;
    };
    let range = req.header("range").map(|r| http::parse_range(r, size));
    let (status, start, len, mut extra) = match range {
        Some(Some((a, b))) => (
            206,
            a,
            b - a + 1,
            vec![("Content-Range", format!("bytes {a}-{b}/{size}"))],
        ),
        Some(None) => {
            let _ = http::write_head(
                w,
                &Head {
                    status: 416,
                    content_type: "text/plain",
                    length: Some(0),
                    extra: &[("Content-Range", format!("bytes */{size}"))],
                },
            );
            return;
        }
        None => (200, 0, size, Vec::new()),
    };
    extra.push(("Accept-Ranges", "bytes".to_string()));
    let _ = http::write_head(
        w,
        &Head {
            status,
            content_type: "application/octet-stream",
            length: Some(len),
            extra: &extra,
        },
    );
    if head_only {
        return;
    }
    if start > 0 && f.seek(SeekFrom::Start(start)).is_err() {
        return;
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    // Addon files are many and small; only the archives are worth a line each.
    let loud = len > 1_000_000;
    if loud {
        let from = if start > 0 {
            format!(" (from {start})")
        } else {
            String::new()
        };
        eprintln!("{} {name} {len}{from}", req.peer);
    }
    match http::copy_n(&mut f, w, len) {
        Ok(sent) if sent == len => {
            if loud {
                eprintln!("{} {name} done", req.peer);
            }
        }
        Ok(sent) => eprintln!("{} {name} stopped at {sent} of {len}", req.peer),
        Err(e) => eprintln!("{} {name} dropped: {e}", req.peer),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::tests::{fill, tmp};
    use crate::manifest::REQUIRED;
    use std::io::{Read, Write};

    fn data_dir(name: &str) -> PathBuf {
        let dir = tmp(name);
        fill(&dir, REQUIRED);
        fill(&dir, &["patch-2.MPQ", "base.MPQ", "realmlist.wtf"]);
        dir
    }

    /// A live host on an ephemeral port.
    fn host_with(data: &[String], addons: &[String], token: Option<&str>) -> std::net::SocketAddr {
        let sets = load(data, addons, "enUS", None).unwrap();
        let host = Arc::new(Host {
            locales_json: locales_json(&sets),
            sets,
            token: token.map(str::to_string),
        });
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || http::serve(listener, move |r, s| handle(&host, r, s)));
        addr
    }

    fn host(name: &str, token: Option<&str>) -> (std::net::SocketAddr, PathBuf) {
        let dir = data_dir(name);
        (host_with(&[dir.display().to_string()], &[], token), dir)
    }

    /// One raw request → (status, headers, body).
    fn get(addr: std::net::SocketAddr, head: &str) -> (u16, String, Vec<u8>) {
        let mut s = TcpStream::connect(addr).unwrap();
        s.write_all(format!("{head}\r\n\r\n").as_bytes()).unwrap();
        let mut raw = Vec::new();
        s.read_to_end(&mut raw).unwrap();
        let split = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        let headers = String::from_utf8_lossy(&raw[..split]).into_owned();
        let status = headers[9..12].parse().unwrap();
        (status, headers, raw[split + 4..].to_vec())
    }

    #[test]
    fn locale_prefixes_are_four_letters() {
        assert_eq!(split_locale("deDE=/x"), (Some("deDE"), "/x"));
        assert_eq!(split_locale("/games/Data"), (None, "/games/Data"));
        assert_eq!(split_locale("C:=x"), (None, "C:=x"));
        assert_eq!(split_locale("/a=b/Data"), (None, "/a=b/Data"));
    }

    #[test]
    fn with_a_token_only_the_hint_is_open() {
        let (addr, _) = host("auth", Some("s3cret"));
        assert_eq!(get(addr, "GET / HTTP/1.1").0, 200);
        assert_eq!(get(addr, "GET /manifest.json HTTP/1.1").0, 401);
        assert_eq!(get(addr, "GET /locales.json HTTP/1.1").0, 401);
        assert_eq!(get(addr, "GET /files/dbc.MPQ HTTP/1.1").0, 401);
        assert_eq!(get(addr, "GET /manifest.json?token=wrong HTTP/1.1").0, 401);
        let (status, _, body) = get(
            addr,
            "GET /manifest.json HTTP/1.1\r\nAuthorization: Bearer s3cret",
        );
        assert_eq!(status, 200);
        let m: Manifest = serde_json::from_slice(&body).unwrap();
        assert_eq!((m.format, m.build), (1, 5875));
        assert_eq!(
            m.files.len(),
            REQUIRED.len() + 1,
            "the required archives and patch-2"
        );
        assert!(m
            .files
            .iter()
            .all(|f| f.name != "base.MPQ" && f.name != "realmlist.wtf"));
        assert_eq!(get(addr, "GET /files/dbc.MPQ?token=s3cret HTTP/1.1").0, 200);
    }

    #[test]
    fn without_a_token_the_host_is_open() {
        let (addr, _) = host("open", None);
        assert_eq!(get(addr, "GET /manifest.json HTTP/1.1").0, 200);
        assert_eq!(get(addr, "GET /files/dbc.MPQ HTTP/1.1").0, 200);
    }

    #[test]
    fn a_download_resumes_with_range() {
        let (addr, dir) = host("range", None);
        let whole = fs::read(dir.join("dbc.MPQ")).unwrap();
        let (status, headers, body) = get(addr, "GET /files/dbc.MPQ HTTP/1.1");
        assert_eq!(status, 200);
        assert!(headers.contains("Accept-Ranges: bytes"));
        assert_eq!(body, whole);
        let (status, headers, body) = get(addr, "GET /files/dbc.MPQ HTTP/1.1\r\nRange: bytes=4-");
        assert_eq!(status, 206);
        assert!(headers.contains(&format!(
            "Content-Range: bytes 4-{}/{}",
            whole.len() - 1,
            whole.len()
        )));
        assert_eq!(body, &whole[4..]);
        let (status, _, _) = get(addr, "GET /files/dbc.MPQ HTTP/1.1\r\nRange: bytes=999-");
        assert_eq!(status, 416);
        let (status, headers, body) = get(addr, "HEAD /files/dbc.MPQ HTTP/1.1");
        assert_eq!(status, 200);
        assert!(headers.contains(&format!("Content-Length: {}", whole.len())));
        assert!(body.is_empty());
    }

    #[test]
    fn nothing_outside_the_manifests_is_served() {
        let (addr, _) = host("escape", None);
        assert_eq!(get(addr, "GET /files/realmlist.wtf HTTP/1.1").0, 404);
        assert_eq!(get(addr, "GET /files/base.MPQ HTTP/1.1").0, 404);
        assert_eq!(
            get(addr, "GET /files/..%2F..%2Fetc%2Fpasswd HTTP/1.1").0,
            404
        );
        assert_eq!(
            get(addr, "GET /files/.inilla-manifest-enUS.json HTTP/1.1").0,
            404
        );
        assert_eq!(get(addr, "GET /addons/..%2Fdbc.MPQ HTTP/1.1").0, 404);
        assert_eq!(get(addr, "POST /manifest.json HTTP/1.1").0, 405);
    }

    #[test]
    fn two_languages_and_their_addons() {
        let en = data_dir("lang-en");
        let de = data_dir("lang-de");
        let missing = tmp("lang-fr-empty");
        let all = tmp("addons-all");
        fs::create_dir_all(all.join("Util")).unwrap();
        fs::write(all.join("Util/Util.toc"), "## Version: 1\n").unwrap();
        let de_addons = tmp("addons-de");
        fs::create_dir_all(de_addons.join("pfQuest/db/deDE")).unwrap();
        fs::write(de_addons.join("pfQuest/pfQuest.toc"), "## Version: 7.0.1\n").unwrap();
        fs::write(de_addons.join("pfQuest/db/deDE/quests.lua"), "Quests").unwrap();
        let addr = host_with(
            &[
                en.display().to_string(),
                format!("deDE={}", de.display()),
                format!("frFR={}", missing.display()),
            ],
            &[
                all.display().to_string(),
                format!("deDE={}", de_addons.display()),
            ],
            None,
        );
        let (_, _, body) = get(addr, "GET /locales.json HTTP/1.1");
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["default"], "enUS");
        let locales: Vec<&str> = v["locales"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| l["locale"].as_str().unwrap())
            .collect();
        assert_eq!(locales, ["enUS", "deDE"], "an empty folder is not offered");
        let (_, _, body) = get(addr, "GET /deDE/manifest.json HTTP/1.1");
        let m: Manifest = serde_json::from_slice(&body).unwrap();
        assert_eq!(m.locale.as_deref(), Some("deDE"));
        assert_eq!(get(addr, "GET /dede/files/dbc.MPQ HTTP/1.1").0, 200);
        let (_, _, body) = get(addr, "GET /deDE/addons.json HTTP/1.1");
        let a: addons::AddonManifest = serde_json::from_slice(&body).unwrap();
        let names: Vec<&str> = a.addons.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["pfQuest", "Util"]);
        let (status, _, body) = get(addr, "GET /deDE/addons/pfQuest/db/deDE/quests.lua HTTP/1.1");
        assert_eq!((status, body.as_slice()), (200, b"Quests".as_slice()));
        let (_, _, body) = get(addr, "GET /addons.json HTTP/1.1");
        let a: addons::AddonManifest = serde_json::from_slice(&body).unwrap();
        assert_eq!(a.addons.len(), 1, "English gets only the shared addons");
        assert_eq!(get(addr, "GET /addons/pfQuest/pfQuest.toc HTTP/1.1").0, 404);
    }

    #[test]
    fn the_manifest_cache_is_reused_until_the_data_changes() {
        let dir = tmp("cache");
        let cache = tmp("cache-dir");
        fill(&dir, REQUIRED);
        let first = manifest(&dir, "enUS", Some(&cache)).unwrap();
        assert!(cache.join(".inilla-manifest-enUS.json").exists());
        assert_eq!(manifest(&dir, "enUS", Some(&cache)).unwrap(), first);
        fs::write(dir.join("dbc.MPQ"), "changed, and longer than before").unwrap();
        let second = manifest(&dir, "enUS", Some(&cache)).unwrap();
        assert_ne!(second, first);
        assert_eq!(
            manifest(&dir, "deDE", Some(&cache))
                .unwrap()
                .locale
                .as_deref(),
            Some("deDE")
        );
    }
}
