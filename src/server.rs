use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{LinkReader, LinkWriter, ListParams, SqliteStore};

// MARK: JSON response types

#[derive(Serialize)]
struct LinkJson {
    url: String,
    title: Option<String>,
    via: Option<crate::Via>,
    tags: Vec<String>,
    notes: Option<String>,
    found_at: Option<DateTime<Utc>>,
    read_at: Option<DateTime<Utc>>,
    published_at: Option<DateTime<Utc>>,
    from_filename: Option<String>,
    image: Option<String>,
    hidden: bool,
    meta: Option<std::collections::HashMap<String, Vec<String>>>,
}

impl From<crate::Link> for LinkJson {
    fn from(link: crate::Link) -> Self {
        Self {
            url: link.url().to_owned(),
            title: link.title().map(|s| s.to_owned()),
            via: link.via().cloned(),
            tags: link.tags().clone(),
            notes: link.notes().map(|s| s.to_owned()),
            found_at: link.found_at(),
            read_at: link.read_at(),
            published_at: link.published_at(),
            from_filename: link.from_filename().map(|s| s.to_owned()),
            image: link.image().map(|s| s.to_owned()),
            hidden: link.hidden(),
            meta: link.meta().cloned(),
        }
    }
}

#[derive(Serialize)]
struct LinkListResponse {
    links: Vec<LinkJson>,
    total: i64,
    page: i64,
    per_page: i64,
}

// MARK: Query/body types

#[derive(Deserialize)]
struct LinkListQuery {
    q: Option<String>,
    tag: Option<String>,
    hidden: Option<bool>,
    page: Option<i64>,
    per_page: Option<i64>,
}

#[derive(Deserialize)]
struct LinkPatch {
    title: Option<String>,
    notes: Option<String>,
    tags: Option<Vec<String>>,
    via: Option<crate::Via>,
    hidden: Option<bool>,
}

// MARK: Handlers

async fn list_links(
    State(store): State<Arc<SqliteStore>>,
    Query(params): Query<LinkListQuery>,
) -> impl IntoResponse {
    let page = params.page.unwrap_or(1).max(1);
    let per_page = params.per_page.unwrap_or(50).clamp(1, 200);
    let offset = (page - 1) * per_page;

    let list_params = ListParams {
        query: params.q,
        tag: params.tag,
        hidden: params.hidden,
        offset,
        limit: per_page,
    };

    let total = match store.count(&list_params).await {
        Ok(t) => t,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    let links = match store.list(&list_params).await {
        Ok(l) => l,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    Json(LinkListResponse {
        links: links.into_iter().map(LinkJson::from).collect(),
        total,
        page,
        per_page,
    })
    .into_response()
}

async fn get_link(
    State(store): State<Arc<SqliteStore>>,
    Path(url): Path<String>,
) -> impl IntoResponse {
    eprintln!("uhhh");
    let decoded = urlencoding::decode(&url)
        .map(|s| s.into_owned())
        .unwrap_or(url);

    match store.get(&decoded).await {
        Ok(Some(link)) => Json(LinkJson::from(link)).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn patch_link(
    State(store): State<Arc<SqliteStore>>,
    Path(url): Path<String>,
    Json(patch): Json<LinkPatch>,
) -> impl IntoResponse {
    let decoded = urlencoding::decode(&url)
        .map(|s| s.into_owned())
        .unwrap_or(url);

    let mut link = match store.get(&decoded).await {
        Ok(Some(link)) => link,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    if let Some(title) = patch.title {
        link.title = if title.is_empty() { None } else { Some(title) };
    }
    if let Some(notes) = patch.notes {
        *link.notes_mut() = if notes.is_empty() { None } else { Some(notes) };
        // Setting notes marks as read.
        if link.read_at().is_none() {
            *link.read_at_mut() = Some(Utc::now());
        }
    }
    if let Some(tags) = patch.tags {
        *link.tags_mut() = tags;
    }
    if let Some(via) = patch.via {
        *link.via_mut() = Some(via);
    }
    if let Some(hidden) = patch.hidden {
        *link.hidden_mut() = hidden;
    }

    match store.write(link).await {
        Ok(_) => match store.get(&decoded).await {
            Ok(Some(link)) => Json(LinkJson::from(link)).into_response(),
            Ok(None) => StatusCode::NOT_FOUND.into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        },
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn list_tags(State(store): State<Arc<SqliteStore>>) -> impl IntoResponse {
    match store.all_tags().await {
        Ok(tags) => Json(tags).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// MARK: Router

pub fn router(store: Arc<SqliteStore>) -> Router {
    let api = Router::new()
        .route("/api/links", get(list_links))
        .route("/api/links/{url}", get(get_link).patch(patch_link))
        .route("/api/tags", get(list_tags))
        .with_state(store);

    // Serve the Svelte UI from ui/dist if it exists.
    let ui_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ui/dist");
    if ui_dir.is_dir() {
        api.fallback_service(tower_http::services::ServeDir::new(&ui_dir).fallback(
            tower_http::services::ServeFile::new(ui_dir.join("index.html")),
        ))
    } else {
        api
    }
}

/// Starts the server on the given port.
pub async fn serve(store: Arc<SqliteStore>, port: u16) -> eyre::Result<()> {
    let app = router(store);
    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{}", port)).await?;
    eprintln!("listening on http://127.0.0.1:{}", port);
    axum::serve(listener, app).await?;
    Ok(())
}
