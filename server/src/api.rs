use crate::{
    catalog::Catalog,
    models::{
        CatalogIssue, File, Game, GamePage, Health, IssuePage, Manifest, ReloadPhase, ReloadStatus,
        System, SystemSummary,
    },
    paths::{regular_file, reject_symlink_root, relative_path, source_file, Signature},
    reload::Reload,
};
use axum::{
    body::Body,
    extract::{
        rejection::{PathRejection, QueryRejection},
        Path, Query, State,
    },
    http::{header, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path as FsPath;
use tokio::io::AsyncReadExt;
use tokio_util::io::ReaderStream;
use utoipa::OpenApi;

#[derive(Clone)]
struct AppState {
    catalog: Catalog,
    reload: Reload,
}

pub fn router(catalog: Catalog) -> Router {
    router_with_reload(catalog, Reload::default())
}

/// Share one indexation manager between startup, API requests and the management UI.
pub fn router_with_reload(catalog: Catalog, reload: Reload) -> Router {
    Router::new()
        .route("/", get(|| async { Html(include_str!("admin.html")) }))
        .route("/api/v1/health", get(health))
        .route("/api/v1/systems", get(systems))
        .route("/api/v1/catalog/summary", get(catalog_summary))
        .route("/api/v1/catalog/issues", get(catalog_issues))
        .route("/api/v1/games", get(games))
        .route("/api/v1/games/fields", get(filter_fields))
        .route("/api/v1/games/{game_id}", get(detail))
        .route("/api/v1/games/{game_id}/manifest", get(manifest))
        .route("/api/v1/files/{file_id}", get(download))
        .route(
            "/api/v1/catalog/reload",
            post(reload_catalog).get(reload_status),
        )
        .route("/openapi.json", get(|| async { Json(openapi()) }))
        .route("/docs", get(|| async { Html(include_str!("docs.html")) }))
        .fallback(|| async { ApiError(StatusCode::NOT_FOUND, "Not Found".into()) })
        .method_not_allowed_fallback(|| async {
            ApiError(
                StatusCode::METHOD_NOT_ALLOWED,
                "Méthode non autorisée".into(),
            )
        })
        .with_state(AppState { catalog, reload })
}

pub struct ApiError(StatusCode, String);
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error":{"message": self.1}}))).into_response()
    }
}
fn internal(error: impl std::fmt::Display) -> ApiError {
    tracing::error!("Erreur interne : {error}");
    ApiError(
        StatusCode::INTERNAL_SERVER_ERROR,
        "Erreur interne du serveur".into(),
    )
}
async fn blocking<T: Send + 'static>(
    task: impl FnOnce() -> Result<T, ApiError> + Send + 'static,
) -> Result<T, ApiError> {
    tokio::task::spawn_blocking(task).await.map_err(internal)?
}
fn find_game(catalog: &Catalog, id: &str) -> Result<Game, ApiError> {
    catalog
        .get("games", id)
        .map_err(internal)?
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "Jeu introuvable".into()))
}

fn path_id(path: Result<Path<String>, PathRejection>) -> Result<String, ApiError> {
    path.map(|Path(id)| id).map_err(|_| {
        ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Paramètres invalides".into(),
        )
    })
}

#[utoipa::path(get, path="/api/v1/health", responses((status=200, body=Health)))]
async fn health() -> Json<Health> {
    Json(Health {
        status: "ok".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        manifest_version: 1,
    })
}

#[utoipa::path(get, path="/api/v1/systems", responses((status=200, body=Vec<System>)))]
async fn systems(State(state): State<AppState>) -> Result<Json<Vec<System>>, ApiError> {
    Ok(Json(
        blocking(move || state.catalog.systems().map_err(internal)).await?,
    ))
}

#[utoipa::path(get, path="/api/v1/catalog/summary", responses((status=200, body=Vec<SystemSummary>)))]
async fn catalog_summary(
    State(state): State<AppState>,
) -> Result<Json<Vec<SystemSummary>>, ApiError> {
    Ok(Json(
        blocking(move || state.catalog.summary().map_err(internal)).await?,
    ))
}

#[derive(Deserialize)]
struct IssueQuery {
    system: Option<String>,
    #[serde(default)]
    offset: u64,
    #[serde(default = "default_limit")]
    limit: u64,
}

#[utoipa::path(get, path="/api/v1/catalog/issues", params(("system"=Option<String>, Query), ("offset"=Option<u64>, Query, minimum=0), ("limit"=Option<u64>, Query, minimum=1, maximum=200)), responses((status=200, body=IssuePage), (status=422, description="Paramètres invalides")))]
async fn catalog_issues(
    State(state): State<AppState>,
    query: Result<Query<IssueQuery>, QueryRejection>,
) -> Result<Json<IssuePage>, ApiError> {
    let Query(query) = query.map_err(|_| {
        ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Paramètres invalides".into(),
        )
    })?;
    if !(1..=200).contains(&query.limit) || query.offset > i64::MAX as u64 {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Paramètres invalides".into(),
        ));
    }
    Ok(Json(
        blocking(move || {
            state
                .catalog
                .issues(query.system.as_deref(), query.offset, query.limit)
                .map_err(internal)
        })
        .await?,
    ))
}

#[derive(Deserialize)]
struct GameQuery {
    system: Option<String>,
    #[serde(default)]
    search: String,
    #[serde(default)]
    filters: String,
    #[serde(default)]
    offset: u64,
    #[serde(default = "default_limit")]
    limit: u64,
}
fn default_limit() -> u64 {
    50
}

#[utoipa::path(get, path="/api/v1/games", params(("system"=Option<String>, Query), ("search"=Option<String>, Query, max_length=256), ("filters"=Option<String>, Query, description="Objet JSON champ: critère ; texte, =exact, >=min, <=max ; year extrait releasedate ; players accepte les plages"), ("offset"=Option<u64>, Query, minimum=0), ("limit"=Option<u64>, Query, minimum=1, maximum=200)), responses((status=200, body=GamePage), (status=422, description="Paramètres invalides")))]
async fn games(
    State(state): State<AppState>,
    query: Result<Query<GameQuery>, QueryRejection>,
) -> Result<Json<GamePage>, ApiError> {
    let Query(query) = query.map_err(|_| {
        ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Paramètres invalides".into(),
        )
    })?;
    if !(1..=200).contains(&query.limit)
        || query.offset > i64::MAX as u64
        || query.search.chars().count() > 256
        || query.filters.len() > 16384
    {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Paramètres invalides".into(),
        ));
    }
    let filters: crate::filters::Filters = if query.filters.is_empty() {
        Default::default()
    } else {
        serde_json::from_str(&query.filters).map_err(|_| {
            ApiError(
                StatusCode::UNPROCESSABLE_ENTITY,
                "Filtres JSON invalides".into(),
            )
        })?
    };
    crate::filters::validate(&filters)
        .map_err(|error| ApiError(StatusCode::UNPROCESSABLE_ENTITY, error.to_string()))?;
    Ok(Json(
        blocking(move || {
            state
                .catalog
                .filtered_page(
                    query.system.as_deref(),
                    &query.search,
                    query.offset,
                    query.limit,
                    &filters,
                )
                .map_err(internal)
        })
        .await?,
    ))
}

#[utoipa::path(get, path="/api/v1/games/fields", responses((status=200, body=Vec<String>)))]
async fn filter_fields(State(state): State<AppState>) -> Result<Json<Vec<String>>, ApiError> {
    Ok(Json(
        blocking(move || state.catalog.filter_fields().map_err(internal)).await?,
    ))
}

#[utoipa::path(get, path="/api/v1/games/{game_id}", params(("game_id"=String, Path)), responses((status=200, body=Game), (status=404, description="Jeu introuvable")))]
async fn detail(
    State(state): State<AppState>,
    id: Result<Path<String>, PathRejection>,
) -> Result<Json<Game>, ApiError> {
    let id = path_id(id)?;
    Ok(Json(
        blocking(move || find_game(&state.catalog, &id)).await?,
    ))
}

#[utoipa::path(get, path="/api/v1/games/{game_id}/manifest", params(("game_id"=String, Path)), responses((status=200, body=Manifest), (status=404, description="Jeu introuvable"), (status=409, description="Jeu non installable")))]
async fn manifest(
    State(state): State<AppState>,
    id: Result<Path<String>, PathRejection>,
) -> Result<Json<Manifest>, ApiError> {
    let id = path_id(id)?;
    Ok(Json(
        blocking(move || {
            let game = find_game(&state.catalog, &id)?;
            if !game.installable {
                return Err(ApiError(
                    StatusCode::CONFLICT,
                    game.unavailable_reason
                        .unwrap_or_else(|| "Format non pris en charge".into()),
                ));
            }
            Ok(Manifest {
                version: 1,
                game_id: game.id,
                system: game.system,
                path: game.path,
                revision: game.revision,
                metadata: game.metadata,
                files: game.files,
            })
        })
        .await?,
    ))
}

#[utoipa::path(post, path="/api/v1/catalog/reload", responses((status=202, body=ReloadStatus)))]
async fn reload_catalog(State(state): State<AppState>) -> (StatusCode, Json<ReloadStatus>) {
    (
        StatusCode::ACCEPTED,
        Json(state.reload.start(state.catalog)),
    )
}

#[utoipa::path(get, path="/api/v1/catalog/reload", responses((status=200, body=ReloadStatus)))]
async fn reload_status(State(state): State<AppState>) -> Json<ReloadStatus> {
    Json(state.reload.status())
}

#[utoipa::path(get, path="/api/v1/files/{file_id}", params(("file_id"=String, Path), ("If-Match"=Option<String>, Header)), responses((status=200, description="Flux binaire vérifié, Content-Length et ETag"), (status=404, description="Fichier introuvable"), (status=409, description="Source modifiée"), (status=412, description="Révision différente")))]
async fn download(
    State(state): State<AppState>,
    id: Result<Path<String>, PathRejection>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let id = path_id(id)?;
    let (stream, size, etag) = blocking(move || {
        let (file, icon) = state
            .catalog
            .download(&id)
            .map_err(internal)?
            .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "Fichier introuvable".into()))?;
        let etag = format!("\"{}\"", file.sha256);
        if headers
            .get(header::IF_MATCH)
            .is_some_and(|value| value.as_bytes() != etag.as_bytes())
        {
            return Err(ApiError(
                StatusCode::PRECONDITION_FAILED,
                "Révision du fichier différente".into(),
            ));
        }
        let open = || -> anyhow::Result<std::fs::File> {
            let root = if icon {
                state.catalog.config.system_icon.clone()
            } else {
                if relative_path(&file.system)? != file.system
                    || FsPath::new(&file.system).components().count() != 1
                {
                    anyhow::bail!("Système invalide");
                }
                state.catalog.config.library.join(&file.system)
            };
            reject_symlink_root(&root)?;
            let (_, path) = source_file(&root, &file.relative)?;
            let stream = regular_file(&path)?;
            if Signature::of(&stream.metadata()?) != file.signature || file.size != file.signature.2
            {
                anyhow::bail!("Fichier modifié");
            }
            Ok(stream)
        };
        let stream = open().map_err(|error| {
            tracing::debug!("Source indisponible : {error:#}");
            ApiError(
                StatusCode::CONFLICT,
                if icon {
                    "Image système modifiée ; actualiser les systèmes"
                } else {
                    "Bibliothèque modifiée ; réindexation nécessaire"
                }
                .into(),
            )
        })?;
        Ok((stream, file.size, etag))
    })
    .await?;
    let stream = tokio::fs::File::from_std(stream).take(size);
    Response::builder()
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::CONTENT_LENGTH, size)
        .header(header::ETAG, etag)
        .body(Body::from_stream(ReaderStream::with_capacity(
            stream,
            1024 * 1024,
        )))
        .map_err(internal)
}

#[derive(OpenApi)]
#[openapi(
    paths(
        health,
        systems,
        catalog_summary,
        catalog_issues,
        games,
        filter_fields,
        detail,
        manifest,
        reload_catalog,
        reload_status,
        download
    ),
    components(schemas(
        Health,
        System,
        SystemSummary,
        CatalogIssue,
        IssuePage,
        File,
        Game,
        GamePage,
        Manifest,
        ReloadStatus,
        ReloadPhase
    ))
)]
struct ApiDoc;

pub fn openapi() -> Value {
    let mut value = serde_json::to_value(ApiDoc::openapi()).expect("OpenAPI serialization");
    value["components"]["schemas"]["Manifest"]["properties"]["version"]["const"] = json!(1);
    value["components"]["schemas"]["Health"]["properties"]["manifest_version"]["const"] = json!(1);
    value["components"]["schemas"]["Health"]["properties"]["status"]["const"] = json!("ok");
    value
}
