use async_channel::Receiver;
use async_channel::Sender;
use async_channel::bounded;
use axum::http::HeaderMap;
use chrono::Utc;
use reqwest::Client;
use serde::Deserialize;
use serde::Serialize;
use shared_types::PaymentDTO;
use std::collections::HashMap;
use std::env;
use std::path::Path;
use std::sync::{Arc, LazyLock};
use tokio;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::net::UnixListener;
use tokio::net::UnixStream;
use tokio::sync::RwLock;
use uuid::Uuid;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let num_workers: usize = env::var("NUM_WORKERS")
        .unwrap_or("5".to_string())
        .parse()
        .unwrap();

    if Path::new("/tmp/api-1.sock").exists() {
        std::fs::remove_file("/tmp/api-1.sock")?;
    }

    let listener = UnixListener::bind("/tmp/api-1.sock")?;
    println!("API-1 listening on {}", "/tmp/api-1.sock");

    let (tx, rx): (Sender<PaymentDTO>, Receiver<PaymentDTO>) = bounded(1000);
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
    pub fallback_provider: Arc<RwLock<Provider>>,
    pub default_provider: Arc<RwLock<Provider>>,
}

impl ProviderHandler {
    pub async fn new() -> anyhow::Result<Self> {
        let mut headers = HeaderMap::new();
        headers.insert("X-Rinha-Token", "123".parse()?);
        headers.insert("Content-Type", "application/json".parse()?);
        // TODO: Tweak Client config
        let client = Client::builder()
            .no_gzip()
            .no_zstd()
            .default_headers(headers.clone())
            .build()?;

        let (default_sum, fallback_sum, default_health, fallback_health) = tokio::try_join!(
            async {
                client
                    .get(URLS.get("default_summary").unwrap())
                    .send()
                    .await?
                    .json::<PaymentSummaryResponse>()
                    .await
            },
            async {
                client
                    .get(URLS.get("fallback_summary").unwrap())
                    .send()
                    .await?
                    .json::<PaymentSummaryResponse>()
                    .await
            },
            async {
                client
                    .get(URLS.get("default_payments_health").unwrap())
                    .send()
                    .await?
                    .json::<ProviderHealthResponse>()
                    .await
            },
            async {
                client
                    .get(URLS.get("fallback_payments_health").unwrap())
                    .send()
                    .await?
                    .json::<ProviderHealthResponse>()
                    .await
            }
        )
        .expect("Unable to connect with external providers, Aborting...");

        Ok(Self {
            client,
            current_provider: CurrentProvider::Default,
            fallback_provider: Arc::new(RwLock::new(Provider::new(
                Fee(fallback_sum.fee_per_transaction),
                fallback_health.failing,
                fallback_health.min_response_time,
            ))),
            default_provider: Arc::new(RwLock::new(Provider::new(
                Fee(default_sum.fee_per_transaction),
                default_health.failing,
                default_health.min_response_time,
            ))),
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
                let mut stream = UnixStream::connect("/tmp/rinha.sock").await?;
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
                    let mut stream = UnixStream::connect("/tmp/rinha.sock").await?;
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
        (
            "default_summary",
            format!("{}/admin/payments-summary", default_base),
        ),
        (
            "fallback_summary",
            format!("{}/admin/payments-summary", fallback_base),
        ),
    ])
});

#[derive(Clone)]
pub enum CurrentProvider {
    Default,
    Fallback,
}

#[derive(Debug, Clone)]
pub struct Fee(pub f64);

#[derive(Debug, Clone)]
pub struct Provider {
    pub fee: Fee,
    pub is_failing: bool,
    pub min_res_time: u64,
}

impl Provider {
    pub fn new(fee: Fee, is_failing: bool, min_res_time: u64) -> Self {
        Provider {
            fee,
            is_failing,
            min_res_time,
        }
    }
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
