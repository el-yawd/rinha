use serde::{Deserialize, Serialize};
use std::{collections::HashMap, env, sync::LazyLock};
use uuid::Uuid;

use anyhow;
use axum::{
    Json, Router,
    extract::Query,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use shared_types::{self, GlobalSummary, PaymentDTO};
use tokio::{
    self,
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
};

const RINHADB_SOCK: LazyLock<String> =
    LazyLock::new(|| env::var("RINHADB_SOCK").unwrap_or("/tmp/rinha.sock".to_string()));
const API_1_SOCK: LazyLock<String> =
    LazyLock::new(|| env::var("API_1_SOCK").unwrap_or("/tmp/api-1.sock".to_string()));
const API_2_SOCK: LazyLock<String> =
    LazyLock::new(|| env::var("API_2_SOCK").unwrap_or("/tmp/api-2.sock".to_string()));

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // HTTP router
    let app = Router::new()
        .route("/payments-summary", get(get_payments_summary))
        .route("/payments", post(exec_payment))
        .route("/purge-payments", post(purge_payments));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:9999").await?;
    axum::serve(listener, app).await?;

    Ok(())
}

async fn get_payments_summary(Query(params): Query<HashMap<String, String>>) -> impl IntoResponse {
    let from = params
        .get("from")
        .cloned()
        .unwrap_or_else(|| "0000-01-01T00:00:00Z".to_string());
    let to = params
        .get("to")
        .cloned()
        .unwrap_or_else(|| "9999-12-31T23:59:59Z".to_string());

    let message = shared_types::Message::Read { from, to };

    let mut stream = UnixStream::connect(RINHADB_SOCK.as_str())
        .await
        .expect("failed to connect to RinhaDB");

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

    let mut reader = BufReader::new(stream);
    let mut response_line = String::new();
    reader
        .read_line(&mut response_line)
        .await
        .expect("failed to read response");

    let summary = serde_json::from_str::<GlobalSummary>(response_line.as_str()).unwrap();

    (StatusCode::OK, Json(summary))
}

// TODO: Round-robin logic
async fn exec_payment(Json(payload): Json<PaymentDTO>) -> impl IntoResponse {
    let mut stream = UnixStream::connect(API_1_SOCK.as_str())
        .await
        .expect("failed to connect to API-1");
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
    let mut stream = UnixStream::connect(RINHADB_SOCK.as_str())
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
