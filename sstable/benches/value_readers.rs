use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use tantivy_sstable::value::{
    RangeValueReader, RangeValueWriter, U64MonotonicValueReader, U64MonotonicValueWriter,
    ValueReader, ValueWriter, VecU32ValueReader, VecU32ValueWriter,
};

fn u64_monotonic_load(c: &mut Criterion) {
    let mut writer = U64MonotonicValueWriter::default();
    for value in 0..100_000u64 {
        writer.write(&value);
    }
    let mut data = Vec::new();
    writer.serialize_block(&mut data);
    c.bench_function("u64_monotonic_load_100000", |b| {
        b.iter(|| {
            let mut reader = U64MonotonicValueReader::default();
            black_box(reader.load(black_box(&data)).unwrap());
            black_box(reader.value(99_999));
        })
    });
}

fn vec_u32_load(c: &mut Criterion) {
    let mut writer = VecU32ValueWriter::default();
    for value in 0..1_000u32 {
        writer.write(&vec![value, value + 1]);
    }
    let mut data = Vec::new();
    writer.serialize_block(&mut data);
    c.bench_function("vec_u32_load_1000_blocks", |b| {
        b.iter(|| {
            let mut reader = VecU32ValueReader::default();
            black_box(reader.load(black_box(&data)).unwrap());
            black_box(reader.value(999));
        })
    });
}

fn range_load(c: &mut Criterion) {
    let mut writer = RangeValueWriter::default();
    for value in 0..1_000u64 {
        writer.write(&(value..value + 1));
    }
    let mut data = Vec::new();
    writer.serialize_block(&mut data);
    c.bench_function("range_load_1000", |b| {
        b.iter(|| {
            let mut reader = RangeValueReader::default();
            black_box(reader.load(black_box(&data)).unwrap());
            black_box(reader.value(999));
        })
    });
}

criterion_group!(value_readers, u64_monotonic_load, vec_u32_load, range_load);
criterion_main!(value_readers);
