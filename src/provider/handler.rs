use ::serde::Deserialize;
use axum::http::HeaderMap;
use reqwest::Client;

use crate::types::Payment;

use super::provider::{CurrentProvider, Fee, Provider, URLS};

#[derive(Clone)]
pub struct ProviderHandler {
    pub client: Client,
    pub current_provider: CurrentProvider,
    pub fallback_provider: Provider,
    pub default_provider: Provider,
}

impl ProviderHandler {
    pub async fn new() -> anyhow::Result<Self> {
        let mut headers = HeaderMap::new();
        headers.insert("X-Rinha-Token", "123".parse().unwrap());
        headers.insert("Content-Type", "application/json".parse().unwrap());
        // TODO: Tweak Client config
        let client = Client::builder().default_headers(headers.clone()).build()?;

        // TODO: Make they in parallel
        let default_res = client
            .get(URLS.get("default_summary").unwrap())
            .send()
            .await?
            .json::<PaymentSummaryResponse>()
            .await?;

        let fallback_res = client
            .get(URLS.get("fallback_summary").unwrap())
            .send()
            .await?
            .json::<PaymentSummaryResponse>()
            .await?;

        Ok(Self {
            client,
            current_provider: CurrentProvider::Default,
            fallback_provider: Provider::new(Fee(fallback_res.fee_per_transaction)),
            default_provider: Provider::new(Fee(default_res.fee_per_transaction)),
        })
    }

    pub async fn process_payment(&self, payment: Payment) -> anyhow::Result<()> {
        self.client
            .post(URLS.get("default_payments").unwrap())
            .body(serde_json::to_string(&payment)?)
            .send()
            .await?
            .error_for_status()?;

        Ok(())
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
