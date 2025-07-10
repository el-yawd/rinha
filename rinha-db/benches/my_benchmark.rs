#[path = "../src/storage.rs"]
mod storage;

use chrono::Utc;
use criterion::{Criterion, criterion_group, criterion_main};
use storage::{ProviderName, SummaryStore};

fn insert_payment_benchmark(c: &mut Criterion) {
    c.bench_function("insert_payment", |b| {
        b.iter(|| {
            let mut store = SummaryStore::default();

            for _ in 0..1000 {
                store.insert(ProviderName::Default, Utc::now(), 42.0);
            }
        })
    });
}

fn query_range_benchmark(c: &mut Criterion) {
    use chrono::Duration;

    c.bench_function("query_range", |b| {
        let mut store = SummaryStore::default();
        let now = Utc::now();

        // Preload some data
        for i in 0..10_000 {
            let ts = now - Duration::seconds(i);
            store.insert(ProviderName::Fallback, ts, 99.99);
        }

        b.iter(|| {
            store.query(now - Duration::minutes(10), now);
        });
    });
}

criterion_group!(benches, insert_payment_benchmark, query_range_benchmark);
criterion_main!(benches);
