use std::collections::HashMap;
use std::env;
use std::sync::Arc;
use std::thread::sleep;
use std::time::Duration;

use axum::Extension;
use axum::extract::State;
use axum::{
    Json, Router,
    extract::Query,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use provider::handler::ProviderHandler;
use types::{GlobalSummary, PaymentDTO, Summary};

mod provider;
mod types;

#[derive(Clone)]
struct AppState {
    pub handler_sender: async_channel::Sender<PaymentDTO>,
}

// (Default, Fallback)
type TreePair = (sled::Tree, sled::Tree);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let num_workers: usize = env::var("NUM_WORKERS")
        .unwrap_or("5".to_string())
        .parse()
        .unwrap();

    let database_url: String = env::var("DATABASE_URL")
        .unwrap_or("app_db".to_string())
        .parse()
        .unwrap();

    let db = sled::open(database_url)?;

    let default_tree = db.open_tree("default")?;
    let fallback_tree = db.open_tree("fallback")?;

    // Initialize one handler per worker
    let handler = ProviderHandler::new(default_tree.clone(), fallback_tree.clone()).await?;

    let (handler_sender, handler_receiver) = async_channel::unbounded::<PaymentDTO>();

    for _ in 0..num_workers {
        spawn_worker(handler_receiver.clone(), handler.clone()).await;
    }

    // HTTP router
    let app = Router::new()
        .route("/payments-summary", get(get_payments_summary))
        .route("/payments", post(exec_payment))
        .route("/purge-payments", post(purge_payments))
        .layer(Extension((default_tree, fallback_tree)))
        .with_state(AppState { handler_sender });

    let listener = tokio::net::TcpListener::bind("0.0.0.0:9999").await?;
    axum::serve(listener, app).await?;

    Ok(())
}

async fn get_payments_summary(
    Query(params): Query<HashMap<String, String>>,
    Extension((default_tree, fallback_tree)): Extension<TreePair>,
) -> impl IntoResponse {
    // We just assume the strings are valid
    let from = params
        .get("from")
        .map(String::as_str)
        .unwrap_or("0000-01-01T00:00:00Z");

    let to = params
        .get("to")
        .map(String::as_str)
        .unwrap_or("9999-12-31T23:59:59Z");

    let default = Summary::from_iter(default_tree.range(from..=to));
    let fallback = Summary::from_iter(fallback_tree.range(from..=to));

    (StatusCode::OK, Json(GlobalSummary { default, fallback }))
}

async fn exec_payment(
    State(app_state): State<AppState>,
    Json(payload): Json<PaymentDTO>,
) -> impl IntoResponse {
    app_state
        .handler_sender
        .send(payload)
        .await
        .map(|_| StatusCode::OK)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

pub async fn purge_payments(
    Extension((default, fallback)): Extension<TreePair>,
) -> impl IntoResponse {
    match (default.clear(), fallback.clear()) {
        (Ok(_), Ok(_)) => StatusCode::OK,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

pub async fn spawn_worker(rx: async_channel::Receiver<PaymentDTO>, handler: ProviderHandler) {
    tokio::spawn(async move {
        while let Ok(msg) = rx.recv().await {
            if let Err(err) = handler.process_payment(msg).await {
                eprintln!("{}", err);
            }
        }
    });
}
