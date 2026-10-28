//! The web UI compiled into the binary, for packaged builds where there is
//! no `ui/dist` folder beside the executable. Only built with the
//! `embed-ui` feature, which needs `ui/dist` to exist at compile time (run
//! `npm run build` in `ui/` first); a plain `cargo build` never needs Node.

use axum::body::Body;
use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};

#[derive(rust_embed::RustEmbed)]
#[folder = "../../ui/dist"]
struct Assets;

/// Serves an embedded file, falling back to `index.html` for any path that
/// isn't one (client-side routes), exactly like the on-disk server does.
pub async fn serve(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    match Assets::get(path).or_else(|| Assets::get("index.html")) {
        Some(file) => {
            let mime = file.metadata.mimetype().to_string();
            ([(header::CONTENT_TYPE, mime)], Body::from(file.data.into_owned())).into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

pub fn available() -> bool {
    Assets::get("index.html").is_some()
}
