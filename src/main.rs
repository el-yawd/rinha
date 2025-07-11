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
use rusqlite::Connection;
use tokio::sync::{mpsc, oneshot};
use types::{Payment, PaymentMessage};

mod provider;
mod types;

// Database operations
#[derive(Debug)]
pub enum DbOperation {
    InsertPayment {
        correlation_id: String,
        amount: f64,
        timestamp: String,
        response: oneshot::Sender<Result<(), String>>,
    },
    GetPaymentsSummary {
        from: String,
        to: String,
        response: oneshot::Sender<Result<(usize, f64), String>>,
    },
}

#[derive(Clone)]
pub struct AppState {
    pub tx: mpsc::Sender<PaymentMessage>,
    pub db_tx: mpsc::Sender<DbOperation>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    const NUM_WORKERS: usize = 4;

    // Create database task
    let (db_tx, db_rx) = mpsc::channel::<DbOperation>(1000);
    spawn_db_task(db_rx).await?;

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
        .with_state(AppState {
            tx: main_tx.clone(),
            db_tx,
        });

    let listener = tokio::net::TcpListener::bind("0.0.0.0:9999").await?;
    axum::serve(listener, app).await?;

    Ok(())
}

async fn spawn_db_task(mut rx: mpsc::Receiver<DbOperation>) -> anyhow::Result<()> {
    let db = Connection::open("payments.db")?;
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS payments (
            correlation_id TEXT PRIMARY KEY,
            amount REAL NOT NULL,
            timestamp TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_payments_timestamp
        ON payments (timestamp);
        ",
    )?;

    tokio::spawn(async move {
        while let Some(op) = rx.recv().await {
            match op {
                DbOperation::InsertPayment {
                    correlation_id,
                    amount,
                    timestamp,
                    response,
                } => {
                    let result = db.execute(
                        "INSERT INTO payments (correlation_id, amount, timestamp) VALUES (?1, ?2, ?3)",
                        (&correlation_id, amount, &timestamp),
                    );

                    let _ = response.send(result.map(|_| ()).map_err(|e| e.to_string()));
                }
                DbOperation::GetPaymentsSummary { from, to, response } => {
                    let result = (|| -> Result<(usize, f64), String> {
                        let mut stmt = db
                            .prepare("SELECT amount FROM payments WHERE timestamp >= ?1 AND timestamp <= ?2")
                            .map_err(|e| e.to_string())?;

                        let rows: Result<Vec<f64>, _> = stmt
                            .query_map([&from, &to], |row| row.get::<_, f64>(0))
                            .map_err(|e| e.to_string())?
                            .collect();

                        let amounts = rows.map_err(|e| e.to_string())?;
                        let count = amounts.len();
                        let total: f64 = amounts.into_iter().sum();
                        Ok((count, total))
                    })();

                    let _ = response.send(result);
                }
            }
        }
    });

    Ok(())
}

async fn get_payments_summary(
    Query(params): Query<HashMap<String, String>>,
    State(app_state): State<AppState>,
) -> impl IntoResponse {
    let from = params
        .get("from")
        .map(String::as_str)
        .unwrap_or("0000-01-01T00:00:00Z")
        .to_string();

    let to = params
        .get("to")
        .map(String::as_str)
        .unwrap_or("9999-12-31T23:59:59Z")
        .to_string();

    let (response_tx, response_rx) = oneshot::channel();

    let db_op = DbOperation::GetPaymentsSummary {
        from,
        to,
        response: response_tx,
    };

    if app_state.db_tx.send(db_op).await.is_err() {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    match response_rx.await {
        Ok(Ok((count, total))) => {
            let response = Json(serde_json::json!({ "count": count, "total": total }));
            (StatusCode::OK, response).into_response()
        }
        Ok(Err(_)) | Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn exec_payment(
    State(app_state): State<AppState>,
    Json(payload): Json<Payment>,
) -> impl IntoResponse {
    let msg = PaymentMessage {
        payment: payload.clone(),
        timestamp: chrono::Utc::now().to_rfc3339(),
    };

    if let Err(e) = app_state.tx.send(msg).await {
        eprintln!("Failed to enqueue payment: {}", e);
        return StatusCode::INTERNAL_SERVER_ERROR;
    }

    let (response_tx, response_rx) = oneshot::channel();
    let timestamp = chrono::Utc::now().to_rfc3339();

    let db_op = DbOperation::InsertPayment {
        correlation_id: payload.correlation_id.to_string(),
        amount: payload.amount,
        timestamp,
        response: response_tx,
    };

    if app_state.db_tx.send(db_op).await.is_err() {
        return StatusCode::INTERNAL_SERVER_ERROR;
    }

    match response_rx.await {
        Ok(Ok(_)) => StatusCode::ACCEPTED,
        Ok(Err(e)) => {
            eprintln!("Failed to insert payment: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

pub async fn spawn_worker(
    mut rx: mpsc::Receiver<PaymentMessage>,
    handler: ProviderHandler,
    id: usize,
) {
    tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            let correlation_id = msg.payment.correlation_id;
            if let Err(e) = handler.process_payment(msg.payment).await {
                eprintln!("[worker {id}] Failed to process payment: {e}");
            } else {
                tracing::debug!("[worker {id}] Processed payment {}", correlation_id);
            }
        }

        eprintln!("[worker {id}] Channel closed, exiting...");
    });
}
