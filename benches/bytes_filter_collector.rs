use binggan::{black_box, BenchRunner};
use tantivy::collector::{BytesFilterCollector, Count};
use tantivy::query::AllQuery;
use tantivy::schema::{Schema, FAST};
use tantivy::{Index, TantivyDocument};

const NUM_DOCS: u64 = 50_000;

fn build_index(optional: bool) -> Index {
    let mut schema_builder = Schema::builder();
    let barcode = schema_builder.add_bytes_field("barcode", FAST);
    let index = Index::create_in_ram(schema_builder.build());
    let mut writer = index.writer_with_num_threads(1, 500_000_000).unwrap();
    for value in 0..NUM_DOCS {
        let mut document = TantivyDocument::default();
        if !optional || value % 2 == 0 {
            document.add_bytes(barcode, &format!("{value:03}").into_bytes());
        }
        writer.add_document(document).unwrap();
    }
    writer.commit().unwrap();
    index
}

fn main() {
    let mut runner = BenchRunner::new();
    for (name, optional) in [("full", false), ("optional", true)] {
        let index = build_index(optional);
        let searcher = index.reader().unwrap().searcher();
        let collector =
            BytesFilterCollector::new("barcode".to_string(), |bytes: &[u8]| bytes.len() % 2 == 1, Count);
        runner.bench_function(format!("{name}_bytes_filter"), move |_| {
            black_box(searcher.search(&AllQuery, &collector).unwrap());
        });
    }
}
