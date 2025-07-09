use std::{collections::HashMap, env};

use axum::{
    Json, Router,
    extract::Query,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let app = Router::new()
        .route("/payments-summary", get(get_payments_summary))
        .route("/payments", post(exec_payment));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:9999").await.unwrap();
    println!("Server started on http://0.0.0.0:9999");
    axum::serve(listener, app).await.unwrap();
}

async fn get_payments_summary(Query(params): Query<HashMap<String, String>>) -> impl IntoResponse {
    let from = params.get("from");
    let to = params.get("to");

    // Query TB based on the range
    // cache range values (sled?)

    StatusCode::OK
}

async fn exec_payment(Json(payload): Json<Payment>) -> impl IntoResponse {
    // 2. Send to PP default (handle faults), or to fallback
    // Create a custom logic to decide which payment processor should be used.
    // 3. Finalize the pending transfer
    let default_payment_processor_url = env::var("PAYMENT_PROCESSOR_URL_DEFAULT")
        .unwrap_or_else(|_| "http://0.0.0.0:8001/payments".to_string());
    let client = Client::new();

    let res = client
        .post(default_payment_processor_url)
        .json(&payload)
        .send()
        .await
        .unwrap();

    if res.status().is_success() {
        StatusCode::OK
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

#[derive(Deserialize, Serialize)]
struct Payment {
    #[serde(rename = "correlationId")]
    correlation_id: Uuid,
    amount: u64,
}

#[derive(Serialize)]
struct GlobalSummary {
    default: Summary,
    fallback: Summary,
}

#[derive(Serialize)]
struct Summary {
    #[serde(rename = "totalRequests")]
    total_requests: u64,
    #[serde(rename = "totalAmount")]
    total_amount: u64,
}
