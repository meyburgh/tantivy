use binggan::{black_box, BenchRunner};
use common::{BitSet, TinySet};
use tantivy::query::BitSetDocSet;
use tantivy::{DocId, DocSet, TERMINATED};

const NUM_DOCS: u32 = 1_000_000;

struct GenericDocSet(BitSetDocSet);

impl DocSet for GenericDocSet {
    fn advance(&mut self) -> DocId {
        self.0.advance()
    }

    fn seek(&mut self, target: DocId) -> DocId {
        self.0.seek(target)
    }

    fn doc(&self) -> DocId {
        self.0.doc()
    }

    fn size_hint(&self) -> u32 {
        self.0.size_hint()
    }
}

fn run(docset: &mut impl DocSet) -> u64 {
    let mut min_doc = 0u32;
    let mut total = 0u64;
    loop {
        let mut mask = [TinySet::EMPTY; 16];
        let next = docset.fill_bitset_block(min_doc, &mut mask);
        total += mask.iter().map(|tinyset| tinyset.len() as u64).sum::<u64>();
        black_box(&mask);
        if next == TERMINATED {
            break;
        }
        min_doc = next;
    }
    total
}

fn main() {
    let mut runner = BenchRunner::new();
    for step in [1, 3, 100] {
        let mut bitset = BitSet::with_max_value(NUM_DOCS);
        for doc in (0..NUM_DOCS).step_by(step) {
            bitset.insert(doc);
        }
        let mut group = runner.new_group();
        group.set_name(format!("fill_bitset_block_step_{step}"));
        let generic_bitset = bitset.clone();
        group.register("generic", move |_| {
            run(&mut GenericDocSet(BitSetDocSet::from(
                generic_bitset.clone(),
            )))
        });
        group.register("specialized", move |_| {
            run(&mut BitSetDocSet::from(bitset.clone()))
        });
        group.run();
    }

    let mut bitset = BitSet::with_max_value(NUM_DOCS);
    for doc in 0..NUM_DOCS {
        bitset.insert(doc);
    }
    let mut group = runner.new_group();
    group.set_name("same_bucket_seek");
    let advance_bitset = bitset.clone();
    group.register("advance", move |_| {
        let mut docset = BitSetDocSet::from(advance_bitset.clone());
        let mut total = 0u64;
        for base in (0..NUM_DOCS).step_by(64) {
            docset.seek(base);
            for _ in 0..63 {
                docset.advance();
            }
            total += u64::from(docset.doc());
        }
        black_box(&docset);
        total
    });
    group.register("seek", move |_| {
        let mut docset = BitSetDocSet::from(bitset.clone());
        let mut total = 0u64;
        for base in (0..NUM_DOCS).step_by(64) {
            docset.seek(base);
            total += u64::from(docset.seek(base + 63));
        }
        black_box(&docset);
        total
    });
    group.run();
}
