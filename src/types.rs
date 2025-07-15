use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct PaymentDTO {
    #[serde(rename = "correlationId")]
    pub correlation_id: Uuid,
    pub amount: f64,
}

#[derive(Serialize, Clone)]
pub struct GlobalSummary {
    pub default: Summary,
    pub fallback: Summary,
}

#[derive(Serialize, Clone)]
pub struct Summary {
    #[serde(rename = "totalRequests")]
    pub total_requests: u64,
    #[serde(rename = "totalAmount")]
    pub total_amount: f64,
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
