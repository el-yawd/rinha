use std::collections::HashMap;
use std::env;
use std::str::FromStr;

use axum::Extension;
use axum::extract::State;
use axum::{
    Json, Router,
    extract::Query,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use chrono::DateTime;
use provider::handler::ProviderHandler;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous};
use sqlx::{Pool, Sqlite};
use sqlx::{Row, SqlitePool};
use types::PaymentDTO;

mod provider;
mod types;

#[derive(Clone)]
struct AppState {
    pub handler_sender: async_channel::Sender<PaymentDTO>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    const NUM_WORKERS: usize = 5;

    let pool = connect_db().await?;

    // Initialize one handler per worker
    let handler = ProviderHandler::new(pool.clone()).await?;

    let (handler_sender, handler_receiver) = async_channel::unbounded::<PaymentDTO>();
    for _ in 0..NUM_WORKERS {
        spawn_worker(handler_receiver.clone(), handler.clone()).await;
    }

    // HTTP router
    let app = Router::new()
        .route("/payments-summary", get(get_payments_summary))
        .route("/payments", post(exec_payment))
        .route("/purge-payments", post(purge_payments))
        .layer(Extension(pool))
        .with_state(AppState { handler_sender });

    let listener = tokio::net::TcpListener::bind("0.0.0.0:9999").await?;
    axum::serve(listener, app).await?;

    Ok(())
}

fn parse_timestamp_to_unix(timestamp_str: &str) -> Result<i64, chrono::ParseError> {
    let dt = DateTime::parse_from_rfc3339(timestamp_str)?;
    Ok(dt.timestamp())
}

async fn get_payments_summary(
    Query(params): Query<HashMap<String, String>>,
    Extension(pool): Extension<Pool<Sqlite>>,
) -> impl IntoResponse {
    let from = params
        .get("from")
        .map(|s| parse_timestamp_to_unix(s))
        .unwrap_or(Ok(0))
        .unwrap_or(0);

    let to = params
        .get("to")
        .map(|s| parse_timestamp_to_unix(s))
        .unwrap_or(Ok(i64::MAX))
        .unwrap_or(i64::MAX);

    let result = sqlx::query(
        "
        SELECT
            is_default,
            COUNT(*) AS totalRequests,
            SUM(amount) AS totalAmount
        FROM payments
        WHERE timestamp >= ? AND timestamp <= ?
        GROUP BY is_default;",
    )
    .bind(&from)
    .bind(&to)
    .fetch_all(&pool)
    .await
    .unwrap();

    let mut summary = serde_json::json!({
        "default": { "totalRequests": 0, "totalAmount": 0.0 },
        "fallback": { "totalRequests": 0, "totalAmount": 0.0 }
    });

    for row in result {
        if row.get::<u8, _>(0) == 1 {
            summary["default"]["totalRequests"] = row.get::<u64, _>(1).into();
            summary["default"]["totalAmount"] = row.get::<f64, _>(2).into();
        } else {
            summary["fallback"]["totalRequests"] = row.get::<u64, _>(1).into();
            summary["fallback"]["totalAmount"] = row.get::<f64, _>(2).into();
        }
    }

    (StatusCode::OK, Json(summary))
}

async fn exec_payment(
    State(app_state): State<AppState>,
    Json(payload): Json<PaymentDTO>,
) -> impl IntoResponse {
    if let Err(_) = app_state.handler_sender.send(payload).await {
        return StatusCode::INTERNAL_SERVER_ERROR;
    }

    StatusCode::OK
}

pub async fn purge_payments(Extension(pool): Extension<Pool<Sqlite>>) -> impl IntoResponse {
    sqlx::query("DELETE FROM payments")
        .execute(&pool)
        .await
        .map(|_| StatusCode::OK)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
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

pub async fn connect_db() -> anyhow::Result<Pool<Sqlite>> {
    let db_url = env::var("DATABASE_URL").unwrap_or("data/app.db".to_string());
    let conn_opts = SqliteConnectOptions::from_str(&db_url)?
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal);

    let pool = SqlitePool::connect_with(conn_opts).await?;

    // TODO: Improve basic datatypes
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS payments (
                correlation_id TEXT PRIMARY KEY,
                is_default INT,
                amount REAL NOT NULL,
                timestamp INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_payments_timestamp
            ON payments (timestamp);
        ",
    )
    .execute(&pool)
    .await?;

    Ok(pool)
}
