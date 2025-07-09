use std::collections::HashMap;

use axum::{
    Json, Router,
    extract::Query,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use provider::handler::ProviderHandler;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

mod provider;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    match ProviderHandler::new().await {
        Ok(handler) => {
            println!(
                "Default Fee: {:?}\n, Fallback Fee: {:?}",
                handler.default_provider.fee, handler.fallback_provider.fee
            );
        }
        Err(e) => {
            eprintln!("Error initializing provider handler: {}", e);
        }
    }

    let app = Router::new()
        .route("/payments-summary", get(get_payments_summary))
        .route("/payments", post(exec_payment));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:9999").await?;
    axum::serve(listener, app).await?;

    Ok(())
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

    StatusCode::OK
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
