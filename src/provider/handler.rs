use std::sync::Arc;

use ::serde::Deserialize;
use axum::http::HeaderMap;
use chrono::Utc;
use reqwest::Client;
use tokio::sync::RwLock;

use crate::types::{PaymentDTO, PaymentServiceDTO};

use super::provider::{CurrentProvider, Fee, Provider, URLS};

#[derive(Clone)]
pub struct ProviderHandler {
    pub client: Client,
    pub current_provider: CurrentProvider,
    pub fallback_provider: Arc<RwLock<Provider>>,
    pub default_provider: Arc<RwLock<Provider>>,
    default_tree: sled::Tree,
    fallback_tree: sled::Tree,
}

impl ProviderHandler {
    pub async fn new(default_tree: sled::Tree, fallback_tree: sled::Tree) -> anyhow::Result<Self> {
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
            fallback_tree,
            default_tree,
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
                self.default_tree
                    .insert(now.as_bytes(), &payload.amount.to_be_bytes())?;

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
                    self.fallback_tree
                        .insert(now.as_bytes(), &payload.amount.to_be_bytes())?;
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
