use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Payment {
    #[serde(rename = "correlationId")]
    pub correlation_id: Uuid,
    pub amount: f64,
}

#[derive(Serialize)]
pub struct GlobalSummary {
    default: Summary,
    fallback: Summary,
}

#[derive(Serialize)]
pub struct Summary {
    #[serde(rename = "totalRequests")]
    total_requests: u64,
    #[serde(rename = "totalAmount")]
    total_amount: f64,
}

#[derive(Debug, Clone)]
pub struct PaymentMessage {
    pub payment: Payment,
}
