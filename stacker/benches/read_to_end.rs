use binggan::{BenchRunner, black_box};
use tantivy_stacker::{ExpUnrolledLinkedList, MemoryArena};

fn main() {
    let mut runner = BenchRunner::new();
    for num_values in [100u32, 100_000, 1_000_000] {
        let mut arena = MemoryArena::default();
        let mut list = ExpUnrolledLinkedList::default();
        for value in 0..num_values {
            list.writer(&mut arena).write_u32_vint(value);
        }
        runner.bench_function(format!("read_to_end_{num_values}"), move |_| {
            let mut output = Vec::new();
            list.read_to_end(&arena, &mut output);
            black_box(output.len());
        });
    }
}
