//! **inilla-datahost** — hand your own World of Warcraft 1.12.1 `Data` folder to your own iPhone,
//! once. The iNilla app's setup screen downloads it from here (resumable, SHA-256 verified).
//!
//! ```text
//! inilla-datahost serve --data "/games/WoW 1.12.1/Data" --token s3cret
//!     → http://<this machine>:8099/   (enter it in iNilla's setup, with the token)
//! inilla-datahost manifest --data "/games/WoW 1.12.1/Data"
//!     → prints the manifest (and caches it) without serving
//! ```
//!
//! It serves exactly what the client mounts — the MPQs at the root of `Data/` — and nothing else:
//! `GET /manifest.json` (names, sizes, SHA-256) and `GET|HEAD /files/<name>` with `Range`, so an
//! interrupted download resumes. Hashing 5 GB takes a minute or two; the manifest is cached
//! (`.inilla-manifest.json`, beside the data or in `--cache-dir`) and rebuilt when an archive's
//! size or mtime changes.
//!
//! The files are Blizzard's. Serve them only to yourself and people who own the game, keep a token
//! on, and prefer a LAN or a VPN (Tailscale, WireGuard) to the open internet.

mod http;
mod manifest;

use std::fs;
use std::io::{Seek, SeekFrom};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use clap::{Parser, Subcommand};

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
    /// Serve the Data folder.
    Serve {
        /// The client's `Data` folder.
        #[arg(long, env = "INILLA_DATA_DIR")]
        data: PathBuf,
        /// Address to listen on.
        #[arg(long, env = "INILLA_DATA_BIND", default_value = "0.0.0.0:8099")]
        bind: String,
        /// Require `Authorization: Bearer <token>` (or `?token=`).
        #[arg(long, env = "INILLA_DATA_TOKEN")]
        token: Option<String>,
        /// Serve without a token (a trusted LAN only).
        #[arg(long)]
        no_token: bool,
        /// The client's locale, advertised in the manifest.
        #[arg(long, env = "INILLA_DATA_LOCALE", default_value = "enUS")]
        locale: String,
        /// Where to cache the manifest, when the Data folder is read-only.
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
            bind,
            token,
            no_token,
            locale,
            cache_dir,
        } => serve(&data, &bind, token, no_token, &locale, cache_dir.as_deref()),
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
    let cache = cache_dir.unwrap_or(dir).join(".inilla-manifest.json");
    let fp = fingerprint(dir, locale);
    if let Ok(text) = fs::read_to_string(&cache) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
            if v["fingerprint"].as_str() == Some(fp.as_str()) {
                if let Ok(m) = serde_json::from_value::<Manifest>(v["manifest"].clone()) {
                    eprintln!("inilla-datahost: manifest from cache {}", cache.display());
                    return Ok(m);
                }
            }
        }
    }
    eprintln!(
        "inilla-datahost: hashing {} archives ({:.1} GB)…",
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

struct Host {
    dir: PathBuf,
    manifest: Manifest,
    manifest_json: String,
    token: Option<String>,
}

fn serve(
    dir: &Path,
    bind: &str,
    token: Option<String>,
    no_token: bool,
    locale: &str,
    cache_dir: Option<&Path>,
) -> Result<(), String> {
    let token = token.filter(|t| !t.trim().is_empty());
    if token.is_none() && !no_token {
        return Err(
            "set --token (or $INILLA_DATA_TOKEN), or pass --no-token on a trusted LAN".into(),
        );
    }
    let m = manifest(dir, locale, cache_dir)?;
    let host = Arc::new(Host {
        dir: dir.to_path_buf(),
        manifest_json: serde_json::to_string(&m).map_err(|e| e.to_string())?,
        manifest: m,
        token,
    });
    let listener = TcpListener::bind(bind).map_err(|e| format!("cannot listen on {bind}: {e}"))?;
    eprintln!(
        "inilla-datahost: serving {} archives ({:.1} GB) from {} on http://{bind}/{}",
        host.manifest.files.len(),
        host.manifest.total_bytes() as f64 / 1e9,
        dir.display(),
        if host.token.is_some() {
            "  (token required)"
        } else {
            "  (NO token)"
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
    match req.path.as_str() {
        "/" => {
            let _ = http::respond_text(
                w,
                200,
                "inilla-datahost: enter this address (and the token) in iNilla → Game data.\n",
                head_only,
            );
        }
        "/manifest.json" => {
            if !authorized {
                let _ = http::respond_text(w, 401, "token required", head_only);
                return;
            }
            eprintln!("{} manifest", req.peer);
            let _ = http::respond(
                w,
                200,
                "application/json",
                host.manifest_json.as_bytes(),
                head_only,
            );
        }
        p if p.starts_with("/files/") => {
            if !authorized {
                let _ = http::respond_text(w, 401, "token required", head_only);
                return;
            }
            let name = &p["/files/".len()..];
            // Only what the manifest lists — never a path into the rest of the disk.
            let Some(file) = host.manifest.files.iter().find(|f| f.name == name) else {
                let _ = http::respond_text(w, 404, "not an archive this host serves", head_only);
                return;
            };
            send_file(&host.dir.join(&file.name), file.size, &req, head_only, w);
        }
        _ => {
            let _ = http::respond_text(w, 404, "not found", head_only);
        }
    }
}

fn send_file(path: &Path, size: u64, req: &Request, head_only: bool, w: &mut TcpStream) {
    let Ok(mut f) = fs::File::open(path) else {
        let _ = http::respond_text(w, 500, "the archive vanished", head_only);
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
    let from = if start > 0 {
        format!(" (from {start})")
    } else {
        String::new()
    };
    eprintln!("{} {name} {len}{from}", req.peer);
    match http::copy_n(&mut f, w, len) {
        Ok(sent) if sent == len => eprintln!("{} {name} done", req.peer),
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

    /// A live host on an ephemeral port over a folder of fake archives.
    fn host(name: &str, token: Option<&str>) -> (std::net::SocketAddr, PathBuf) {
        let dir = tmp(name);
        fill(&dir, REQUIRED);
        fill(&dir, &["patch-2.MPQ", "base.MPQ", "realmlist.wtf"]);
        let m = manifest(&dir, "enUS", None).unwrap();
        let host = Arc::new(Host {
            dir: dir.clone(),
            manifest_json: serde_json::to_string(&m).unwrap(),
            manifest: m,
            token: token.map(str::to_string),
        });
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || http::serve(listener, move |r, s| handle(&host, r, s)));
        (addr, dir)
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
    fn the_manifest_and_files_need_the_token() {
        let (addr, _) = host("auth", Some("s3cret"));
        assert_eq!(get(addr, "GET / HTTP/1.1").0, 200);
        assert_eq!(get(addr, "GET /manifest.json HTTP/1.1").0, 401);
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
    fn nothing_outside_the_manifest_is_served() {
        let (addr, _) = host("escape", None);
        assert_eq!(get(addr, "GET /files/realmlist.wtf HTTP/1.1").0, 404);
        assert_eq!(get(addr, "GET /files/base.MPQ HTTP/1.1").0, 404);
        assert_eq!(
            get(addr, "GET /files/..%2F..%2Fetc%2Fpasswd HTTP/1.1").0,
            404
        );
        assert_eq!(
            get(addr, "GET /files/.inilla-manifest.json HTTP/1.1").0,
            404
        );
        assert_eq!(get(addr, "POST /manifest.json HTTP/1.1").0, 405);
    }

    #[test]
    fn the_manifest_cache_is_reused_until_the_data_changes() {
        let dir = tmp("cache");
        let cache = tmp("cache-dir");
        fill(&dir, REQUIRED);
        let first = manifest(&dir, "enUS", Some(&cache)).unwrap();
        assert!(cache.join(".inilla-manifest.json").exists());
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
