use binggan::{black_box, BenchRunner};
use tantivy::collector::{Count, FilterCollector};
use tantivy::query::AllQuery;
use tantivy::schema::{Schema, FAST};
use tantivy::{Index, TantivyDocument};

const NUM_DOCS: u64 = 500_000;

fn main() {
    let mut schema_builder = Schema::builder();
    let value_field = schema_builder.add_u64_field("value", FAST);
    let index = Index::create_in_ram(schema_builder.build());
    let mut writer = index.writer_with_num_threads(1, 500_000_000).unwrap();
    for value in 0..NUM_DOCS {
        let mut document = TantivyDocument::default();
        document.add_u64(value_field, value % 100);
        writer.add_document(document).unwrap();
    }
    writer.commit().unwrap();
    let searcher = index.reader().unwrap().searcher();
    let collector = FilterCollector::new("value".to_string(), |value: u64| value % 2 == 0, Count);
    let mut runner = BenchRunner::new();
    runner.bench_function("full_column_filter", move |_| {
        black_box(searcher.search(&AllQuery, &collector).unwrap());
    });

    let mut schema_builder = Schema::builder();
    let value_field = schema_builder.add_u64_field("value", FAST);
    let index = Index::create_in_ram(schema_builder.build());
    let mut writer = index.writer_with_num_threads(1, 500_000_000).unwrap();
    for value in 0..NUM_DOCS {
        let mut document = TantivyDocument::default();
        if value % 2 == 0 {
            document.add_u64(value_field, value % 100);
        }
        writer.add_document(document).unwrap();
    }
    writer.commit().unwrap();
    let searcher = index.reader().unwrap().searcher();
    let collector = FilterCollector::new("value".to_string(), |value: u64| value % 2 == 0, Count);
    runner.bench_function("optional_column_filter", move |_| {
        black_box(searcher.search(&AllQuery, &collector).unwrap());
    });
}
