use std::collections::HashMap;

use axum::extract::State;
use axum::{
    Json, Router,
    extract::Query,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use provider::handler::ProviderHandler;
use tokio::sync::mpsc;
use types::{Payment, PaymentMessage};

mod provider;
mod types;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    const NUM_WORKERS: usize = 4;

    let (main_tx, mut main_rx) = mpsc::channel::<PaymentMessage>(1000);

    // Initialize one handler per worker
    let handler = match ProviderHandler::new().await {
        Ok(h) => h,
        Err(e) => {
            return Err(e);
        }
    };

    let mut worker_txs = Vec::with_capacity(NUM_WORKERS);

    for i in 0..NUM_WORKERS {
        let (tx, rx) = mpsc::channel(100);
        spawn_worker(rx, handler.clone(), i).await;
        worker_txs.push(tx);
    }

    tokio::spawn(async move {
        let mut i = 0;
        while let Some(msg) = main_rx.recv().await {
            let tx = &worker_txs[i % NUM_WORKERS];
            if tx.send(msg).await.is_err() {
                eprintln!("worker {i} died");
            }
            i += 1;
        }
    });

    // HTTP router
    let app = Router::new()
        .route("/payments-summary", get(get_payments_summary))
        .route("/payments", post(exec_payment))
        .with_state(main_tx.clone());
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

async fn exec_payment(
    State(tx): State<mpsc::Sender<PaymentMessage>>,
    Json(payload): Json<Payment>,
) -> impl IntoResponse {
    let now = chrono::Utc::now().to_rfc3339();

    let msg = PaymentMessage {
        payment: payload,
        timestamp: now,
    };

    if let Err(e) = tx.send(msg).await {
        eprintln!("Failed to enqueue payment: {}", e);
        return StatusCode::INTERNAL_SERVER_ERROR;
    }

    StatusCode::ACCEPTED
}

pub async fn spawn_worker(
    mut rx: mpsc::Receiver<PaymentMessage>,
    handler: ProviderHandler,
    id: usize,
) {
    tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            let correlation_id = msg.payment.correlation_id;
            if let Err(e) = handler
                .process_payment(msg.payment, msg.timestamp.clone())
                .await
            {
                eprintln!("[worker {id}] Failed to process payment: {e}");
            } else {
                tracing::debug!("[worker {id}] Processed payment {}", correlation_id);
            }
        }

        eprintln!("[worker {id}] Channel closed, exiting...");
    });
}
