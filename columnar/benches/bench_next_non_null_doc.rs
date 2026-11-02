use binggan::{BenchRunner, black_box};
use tantivy_columnar::column_index::OptionalIndex;

fn main() {
    let mut runner = BenchRunner::new();
    for (name, step) in [("sparse", 21usize), ("dense", 6usize)] {
        let row_ids: Vec<u32> = (0..1_000_000usize)
            .step_by(step)
            .map(|value| value as u32)
            .collect();
        let targets: Vec<u32> = row_ids.iter().skip(1).map(|value| value - 1).collect();
        let optional_index = OptionalIndex::for_test(1_000_000, &row_ids);
        runner.bench_function(format!("next_non_null_doc_{name}"), move |_| {
            let mut sum = 0u64;
            for &target in &targets {
                sum += u64::from(optional_index.next_non_null_doc(target).unwrap());
            }
            black_box(sum);
        });
    }
}
