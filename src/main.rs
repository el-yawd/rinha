use std::collections::HashMap;

use axum::{
    extract::Query,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use uuid::Uuid;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let app = Router::new()
        .route("/payments-summary", get(get_payments_summary))
        .route("/payments", post(exec_payment));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:9999").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn get_payments_summary(Query(params): Query<HashMap<String, String>>) -> impl IntoResponse {
    let from = params.get("from");
    let to = params.get("to");

    (StatusCode::OK)
}

async fn exec_payment(Json(payload): Json<Payment>) -> impl IntoResponse {
    (StatusCode::OK)
}

#[derive(Deserialize)]
struct Payment {
    #[serde(rename = "correlationId")]
    correlation_id: Uuid,
    amount: f64,
}

#[derive(Deserialize)]
struct GlobalSummary {
    default: Summary,
    fallback: Summary,
}

#[derive(Deserialize)]
struct Summary {
    #[serde(rename = "totalRequests")]
    total_requests: u64,
    #[serde(rename = "totalAmount")]
    total_amount: u64,
}
