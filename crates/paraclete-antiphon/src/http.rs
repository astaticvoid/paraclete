// SPDX-License-Identifier: GPL-3.0-or-later
//! Static file serving for the Theoria web client (`tiny_http`, own thread).
//!
//! Unauthenticated by design: it serves only the client bundle; the token
//! gates the WebSocket session (w0 spec §Auth). W1 adds `StaticSource::Embedded`
//! behind the `embed-ui` cargo feature (`include_dir!`s
//! `web/packages/app/dist` at compile time, wired from `paraclete-app`);
//! `StaticSource::Disk` (W0's behaviour) still serves from a directory on
//! disk and remains available via `--theoria-dir` for dev.

use std::fs;
use std::path::{Path, PathBuf};

#[cfg(feature = "embed-ui")]
use include_dir::Dir;

/// Where the client bundle bytes come from. `Disk` reads the filesystem on
/// every request (dev-friendly: edit-reload with no rebuild); `Embedded`
/// serves bytes baked into the binary at compile time (release builds —
/// one binary, no separate asset directory to ship).
pub enum StaticSource {
    Disk(PathBuf),
    #[cfg(feature = "embed-ui")]
    Embedded(&'static Dir<'static>),
}

fn content_type(path: &str) -> &'static str {
    let ext = Path::new(path).extension().and_then(|e| e.to_str());
    match ext {
        Some("html") => "text/html; charset=utf-8",
        Some("js") | Some("mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("ico") => "image/x-icon",
        Some("webmanifest") => "application/manifest+json",
        _ => "application/octet-stream",
    }
}

/// Resolve a request URL to a relative path under the static root. Returns
/// `None` for traversal attempts. `/` resolves to `index.html`. Shared by
/// both `StaticSource` variants — only how the resolved path is turned into
/// bytes differs.
fn resolve_rel_path(url: &str) -> Option<&str> {
    let path = url.split('?').next().unwrap_or("");
    let path = path.trim_start_matches('/');
    let rel = if path.is_empty() { "index.html" } else { path };
    if rel.split('/').any(|seg| seg == ".." || seg.is_empty() || seg.contains('\\')) {
        return None;
    }
    Some(rel)
}

/// Resolve a request URL to a file under `root`. Returns `None` for traversal
/// attempts or missing files. `/` serves `index.html`.
pub fn resolve_static_path(root: &Path, url: &str) -> Option<PathBuf> {
    let rel = resolve_rel_path(url)?;
    let full = root.join(rel);
    full.is_file().then_some(full)
}

fn respond_disk(root: &Path, url: &str) -> tiny_http::Response<std::io::Cursor<Vec<u8>>> {
    match resolve_static_path(root, url) {
        Some(path) => match fs::read(&path) {
            Ok(body) => {
                let mime = content_type(&path.to_string_lossy());
                let header = tiny_http::Header::from_bytes(&b"Content-Type"[..], mime.as_bytes())
                    .expect("static header");
                tiny_http::Response::from_data(body).with_header(header)
            }
            Err(_) => tiny_http::Response::from_string("500").with_status_code(500),
        },
        None => tiny_http::Response::from_string("404").with_status_code(404),
    }
}

#[cfg(feature = "embed-ui")]
fn respond_embedded(
    dir: &'static Dir<'static>,
    url: &str,
) -> tiny_http::Response<std::io::Cursor<Vec<u8>>> {
    let Some(rel) = resolve_rel_path(url) else {
        return tiny_http::Response::from_string("404").with_status_code(404);
    };
    match dir.get_file(rel) {
        Some(file) => {
            let mime = content_type(rel);
            let header = tiny_http::Header::from_bytes(&b"Content-Type"[..], mime.as_bytes())
                .expect("static header");
            tiny_http::Response::from_data(file.contents().to_vec()).with_header(header)
        }
        None => tiny_http::Response::from_string("404").with_status_code(404),
    }
}

/// Spawn the HTTP thread. Returns an error only if the port cannot be bound.
pub fn spawn_http(source: StaticSource, port: u16) -> std::io::Result<()> {
    let server = tiny_http::Server::http(("0.0.0.0", port))
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::AddrInUse, e.to_string()))?;
    std::thread::Builder::new().name("antiphon-http".into()).spawn(move || {
        for request in server.incoming_requests() {
            let response = match &source {
                StaticSource::Disk(root) => respond_disk(root, request.url()),
                #[cfg(feature = "embed-ui")]
                StaticSource::Embedded(dir) => respond_embedded(dir, request.url()),
            };
            let _ = request.respond(response);
        }
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_rejects_traversal_and_serves_index() {
        let dir = std::env::temp_dir().join("antiphon-http-test");
        let _ = fs::create_dir_all(&dir);
        fs::write(dir.join("index.html"), "<html>").unwrap();
        fs::write(dir.join("theoria.js"), "//").unwrap();

        assert_eq!(resolve_static_path(&dir, "/"), Some(dir.join("index.html")));
        assert_eq!(resolve_static_path(&dir, "/?t=abcd"), Some(dir.join("index.html")));
        assert_eq!(resolve_static_path(&dir, "/theoria.js"), Some(dir.join("theoria.js")));
        assert_eq!(resolve_static_path(&dir, "/../Cargo.toml"), None);
        assert_eq!(resolve_static_path(&dir, "/a/../../secret"), None);
        assert_eq!(resolve_static_path(&dir, "/missing.js"), None);
    }
}
