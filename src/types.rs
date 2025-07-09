use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Deserialize, Serialize, Debug)]
pub struct Payment {
    #[serde(rename = "correlationId")]
    pub correlation_id: Uuid,
    amount: u64,
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
    total_amount: u64,
}

#[derive(Debug)]
pub struct PaymentMessage {
    pub payment: Payment,
    pub timestamp: String,
}
