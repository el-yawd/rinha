use reqwest::{Client, Request};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
use uuid::Uuid;

use anyhow;
use axum::{
    Json, Router,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
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
    db_client: Client,
    api_pool: [Arc<UnixConnectionPool>; 2],
    balancer: Arc<AtomicU64>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut headers = HeaderMap::new();
    headers.insert("Content-Type", "application/json".parse()?);
    // TODO: Tweak Client config
    let db_client = Client::builder()
        .no_gzip()
        .no_zstd()
        .default_headers(headers.clone())
        .build()?;

    // HTTP router
    let app = Router::new()
        .route("/payments-summary", get(get_payments_summary))
        .route("/payments", post(exec_payment))
        .route("/purge-payments", post(purge_payments))
        .with_state(AppState {
            db_client,
            api_pool: [
                Arc::new(UnixConnectionPool::new(Path::new("/tmp/api-1.sock"), 200).await?),
                Arc::new(UnixConnectionPool::new(Path::new("/tmp/api-2.sock"), 200).await?),
            ],
            balancer: Arc::new(AtomicU64::new(0)),
        });

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

    let res = state
        .db_client
        .get(format!(
            "http://rinha-db:8888/summary?from={}&to={}",
            from, to
        ))
        .send()
        .await;

    match res {
        Ok(res) => {
            let summary = res.json::<GlobalSummary>().await.unwrap();
            (StatusCode::OK, Json(summary))
        }
        Err(_) => todo!(),
    }
}

async fn exec_payment(
    State(state): State<AppState>,
    Json(payload): Json<PaymentDTO>,
) -> impl IntoResponse {
    let api_pool = if (state.balancer.fetch_add(1, Ordering::Relaxed) & 1) == 0 {
        &state.api_pool[0]
    } else {
        &state.api_pool[1]
    };
    let mut stream = api_pool.acquire().await.unwrap();
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

async fn purge_payments(State(state): State<AppState>) -> impl IntoResponse {
    let _ = state
        .db_client
        .delete("http://rinha-db:8888/purge")
        .send()
        .await;

    StatusCode::OK
}
