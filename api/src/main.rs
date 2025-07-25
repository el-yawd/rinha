use async_channel::Receiver;
use async_channel::Sender;
use async_channel::unbounded;
use axum::http::HeaderMap;
use chrono::Utc;
use reqwest::Client;
use serde::Deserialize;
use serde::Serialize;
use shared_types::PaymentDTO;
use shared_types::UnixConnectionPool;
use std::collections::HashMap;
use std::env;
use std::path::Path;
use std::sync::{Arc, LazyLock};
use tokio;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::net::UnixListener;
use uuid::Uuid;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let num_workers: usize = env::var("NUM_WORKERS")
        .unwrap_or("5".to_string())
        .parse()
        .unwrap();
    let api_path = env::var("API_PATH").unwrap_or("/tmp/api-1.sock".to_string());

    if Path::new(api_path.as_str()).exists() {
        std::fs::remove_file(api_path.as_str())?;
    }

    let listener = UnixListener::bind(api_path.as_str())?;
    println!("API listening on {}", api_path.as_str());

    let (tx, rx): (Sender<PaymentDTO>, Receiver<PaymentDTO>) = unbounded();
    let handler = Arc::new(ProviderHandler::new().await?);

    for i in 0..num_workers {
        let handler = Arc::clone(&handler);
        let rx = rx.clone();
        tokio::spawn(async move {
            while let Ok(payment) = rx.recv().await {
                if let Err(e) = handler.process_payment(payment).await {
                    eprintln!("[worker-{i}] Failed to process payment: {e}");
                }
            }
        });
    }

    loop {
        let (stream, _) = listener.accept().await?;
        let (reader, _) = stream.into_split();
        let mut reader = BufReader::new(reader).lines();

        let tx = tx.clone();
        tokio::spawn(async move {
            while let Ok(Some(line)) = reader.next_line().await {
                if line.trim().is_empty() {
                    continue;
                }

                match serde_json::from_str::<PaymentDTO>(&line) {
                    Ok(payment) => {
                        if let Err(e) = tx.send(payment).await {
                            eprintln!("Channel send failed: {e}");
                        }
                    }
                    Err(e) => {
                        eprintln!("Invalid payment payload: {e}");
                    }
                }
            }
        });
    }
}

#[derive(Clone)]
pub struct ProviderHandler {
    pub client: Client,
    pub current_provider: CurrentProvider,
    db_pool: Arc<UnixConnectionPool>,
}

impl ProviderHandler {
    pub async fn new() -> anyhow::Result<Self> {
        let mut headers = HeaderMap::new();
        headers.insert("Content-Type", "application/json".parse()?);
        // TODO: Tweak Client config
        let client = Client::builder()
            .no_gzip()
            .no_zstd()
            .default_headers(headers.clone())
            .build()?;

        let db_pool = Arc::new(UnixConnectionPool::new(Path::new("/tmp/rinha.sock"), 10).await?);

        Ok(Self {
            client,
            current_provider: CurrentProvider::Default,
            db_pool,
        })
    }

    /// Process a payment using a naive strategy. If default provider is down try fallback, if both fails drop the payment.
    // TODO: Explore different strategies for handling payment processing failures.
    pub async fn process_payment(&self, payload: PaymentDTO) -> anyhow::Result<()> {
        let now = Utc::now().to_rfc3339();
        let payload = PaymentServiceDTO::new(payload, now.clone());
        match self
            .client
            .post(URLS.get("default_payments").unwrap())
            .body(serde_json::to_string(&payload)?)
            .send()
            .await?
            .error_for_status()
        {
            Ok(_) => {
                let mut stream = self.db_pool.acquire().await?;
                let msg = shared_types::Message::Write {
                    key: now,
                    value: payload.amount,
                    tree: shared_types::SledTree::Default,
                };
                let serialized = serde_json::to_string(&msg)?;
                stream.write_all(serialized.as_bytes()).await?;
                stream.write_all(b"\n").await?;
                stream.flush().await?;
                Ok(())
            }

            Err(_) => {
                let res = self
                    .client
                    .post(URLS.get("fallback_payments").unwrap())
                    .body(serde_json::to_string(&payload)?)
                    .send()
                    .await?
                    .error_for_status();

                if res.is_ok() {
                    let mut stream = self.db_pool.acquire().await?;
                    let msg = shared_types::Message::Write {
                        key: now,
                        value: payload.amount,
                        tree: shared_types::SledTree::Fallback,
                    };
                    let serialized = serde_json::to_string(&msg)?;
                    stream.write_all(serialized.as_bytes()).await?;
                    stream.write_all(b"\n").await?;
                    stream.flush().await?;
                }

                Ok(())
            }
        }
    }
}

#[derive(Deserialize)]
struct PaymentSummaryResponse {
    #[serde(rename = "totalRequests")]
    total_requests: f64,
    #[serde(rename = "totalAmount")]
    total_amount: f64,
    #[serde(rename = "totalFee")]
    total_fee: f64,
    #[serde(rename = "feePerTransaction")]
    fee_per_transaction: f64,
}

#[derive(Deserialize)]
struct ProviderHealthResponse {
    failing: bool,
    #[serde(rename = "minResponseTime")]
    min_response_time: u64,
}

pub static URLS: LazyLock<HashMap<&'static str, String>> = LazyLock::new(|| {
    let default_base = env::var("PAYMENT_PROCESSOR_URL_DEFAULT")
        .unwrap_or_else(|_| "http://0.0.0.0:8001".to_string());
    let fallback_base = env::var("PAYMENT_PROCESSOR_URL_FALLBACK")
        .unwrap_or_else(|_| "http://0.0.0.0:8002".to_string());

    HashMap::from([
        ("default_payments", format!("{}/payments", default_base)),
        ("fallback_payments", format!("{}/payments", fallback_base)),
        (
            "default_payments_health",
            format!("{}/payments/service-health", default_base),
        ),
        (
            "fallback_payments_health",
            format!("{}/payments/service-health", fallback_base),
        ),
    ])
});

#[derive(Clone)]
pub enum CurrentProvider {
    Default,
    Fallback,
}

#[derive(Serialize)]
pub struct PaymentServiceDTO {
    #[serde(rename = "correlationId")]
    pub correlation_id: Uuid,
    pub amount: f64,
    #[serde(rename = "requestedAt")]
    pub requested_at: String,
}

impl PaymentServiceDTO {
    pub fn new(payment: PaymentDTO, requested_at: String) -> Self {
        PaymentServiceDTO {
            correlation_id: payment.correlation_id,
            amount: payment.amount,
            requested_at,
        }
    }
}
