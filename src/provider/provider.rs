use std::{collections::HashMap, env, sync::LazyLock};

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

pub enum CurrentProvider {
    Default,
    Fallback,
}

#[derive(Debug)]
pub struct Fee(pub f64);

#[derive(Debug)]
pub struct Provider {
    pub fee: Fee,
}

impl Provider {
    pub fn new(fee: Fee) -> Self {
        Provider { fee }
    }
}
