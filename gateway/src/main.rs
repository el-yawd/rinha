use serde::{Deserialize, Serialize};
use std::{collections::HashMap, path::Path, sync::Arc};
use uuid::Uuid;

use anyhow;
use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use shared_types::{self, GlobalSummary, PaymentDTO, UnixConnectionPool};
use tokio::{
    self,
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
};

#[derive(Clone)]
struct AppState {
    db_pool: Arc<UnixConnectionPool>,
    api_pool: Arc<UnixConnectionPool>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // HTTP router
    let app = Router::new()
        .route("/payments-summary", get(get_payments_summary))
        .route("/payments", post(exec_payment))
        .with_state(AppState {
            db_pool: Arc::new(UnixConnectionPool::new(Path::new("/tmp/rinha.sock"), 10).await?),
            api_pool: Arc::new(UnixConnectionPool::new(Path::new("/tmp/api-1.sock"), 10).await?),
        });

    // Purge is broken so far, let's ignore it for now
    // .route("/purge-payments", post(purge_payments));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:9999").await?;
    axum::serve(listener, app).await?;

    Ok(())
}

async fn get_payments_summary(
    Query(params): Query<HashMap<String, String>>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let from = params
        .get("from")
        .cloned()
        .unwrap_or_else(|| "0000-01-01T00:00:00Z".to_string());
    let to = params
        .get("to")
        .cloned()
        .unwrap_or_else(|| "9999-12-31T23:59:59Z".to_string());

    let message = shared_types::Message::Read { from, to };

    let mut stream = state.db_pool.acquire().await.unwrap();

    let serialized = serde_json::to_string(&message).expect("failed to serialize message");

    stream
        .write_all(serialized.as_bytes())
        .await
        .expect("failed to write to RinhaDB");
    stream
        .write_all(b"\n")
        .await
        .expect("failed to write newline");

    stream.flush().await.expect("failed to flush");

    let mut reader = BufReader::new(stream.take().unwrap());
    let mut response_line = String::new();
    reader
        .read_line(&mut response_line)
        .await
        .expect("failed to read response");

    let summary = serde_json::from_str::<GlobalSummary>(response_line.as_str()).unwrap();

    (StatusCode::OK, Json(summary))
}

// TODO: Round-robin logic
async fn exec_payment(
    State(state): State<AppState>,
    Json(payload): Json<PaymentDTO>,
) -> impl IntoResponse {
    let mut stream = state.api_pool.acquire().await.unwrap();
    let serialized = serde_json::to_string(&payload).expect("failed to serialize payload");

    stream
        .write_all(serialized.as_bytes())
        .await
        .expect("failed to write to API-1");
    stream
        .write_all(b"\n")
        .await
        .expect("failed to write newline");

    stream.flush().await.expect("failed to flush");

    StatusCode::OK
}

async fn purge_payments() -> impl IntoResponse {
    let mut stream = UnixStream::connect("/tmp/rinha.sock")
        .await
        .expect("failed to connect to RinhaDB");

    let serialized =
        serde_json::to_string(&shared_types::Message::Purge).expect("failed to serialize message");

    stream
        .write_all(serialized.as_bytes())
        .await
        .expect("failed to write to RinhaDB");
    stream
        .write_all(b"\n")
        .await
        .expect("failed to write newline");

    stream.flush().await.expect("failed to flush");

    StatusCode::OK
}
