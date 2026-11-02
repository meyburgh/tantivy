use binggan::{black_box, BenchRunner};
use common::TinySet;
use tantivy::query::AllScorer;
use tantivy::{DocSet, COLLECT_BLOCK_BUFFER_LEN, TERMINATED};

fn main() {
    let mut runner = BenchRunner::new();
    for num_docs in [64, 65, 1_000_000] {
        runner.bench_function(format!("fill_buffer_{num_docs}"), move |_| {
            let mut scorer = AllScorer::new(num_docs);
            let mut buffer = [0u32; COLLECT_BLOCK_BUFFER_LEN];
            let mut total = 0usize;
            loop {
                let num_items = scorer.fill_buffer(&mut buffer);
                total += num_items;
                black_box(&buffer[..num_items]);
                if num_items != buffer.len() {
                    break;
                }
            }
            total
        });
    }

    runner.bench_function("advance_1000000", |_| {
        let mut scorer = AllScorer::new(1_000_000);
        let mut count = 0usize;
        while black_box(scorer.advance()) != TERMINATED {
            count += 1;
        }
        count
    });

    runner.bench_function("fill_bitset_block_1000000", |_| {
        let mut scorer = AllScorer::new(1_000_000);
        let mut min_doc = 0u32;
        let mut total = 0u64;
        loop {
            let mut mask = [TinySet::EMPTY; 16];
            let next = scorer.fill_bitset_block(min_doc, &mut mask);
            total += mask.iter().map(|tinyset| tinyset.len() as u64).sum::<u64>();
            black_box(&mask);
            if next == TERMINATED {
                break;
            }
            min_doc = next;
        }
        total
    });
}
