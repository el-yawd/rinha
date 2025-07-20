use serde::{Deserialize, Serialize};
use sled;
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize)]
pub enum Message {
    Read {
        from: String,
        to: String,
    },
    Write {
        key: String,
        value: f64,
        tree: SledTree,
    },
    Purge,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum SledTree {
    Fallback,
    Default,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct GlobalSummary {
    pub default: Summary,
    pub fallback: Summary,
}

impl Default for GlobalSummary {
    fn default() -> Self {
        Self {
            default: Summary::new(),
            fallback: Summary::new(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Summary {
    #[serde(rename = "totalRequests")]
    pub total_requests: u64,
    #[serde(rename = "totalAmount")]
    pub total_amount: f64,
}

impl Summary {
    pub fn new() -> Self {
        Summary {
            total_requests: 0,
            total_amount: 0.0,
        }
    }
}

impl FromIterator<sled::Result<(sled::IVec, sled::IVec)>> for Summary {
    fn from_iter<I: IntoIterator<Item = sled::Result<(sled::IVec, sled::IVec)>>>(iter: I) -> Self {
        iter.into_iter()
            .filter_map(Result::ok)
            .map(|(_, value)| {
                f64::from_be_bytes(value.as_ref().try_into().expect("Expected 8 bytes"))
            })
            .fold(Summary::new(), |mut summary, amount| {
                summary.total_amount += amount;
                summary.total_requests += 1;
                summary
            })
    }
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct PaymentDTO {
    #[serde(rename = "correlationId")]
    pub correlation_id: Uuid,
    pub amount: f64,
}
