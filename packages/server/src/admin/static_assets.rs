// Author: Darshankumar Joshi
//
// Static asset serving for the embedded admin dashboard.
//
// The admin SPA (Vite build output at `packages/admin/dist/`) is embedded
// into the binary at compile time via [`include_dir!`]. At runtime, the
// [`serve_admin_static`] handler resolves request paths against the embedded
// directory, falling back to `index.html` for unknown paths so SPA client-
// side routing (React Router) keeps working on deep links.
//
// Routes are mounted by [`admin_router`]:
//   - `GET /admin`              → 308 → `/admin/`
//   - `GET /admin/`             → serves `index.html`
//   - `GET /admin/{*path}`      → serves the matching embedded file or
//                                 falls back to `index.html` for SPA routes.
//
// MIME types are inferred from the file extension via `mime_guess`, with a
// safe `application/octet-stream` default for unknown extensions.

use axum::{
    Router,
    extract::Path,
    http::{StatusCode, header},
    response::{IntoResponse, Redirect, Response},
    routing::get,
};
use include_dir::{Dir, include_dir};

/// Embedded admin dashboard build output.
///
/// `include_dir!` walks `packages/admin/dist/` at compile time and bakes
/// every file into the binary as `&'static [u8]`. The path is resolved
/// relative to `CARGO_MANIFEST_DIR` so this works whether the workspace is
/// built from the repo root, from inside `packages/server/`, or from a
/// container build context.
///
/// **Build-time requirement:** `packages/admin/dist/` MUST exist when
/// `cargo build -p ddb-server` runs. The repo ships a minimal stub
/// `index.html` (gitignore exception) so the crate compiles even before
/// `npm run build` has run. CI and the Dockerfile both run the admin
/// build before the rust build to embed the real dashboard.
static ADMIN_DIST: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../admin/dist");

/// Build the admin router that serves the embedded dashboard.
///
/// Mount this at the **top level** (NOT under `/api`) so the existing
/// `/api/admin/{schema,functions,…}` JSON endpoints — which live inside
/// `build_router` — remain reachable. Putting a wildcard `/admin/{*path}`
/// route inside `build_router` would shadow those API endpoints.
pub fn admin_router() -> Router {
    Router::new()
        .route("/admin", get(redirect_to_index))
        .route("/admin/", get(serve_index))
        .route("/admin/{*path}", get(serve_admin_static))
}

/// Redirect bare `/admin` to `/admin/` so relative asset paths in
/// `index.html` resolve correctly. Uses 308 (Permanent Redirect) which
/// preserves the GET method.
async fn redirect_to_index() -> Redirect {
    Redirect::permanent("/admin/")
}

/// Serve the SPA shell at `/admin/`.
async fn serve_index() -> Response {
    serve_file_or_index("index.html")
}

/// Serve a static file from the embedded admin dist.
///
/// Behaviour:
/// 1. Look up the requested path inside `ADMIN_DIST`.
/// 2. If the file exists, serve it with a `Content-Type` derived from its
///    extension.
/// 3. If the file does not exist, fall back to `index.html` so the SPA can
///    handle the route client-side. This is the standard "SPA fallback"
///    pattern — it makes deep links like `/admin/tables/users` work after
///    a hard refresh.
/// 4. If even `index.html` is missing (build failure / dev stub), return
///    `404 Not Found`.
pub async fn serve_admin_static(Path(path): Path<String>) -> Response {
    serve_file_or_index(&path)
}

/// Resolve `path` against the embedded directory, with `index.html` SPA
/// fallback and `Content-Type` inference.
fn serve_file_or_index(path: &str) -> Response {
    // Normalise: strip leading slash and reject directory traversal. Even
    // though `include_dir` is read-only at runtime, refusing `..` keeps
    // the surface small and the error messages tidy.
    let trimmed = path.trim_start_matches('/');
    if trimmed.contains("..") {
        return StatusCode::BAD_REQUEST.into_response();
    }

    // Try the literal path first; if missing, fall back to index.html so
    // client-side routing works on refresh / deep links.
    let file = ADMIN_DIST
        .get_file(trimmed)
        .or_else(|| ADMIN_DIST.get_file("index.html"));

    match file {
        Some(f) => {
            let mime = mime_guess::from_path(f.path()).first_or_octet_stream();
            (
                [(header::CONTENT_TYPE, mime.as_ref().to_string())],
                f.contents(),
            )
                .into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The embedded dist directory must contain at least an `index.html`,
    /// even in a fresh checkout where `npm run build` has not run yet. The
    /// committed stub guarantees this.
    #[test]
    fn embedded_dist_has_index_html() {
        assert!(
            ADMIN_DIST.get_file("index.html").is_some(),
            "packages/admin/dist/index.html missing — commit the stub or run `npm run build` in packages/admin/"
        );
    }

    /// Unknown paths must SPA-fallback to `index.html` (never 404 for
    /// route-shaped requests). This is what makes deep-link refresh work.
    #[tokio::test]
    async fn unknown_path_falls_back_to_index() {
        let resp = serve_admin_static(Path("tables/users/edit".to_string())).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// Directory traversal attempts must be rejected.
    #[tokio::test]
    async fn rejects_directory_traversal() {
        let resp = serve_admin_static(Path("../Cargo.toml".to_string())).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
}
