use common::{BitSet, TinySet};

use crate::docset::{DocSet, BLOCK_NUM_TINYBITSETS, BLOCK_WINDOW, TERMINATED};
use crate::DocId;

/// A `BitSetDocSet` makes it possible to iterate through a bitset as if it was a `DocSet`.
///
/// # Implementation detail
///
/// Skipping is relatively fast here as we can directly point to the
/// right tiny bitset bucket.
///
/// TODO: Consider implementing a `BitTreeSet` in order to advance faster
/// when the bitset is sparse
pub struct BitSetDocSet {
    docs: BitSet,
    cursor_bucket: u32, //< index associated with the current tiny bitset
    cursor_tinybitset: TinySet,
    doc: u32,
    use_dense_fill: bool,
}

impl BitSetDocSet {
    fn go_to_bucket(&mut self, bucket_addr: u32) {
        self.cursor_bucket = bucket_addr;
        self.cursor_tinybitset = self.docs.tinyset(bucket_addr);
    }

    #[cold]
    #[inline(never)]
    fn fill_bitset_block_dense(
        &mut self,
        min_doc: DocId,
        mask: &mut [TinySet; BLOCK_NUM_TINYBITSETS],
    ) -> DocId {
        self.seek(min_doc);
        let horizon = min_doc + BLOCK_WINDOW;
        if self.doc == TERMINATED || self.doc >= horizon {
            return self.doc;
        }

        let delta = self.doc - min_doc;
        mask[(delta / 64) as usize].insert_mut(delta % 64);
        let shift = min_doc % 64;
        let min_bucket = min_doc / 64;

        loop {
            if self.cursor_tinybitset.is_empty() {
                let Some(cursor_bucket) = self.docs.first_non_empty_bucket(self.cursor_bucket + 1)
                else {
                    self.doc = TERMINATED;
                    return TERMINATED;
                };
                self.go_to_bucket(cursor_bucket);
            }

            let bucket_base = self.cursor_bucket * 64;
            if bucket_base >= horizon {
                let lower = self.cursor_tinybitset.pop_lowest().unwrap();
                self.doc = bucket_base | lower;
                return self.doc;
            }

            let remaining = self.cursor_tinybitset;
            let visible = if bucket_base + 64 > horizon {
                remaining.intersect(TinySet::range_lower(horizon % 64))
            } else {
                remaining
            };
            let bucket = (self.cursor_bucket - min_bucket) as usize;
            let visible_bits = u64::from_le_bytes(visible.into_bytes());
            let upper_bits = visible_bits >> shift;
            if upper_bits != 0 {
                mask[bucket] = mask[bucket].union(TinySet::deserialize(upper_bits.to_le_bytes()));
            }
            if shift != 0 && bucket > 0 {
                let lower_bits = visible_bits << (64 - shift);
                if lower_bits != 0 {
                    mask[bucket - 1] =
                        mask[bucket - 1].union(TinySet::deserialize(lower_bits.to_le_bytes()));
                }
            }

            if visible != remaining {
                let remaining_bits = u64::from_le_bytes(remaining.into_bytes());
                let beyond_bits = remaining_bits & !visible_bits;
                self.cursor_tinybitset = TinySet::deserialize(beyond_bits.to_le_bytes());
                let lower = self.cursor_tinybitset.pop_lowest().unwrap();
                self.doc = bucket_base | lower;
                return self.doc;
            }
            self.cursor_tinybitset = TinySet::EMPTY;
        }
    }
}

impl From<BitSet> for BitSetDocSet {
    fn from(docs: BitSet) -> BitSetDocSet {
        let use_dense_fill = (docs.len() as u64).saturating_mul(32) >= u64::from(docs.max_value());
        let first_tiny_bitset = if docs.max_value() == 0 {
            TinySet::empty()
        } else {
            docs.tinyset(0)
        };
        let mut docset = BitSetDocSet {
            docs,
            cursor_bucket: 0,
            cursor_tinybitset: first_tiny_bitset,
            doc: 0u32,
            use_dense_fill,
        };
        docset.advance();
        docset
    }
}

impl DocSet for BitSetDocSet {
    #[inline]
    fn advance(&mut self) -> DocId {
        if let Some(lower) = self.cursor_tinybitset.pop_lowest() {
            self.doc = (self.cursor_bucket * 64u32) | lower;
            return self.doc;
        }
        if let Some(cursor_bucket) = self.docs.first_non_empty_bucket(self.cursor_bucket + 1) {
            self.go_to_bucket(cursor_bucket);
            let lower = self.cursor_tinybitset.pop_lowest().unwrap();
            self.doc = (cursor_bucket * 64u32) | lower;
            self.doc
        } else {
            self.doc = TERMINATED;
            TERMINATED
        }
    }

    fn seek(&mut self, target: DocId) -> DocId {
        if target >= self.docs.max_value() {
            self.doc = TERMINATED;
            return TERMINATED;
        }
        let target_bucket = target / 64u32;
        if target_bucket > self.cursor_bucket {
            self.go_to_bucket(target_bucket);
        }
        if self.doc < target {
            let greater_filter = TinySet::range_greater_or_equal(target);
            self.cursor_tinybitset = self.cursor_tinybitset.intersect(greater_filter);
            self.advance()
        } else {
            self.doc
        }
    }

    #[inline]
    fn fill_bitset_block(
        &mut self,
        min_doc: DocId,
        mask: &mut [TinySet; BLOCK_NUM_TINYBITSETS],
    ) -> DocId {
        if self.use_dense_fill {
            return self.fill_bitset_block_dense(min_doc, mask);
        }

        self.seek(min_doc);
        let horizon = min_doc + BLOCK_WINDOW;
        loop {
            let doc = self.doc();
            if doc >= horizon {
                return doc;
            }
            let delta = doc - min_doc;
            mask[(delta / 64) as usize].insert_mut(delta % 64);
            if self.advance() == TERMINATED {
                return TERMINATED;
            }
        }
    }

    /// Returns the current document
    fn doc(&self) -> DocId {
        self.doc
    }

    /// Returns the number of values set in the underlying bitset.
    fn size_hint(&self) -> u32 {
        self.docs.len() as u32
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use common::{BitSet, TinySet};

    use super::BitSetDocSet;
    use crate::docset::{DocSet, BLOCK_NUM_TINYBITSETS, TERMINATED};
    use crate::query::VecDocSet;
    use crate::tests::generate_nonunique_unsorted;
    use crate::DocId;

    fn create_docbitset(docs: &[DocId], max_doc: DocId) -> BitSetDocSet {
        let mut docset = BitSet::with_max_value(max_doc);
        for &doc in docs {
            docset.insert(doc);
        }
        BitSetDocSet::from(docset)
    }

    #[test]
    fn test_bitset_large() {
        let arr = generate_nonunique_unsorted(100_000, 5_000);
        let mut btreeset: BTreeSet<u32> = BTreeSet::new();
        let mut bitset = BitSet::with_max_value(100_000);
        for el in arr {
            btreeset.insert(el);
            bitset.insert(el);
        }
        for i in 0..100_000 {
            assert_eq!(btreeset.contains(&i), bitset.contains(i));
        }
        assert_eq!(btreeset.len(), bitset.len());
        let mut bitset_docset = BitSetDocSet::from(bitset);
        let mut remaining = true;
        for el in btreeset.into_iter() {
            assert!(remaining);
            assert_eq!(bitset_docset.doc(), el);
            remaining = bitset_docset.advance() != TERMINATED;
        }
        assert!(!remaining);
    }

    #[test]
    fn test_empty() {
        let bitset = BitSet::with_max_value(1000);
        let mut empty = BitSetDocSet::from(bitset);
        assert_eq!(empty.advance(), TERMINATED)
    }

    #[test]
    fn test_seek_terminated() {
        let bitset = BitSet::with_max_value(1000);
        let mut empty = BitSetDocSet::from(bitset);
        assert_eq!(empty.seek(TERMINATED), TERMINATED)
    }

    fn test_go_through_sequential(docs: &[DocId]) {
        let mut docset = create_docbitset(docs, 1_000u32);
        for &doc in docs {
            assert_eq!(doc, docset.doc());
            docset.advance();
        }
        assert_eq!(docset.advance(), TERMINATED);
    }

    #[test]
    fn test_docbitset_sequential() {
        test_go_through_sequential(&[1, 2, 3]);
        test_go_through_sequential(&[1, 2, 3, 4, 5, 63, 64, 65]);
        test_go_through_sequential(&[63, 64, 65]);
        test_go_through_sequential(&[1, 2, 3, 4, 95, 96, 97, 98, 99]);
    }

    #[test]
    fn test_docbitset_skip() {
        {
            let mut docset = create_docbitset(&[1, 5, 6, 7, 5112], 10_000);
            assert_eq!(docset.seek(7), 7);
            assert_eq!(docset.doc(), 7);
            assert_eq!(docset.advance(), 5112);
            assert_eq!(docset.doc(), 5112);
            assert_eq!(docset.advance(), TERMINATED);
        }
        {
            let mut docset = create_docbitset(&[1, 5, 6, 7, 5112], 10_000);
            assert_eq!(docset.seek(3), 5);
            assert_eq!(docset.doc(), 5);
            assert_eq!(docset.advance(), 6);
        }
        {
            let mut docset = create_docbitset(&[5112], 10_000);
            assert_eq!(docset.seek(5112), 5112);
            assert_eq!(docset.doc(), 5112);
            assert_eq!(docset.advance(), TERMINATED);
        }
        {
            let mut docset = create_docbitset(&[5112], 10_000);
            assert_eq!(docset.seek(5113), TERMINATED);
            assert_eq!(docset.advance(), TERMINATED);
        }
        {
            let mut docset = create_docbitset(&[5112], 10_000);
            assert_eq!(docset.seek(5111), 5112);
            assert_eq!(docset.doc(), 5112);
            assert_eq!(docset.advance(), TERMINATED);
        }
        {
            let mut docset = create_docbitset(&[1, 5, 6, 7, 5112, 5500, 6666], 10_000);
            assert_eq!(docset.seek(5112), 5112);
            assert_eq!(docset.doc(), 5112);
            assert_eq!(docset.advance(), 5500);
            assert_eq!(docset.doc(), 5500);
            assert_eq!(docset.advance(), 6666);
            assert_eq!(docset.doc(), 6666);
            assert_eq!(docset.advance(), TERMINATED);
        }
        {
            let mut docset = create_docbitset(&[1, 5, 6, 7, 5112, 5500, 6666], 10_000);
            assert_eq!(docset.seek(5111), 5112);
            assert_eq!(docset.doc(), 5112);
            assert_eq!(docset.advance(), 5500);
            assert_eq!(docset.doc(), 5500);
            assert_eq!(docset.advance(), 6666);
            assert_eq!(docset.doc(), 6666);
            assert_eq!(docset.advance(), TERMINATED);
        }
        {
            let mut docset = create_docbitset(&[1, 5, 6, 7, 5112, 5513, 6666], 10_000);
            assert_eq!(docset.seek(5111), 5112);
            assert_eq!(docset.doc(), 5112);
            assert_eq!(docset.advance(), 5513);
            assert_eq!(docset.doc(), 5513);
            assert_eq!(docset.advance(), 6666);
            assert_eq!(docset.doc(), 6666);
            assert_eq!(docset.advance(), TERMINATED);
        }
    }

    fn assert_fill_bitset_block(docs: &[DocId], max_doc: DocId, min_doc: DocId) {
        let mut bitset_docset = create_docbitset(docs, max_doc);
        let mut vec_docset = VecDocSet::from(docs.to_vec());
        let mut initial_mask = [TinySet::EMPTY; BLOCK_NUM_TINYBITSETS];
        initial_mask[0].insert_mut(0);
        initial_mask[BLOCK_NUM_TINYBITSETS - 1].insert_mut(63);
        let mut actual_mask = initial_mask;
        let mut expected_mask = initial_mask;

        let actual_next = bitset_docset.fill_bitset_block(min_doc, &mut actual_mask);
        let expected_next = vec_docset.fill_bitset_block(min_doc, &mut expected_mask);
        assert_eq!(actual_mask, expected_mask);
        assert_eq!(actual_next, expected_next);
        assert_eq!(bitset_docset.doc(), vec_docset.doc());
    }

    #[test]
    fn test_fill_bitset_block() {
        for max_doc in [1, 17, 63, 64, 65, 100, 1023, 1024, 1025, 1100, 2048] {
            let dense: Vec<DocId> = (0..max_doc).collect();
            let sparse: Vec<DocId> = (0..max_doc).step_by(3).collect();
            let very_sparse: Vec<DocId> = (0..max_doc).step_by(100).collect();
            for docs in [dense, sparse, very_sparse] {
                for min_doc in [0, 1, 17, 63, 64, 65, 100, 1023, 1024, 1025, 1100] {
                    assert_fill_bitset_block(&docs, max_doc, min_doc);
                }
            }
        }

        for step in [1, 3, 100] {
            let docs: Vec<DocId> = (0..2_500).step_by(step).collect();
            let mut bitset_docset = create_docbitset(&docs, 2_500);
            let mut vec_docset = VecDocSet::from(docs);
            let mut min_doc = 0;
            loop {
                let mut actual_mask = [TinySet::EMPTY; BLOCK_NUM_TINYBITSETS];
                let mut expected_mask = [TinySet::EMPTY; BLOCK_NUM_TINYBITSETS];
                let actual_next = bitset_docset.fill_bitset_block(min_doc, &mut actual_mask);
                let expected_next = vec_docset.fill_bitset_block(min_doc, &mut expected_mask);
                assert_eq!(actual_mask, expected_mask);
                assert_eq!(actual_next, expected_next);
                assert_eq!(bitset_docset.doc(), vec_docset.doc());
                if actual_next == TERMINATED {
                    break;
                }
                min_doc = actual_next;
            }
        }
    }

    #[test]
    fn test_seek_all_targets() {
        for max_doc in [1, 63, 64, 65, 130, 1025] {
            for step in [1, 3, 7, 100] {
                let docs: Vec<DocId> = (0..max_doc).step_by(step).collect();
                let mut bitset_docset = create_docbitset(&docs, max_doc);
                let mut vec_docset = VecDocSet::from(docs);
                for target in 0..=max_doc {
                    if target < bitset_docset.doc() {
                        continue;
                    }
                    assert_eq!(bitset_docset.seek(target), vec_docset.seek(target));
                    assert_eq!(bitset_docset.doc(), vec_docset.doc());
                }
            }
        }
    }
}

#[cfg(all(test, feature = "unstable"))]
mod bench {

    use super::{BitSet, BitSetDocSet};
    use crate::docset::TERMINATED;
    use crate::{test, tests, DocSet};

    #[bench]
    fn bench_bitset_1pct_insert(b: &mut test::Bencher) {
        let els = tests::generate_nonunique_unsorted(1_000_000u32, 10_000);
        b.iter(|| {
            let mut bitset = BitSet::with_max_value(1_000_000);
            for el in els.iter().cloned() {
                bitset.insert(el);
            }
        });
    }

    #[bench]
    fn bench_bitset_1pct_clone(b: &mut test::Bencher) {
        let els = tests::generate_nonunique_unsorted(1_000_000u32, 10_000);
        let mut bitset = BitSet::with_max_value(1_000_000);
        for el in els {
            bitset.insert(el);
        }
        b.iter(|| bitset.clone());
    }

    #[bench]
    fn bench_bitset_1pct_clone_iterate(b: &mut test::Bencher) {
        let els = tests::sample(1_000_000u32, 0.01);
        let mut bitset = BitSet::with_max_value(1_000_000);
        for el in els {
            bitset.insert(el);
        }
        b.iter(|| {
            let mut docset = BitSetDocSet::from(bitset.clone());
            while docset.advance() != TERMINATED {}
        });
    }
}
