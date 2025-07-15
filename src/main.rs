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
use moka::future::Cache;
use provider::handler::ProviderHandler;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous};
use sqlx::{Pool, Sqlite};
use sqlx::{Row, SqlitePool};
use types::{GlobalSummary, PaymentDTO, Summary};

mod provider;
mod types;

#[derive(Clone)]
struct AppState {
    pub handler_sender: async_channel::Sender<PaymentDTO>,
    pub summary_cache: Arc<Cache<String, GlobalSummary>>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    const NUM_WORKERS: usize = 5;

    match connect_db().await {
        Ok(pool) => {
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
                .with_state(AppState {
                    handler_sender,
                    summary_cache: Arc::new(Cache::new(1000)),
                });

            let listener = tokio::net::TcpListener::bind("0.0.0.0:9999").await?;
            axum::serve(listener, app).await?;
            return Ok(());
        }

        Err(err) => {
            println!("Something went wrong with the db, entering in sleep mode for debugging...");
            println!("Error: {}", err);
            sleep(Duration::from_secs(100000000000));
        }
    }

    Ok(())
}

async fn get_payments_summary(
    Query(params): Query<HashMap<String, String>>,
    Extension(pool): Extension<Pool<Sqlite>>,
    State(state): State<AppState>,
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

    let cache_key = format!("{from}:{to}");

    if let Some(summary) = state.summary_cache.get(&cache_key).await {
        return (StatusCode::OK, Json(summary));
    }

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

    let mut default = Summary {
        total_requests: 0,
        total_amount: 0.0,
    };

    let mut fallback = Summary {
        total_requests: 0,
        total_amount: 0.0,
    };

    for row in result {
        let is_default: u8 = row.get(0);
        let count: u64 = row.get(1);
        let sum: f64 = row.get(2);

        if is_default == 1 {
            default.total_requests = count;
            default.total_amount = sum;
        } else {
            fallback.total_requests = count;
            fallback.total_amount = sum;
        }
    }

    (StatusCode::OK, Json(GlobalSummary { default, fallback }))
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
    let conn_opts = SqliteConnectOptions::new()
        .filename(&db_url)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Off) // We just have to survive for 5 min :)
        .busy_timeout(Duration::from_secs(10))
        .pragma("temp_store", "MEMORY")
        .pragma("mmap_size", "10000000")
        .pragma("cache_size", "-40000") // double of default
        .pragma("optimize", "0x10002");

    let pool = SqlitePool::connect_with(conn_opts).await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS payments (
            correlation_id TEXT PRIMARY KEY,
            is_default INTEGER NOT NULL,
            amount REAL NOT NULL,
            timestamp TEXT NOT NULL
        ) WITHOUT ROWID;

        CREATE INDEX IF NOT EXISTS idx_payments_timestamp
        ON payments (timestamp);

        CREATE INDEX IF NOT EXISTS idx_payments_provider
        ON payments (is_default, timestamp);

        ANALYZE payments;",
    )
    .execute(&pool)
    .await?;

    Ok(pool)
}
