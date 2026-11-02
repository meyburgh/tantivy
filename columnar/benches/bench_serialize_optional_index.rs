use binggan::{BenchRunner, black_box};
use tantivy_columnar::column_index::{
    OptionalIndex, SerializableColumnIndex, serialize_column_index,
};

fn main() {
    let mut runner = BenchRunner::new();
    for (name, num_rows, row_ids) in [
        ("small", 100, (0..100).collect::<Vec<_>>()),
        (
            "sparse",
            1_000_000,
            (0..1_000_000).step_by(21).collect::<Vec<_>>(),
        ),
        ("dense", 1_000_000, (0..1_000_000).collect::<Vec<_>>()),
    ] {
        let optional_index = OptionalIndex::for_test(num_rows, &row_ids);
        runner.bench_function(format!("serialize_optional_index_{name}"), move |_| {
            let serializable = SerializableColumnIndex::Optional((&optional_index).into());
            let mut output = Vec::new();
            serialize_column_index(serializable, &mut output).unwrap();
            black_box(output.len());
        });

        let optional_index = OptionalIndex::for_test(num_rows, &row_ids);
        let ranks: Vec<u32> = (0..row_ids.len() as u32).collect();
        runner.bench_function(format!("select_batch_{name}"), move |_| {
            let mut ranks = ranks.clone();
            optional_index.select_batch(&mut ranks);
            black_box(ranks.len());
        });
    }
}
