use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{delete, get};
use axum::{Json, Router, routing::post};
use serde_json::json;
use shared_types::{DBWrite, GlobalSummary, SledTree, Summary};
use sled::{self, Db, Tree};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::time;

#[derive(Clone)]
struct AppState {
    db: Db,
    default_tree: Tree,
    fallback_tree: Tree,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let db = sled::open("app_db")?;
    let default_tree = db.open_tree("default")?;
    let fallback_tree = db.open_tree("fallback")?;

    let app_state = AppState {
        db: db.clone(),
        default_tree: default_tree.clone(),
        fallback_tree: fallback_tree.clone(),
    };

    // Start the periodic flush task
    let flush_state = app_state.clone();
    tokio::spawn(async move {
        periodic_flush(flush_state).await;
    });

    let app = Router::new()
        .route("/payment", post(process_payment))
        .route("/summary", get(get_payments_summary))
        .route("/purge", delete(purge_payments))
        .with_state(app_state);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8888").await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn periodic_flush(state: AppState) {
    let mut interval = time::interval(Duration::from_millis(100));

    loop {
        interval.tick().await;

        // Flush both trees
        if let Err(e) = state.default_tree.flush() {
            eprintln!("Error flushing default tree: {}", e);
        }

        if let Err(e) = state.fallback_tree.flush() {
            eprintln!("Error flushing fallback tree: {}", e);
        }
    }
}

async fn purge_payments(State(state): State<AppState>) -> impl IntoResponse {
    state.db.clear().unwrap();
    StatusCode::OK
}

async fn get_payments_summary(
    Query(params): Query<HashMap<String, String>>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let from = params.get("from").unwrap();
    let to = params.get("to").unwrap();
    let default = Summary::from_iter(state.default_tree.range(from.as_str()..=to.as_str()));
    let fallback = Summary::from_iter(state.fallback_tree.range(from.as_str()..=to.as_str()));
    let global_summary = GlobalSummary { default, fallback };
    Json(global_summary)
}

async fn process_payment(
    State(state): State<AppState>,
    Json(payload): Json<DBWrite>,
) -> impl IntoResponse {
    let bytes = payload.value.to_be_bytes();

    match payload.tree {
        SledTree::Fallback => {
            if let Err(e) = state.fallback_tree.insert(payload.key.as_bytes(), &bytes) {
                eprintln!("Error inserting into fallback tree: {}", e);
                return StatusCode::INTERNAL_SERVER_ERROR;
            }
        }
        SledTree::Default => {
            if let Err(e) = state.default_tree.insert(payload.key.as_bytes(), &bytes) {
                eprintln!("Error inserting into default tree: {}", e);
                return StatusCode::INTERNAL_SERVER_ERROR;
            }
        }
    };

    StatusCode::OK
}
