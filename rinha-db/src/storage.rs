use chrono::Utc;
use chrono::{DateTime, Timelike};
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ProviderName {
    Default,
    Fallback,
}

#[derive(Debug, Clone)]
pub struct Stats {
    total_requests: u64,
    total_amount: f64,
}

pub type TimeBucket = DateTime<Utc>;
pub type ProviderStats = BTreeMap<TimeBucket, Stats>;

#[derive(Debug, Default)]
pub struct SummaryStore {
    data: HashMap<ProviderName, ProviderStats>,
}

impl SummaryStore {
    pub fn insert(&mut self, provider: ProviderName, time: DateTime<Utc>, amount: f64) {
        let bucket = time.with_second(0).unwrap().with_nanosecond(0).unwrap();

        let provider_map = self.data.entry(provider).or_default();

        provider_map
            .entry(bucket)
            .and_modify(|s| {
                s.total_requests += 1;
                s.total_amount += amount;
            })
            .or_insert(Stats {
                total_requests: 1,
                total_amount: amount,
            });
    }

    pub fn query(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> HashMap<ProviderName, Stats> {
        let mut result = HashMap::new();

        for (provider, tree) in &self.data {
            let mut total_requests = 0;
            let mut total_amount = 0.0;

            for (_, stat) in tree.range(from..=to) {
                total_requests += stat.total_requests;
                total_amount += stat.total_amount;
            }

            result.insert(
                provider.clone(),
                Stats {
                    total_requests,
                    total_amount,
                },
            );
        }

        result
    }
}
