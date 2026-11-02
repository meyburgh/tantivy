use common::TinySet;

use crate::docset::{
    DocSet, BLOCK_NUM_TINYBITSETS, BLOCK_WINDOW, COLLECT_BLOCK_BUFFER_LEN, TERMINATED,
};
use crate::index::SegmentReader;
use crate::query::boost_query::BoostScorer;
use crate::query::explanation::does_not_match;
use crate::query::{EnableScoring, Explanation, Query, Scorer, Weight};
use crate::{DocId, Score};

/// Query that matches all of the documents.
///
/// All of the documents get the score 1.0.
#[derive(Clone, Debug)]
pub struct AllQuery;

impl Query for AllQuery {
    fn weight(&self, _: EnableScoring<'_>) -> crate::Result<Box<dyn Weight>> {
        Ok(Box::new(AllWeight))
    }
}

/// Weight associated with the `AllQuery` query.
pub struct AllWeight;

impl Weight for AllWeight {
    fn scorer(&self, reader: &SegmentReader, boost: Score) -> crate::Result<Box<dyn Scorer>> {
        let all_scorer = AllScorer::new(reader.max_doc());
        if boost != 1.0 {
            Ok(Box::new(BoostScorer::new(all_scorer, boost)))
        } else {
            Ok(Box::new(all_scorer))
        }
    }

    fn explain(&self, reader: &SegmentReader, doc: DocId) -> crate::Result<Explanation> {
        if doc >= reader.max_doc() {
            return Err(does_not_match(doc));
        }
        Ok(Explanation::new("AllQuery", 1.0))
    }
}

/// Scorer associated with the `AllQuery` query.
pub struct AllScorer {
    doc: DocId,
    max_doc: DocId,
}

impl AllScorer {
    /// Creates a new AllScorer with `max_doc` docs.
    pub fn new(max_doc: DocId) -> AllScorer {
        let doc = if max_doc == 0u32 { TERMINATED } else { 0 };
        AllScorer { doc, max_doc }
    }
}

impl DocSet for AllScorer {
    #[inline(always)]
    fn advance(&mut self) -> DocId {
        if self.doc + 1 >= self.max_doc {
            self.doc = TERMINATED;
            return TERMINATED;
        }
        self.doc += 1;
        self.doc
    }

    fn seek(&mut self, target: DocId) -> DocId {
        debug_assert!(target >= self.doc);
        self.doc = target;
        if self.doc >= self.max_doc {
            self.doc = TERMINATED;
        }
        self.doc
    }

    fn fill_buffer(&mut self, buffer: &mut [DocId; COLLECT_BLOCK_BUFFER_LEN]) -> usize {
        if self.doc() == TERMINATED {
            return 0;
        }
        let num_items = buffer.len();
        let is_safe_distance = self.doc() + (num_items as u32) < self.max_doc;
        if is_safe_distance {
            for buffer_val in buffer {
                *buffer_val = self.doc();
                self.doc += 1;
            }
            num_items
        } else if self.doc() + (num_items as u32) == self.max_doc {
            for buffer_val in buffer {
                *buffer_val = self.doc();
                self.doc += 1;
            }
            self.doc = TERMINATED;
            num_items
        } else {
            for (i, buffer_val) in buffer.iter_mut().enumerate() {
                *buffer_val = self.doc();
                if self.advance() == TERMINATED {
                    return i + 1;
                }
            }
            buffer.len()
        }
    }

    fn fill_bitset_block(
        &mut self,
        min_doc: DocId,
        mask: &mut [TinySet; BLOCK_NUM_TINYBITSETS],
    ) -> DocId {
        self.seek(min_doc);
        if self.doc == TERMINATED {
            return TERMINATED;
        }
        let horizon = min_doc + BLOCK_WINDOW;
        if self.doc == min_doc && horizon <= self.max_doc {
            mask.fill(TinySet::full());
            if horizon == self.max_doc {
                self.doc = TERMINATED;
                return TERMINATED;
            }
            self.doc = horizon;
            return horizon;
        }
        while self.doc < horizon {
            let delta = self.doc - min_doc;
            let bucket = (delta / 64) as usize;
            let offset = delta % 64;
            let bucket_len = (64 - offset)
                .min(horizon - self.doc)
                .min(self.max_doc - self.doc);
            let end_offset = offset + bucket_len;
            let mut tinyset = TinySet::range_greater_or_equal(offset);
            if end_offset < 64 {
                tinyset = tinyset.intersect(TinySet::range_lower(end_offset));
            }
            mask[bucket] = mask[bucket].union(tinyset);
            self.doc += bucket_len;
            if self.doc == self.max_doc {
                self.doc = TERMINATED;
                return TERMINATED;
            }
        }
        self.doc
    }

    #[inline(always)]
    fn doc(&self) -> DocId {
        self.doc
    }

    fn size_hint(&self) -> u32 {
        self.max_doc
    }
}

impl Scorer for AllScorer {
    #[inline]
    fn score(&mut self) -> Score {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use common::TinySet;

    use super::AllQuery;
    use crate::docset::{DocSet, BLOCK_NUM_TINYBITSETS, COLLECT_BLOCK_BUFFER_LEN, TERMINATED};
    use crate::query::{AllScorer, EnableScoring, Query, VecDocSet};
    use crate::schema::{Schema, TEXT};
    use crate::{Index, IndexWriter};

    fn create_test_index() -> crate::Result<Index> {
        let mut schema_builder = Schema::builder();
        let field = schema_builder.add_text_field("text", TEXT);
        let schema = schema_builder.build();
        let index = Index::create_in_ram(schema);
        let mut index_writer: IndexWriter = index.writer_for_tests()?;
        index_writer.add_document(doc!(field=>"aaa"))?;
        index_writer.add_document(doc!(field=>"bbb"))?;
        index_writer.commit()?;
        index_writer.add_document(doc!(field=>"ccc"))?;
        index_writer.commit()?;
        Ok(index)
    }

    #[test]
    fn test_all_query() -> crate::Result<()> {
        let index = create_test_index()?;
        let reader = index.reader()?;
        let searcher = reader.searcher();
        let weight = AllQuery.weight(EnableScoring::disabled_from_schema(&index.schema()))?;
        {
            let reader = searcher.segment_reader(0);
            let mut scorer = weight.scorer(reader, 1.0)?;
            assert_eq!(scorer.doc(), 0u32);
            assert_eq!(scorer.advance(), 1u32);
            assert_eq!(scorer.doc(), 1u32);
            assert_eq!(scorer.advance(), TERMINATED);
        }
        {
            let reader = searcher.segment_reader(1);
            let mut scorer = weight.scorer(reader, 1.0)?;
            assert_eq!(scorer.doc(), 0u32);
            assert_eq!(scorer.advance(), TERMINATED);
        }
        Ok(())
    }

    #[test]
    fn test_all_query_with_boost() -> crate::Result<()> {
        let index = create_test_index()?;
        let reader = index.reader()?;
        let searcher = reader.searcher();
        let weight = AllQuery.weight(EnableScoring::disabled_from_schema(searcher.schema()))?;
        let reader = searcher.segment_reader(0);
        {
            let mut scorer = weight.scorer(reader, 2.0)?;
            assert_eq!(scorer.doc(), 0u32);
            assert_eq!(scorer.score(), 2.0);
        }
        {
            let mut scorer = weight.scorer(reader, 1.5)?;
            assert_eq!(scorer.doc(), 0u32);
            assert_eq!(scorer.score(), 1.5);
        }
        Ok(())
    }

    #[test]
    pub fn test_fill_buffer() {
        let mut postings = AllScorer {
            doc: 0u32,
            max_doc: COLLECT_BLOCK_BUFFER_LEN as u32 * 2 + 9,
        };
        let mut buffer = [0u32; COLLECT_BLOCK_BUFFER_LEN];
        assert_eq!(postings.fill_buffer(&mut buffer), COLLECT_BLOCK_BUFFER_LEN);
        for i in 0u32..COLLECT_BLOCK_BUFFER_LEN as u32 {
            assert_eq!(buffer[i as usize], i);
        }
        assert_eq!(postings.fill_buffer(&mut buffer), COLLECT_BLOCK_BUFFER_LEN);
        for i in 0u32..COLLECT_BLOCK_BUFFER_LEN as u32 {
            assert_eq!(buffer[i as usize], i + COLLECT_BLOCK_BUFFER_LEN as u32);
        }
        assert_eq!(postings.fill_buffer(&mut buffer), 9);

        for num_docs in 0..=(COLLECT_BLOCK_BUFFER_LEN as u32 * 2 + 1) {
            let mut postings = AllScorer::new(num_docs);
            let mut actual = Vec::new();
            loop {
                let num_items = postings.fill_buffer(&mut buffer);
                actual.extend_from_slice(&buffer[..num_items]);
                if num_items != buffer.len() {
                    break;
                }
            }
            assert_eq!(actual, (0..num_docs).collect::<Vec<_>>());
            assert_eq!(postings.doc(), TERMINATED);
            assert_eq!(postings.fill_buffer(&mut buffer), 0);
        }
    }

    fn assert_fill_bitset_block(num_docs: u32, min_doc: u32) {
        let doc_ids: Vec<u32> = (0..num_docs).collect();
        let mut all_scorer = AllScorer::new(num_docs);
        let mut vec_docset = VecDocSet::from(doc_ids);
        let mut initial_mask = [TinySet::EMPTY; BLOCK_NUM_TINYBITSETS];
        initial_mask[0].insert_mut(0);
        initial_mask[BLOCK_NUM_TINYBITSETS - 1].insert_mut(63);
        let mut actual_mask = initial_mask;
        let mut expected_mask = initial_mask;

        let actual_next = all_scorer.fill_bitset_block(min_doc, &mut actual_mask);
        let expected_next = vec_docset.fill_bitset_block(min_doc, &mut expected_mask);
        assert_eq!(actual_mask, expected_mask);
        assert_eq!(actual_next, expected_next);
        assert_eq!(all_scorer.doc(), vec_docset.doc());
    }

    #[test]
    fn test_fill_bitset_block() {
        for num_docs in [1, 17, 63, 64, 65, 100, 1023, 1024, 1025, 1100, 2048] {
            for min_doc in [0, 1, 17, 63, 64, 65, 100, 1023, 1024, 1025, 1100] {
                assert_fill_bitset_block(num_docs, min_doc);
            }
        }

        let num_docs = 2_500u32;
        let mut all_scorer = AllScorer::new(num_docs);
        let mut vec_docset = VecDocSet::from((0..num_docs).collect::<Vec<_>>());
        let mut min_doc = 0;
        loop {
            let mut actual_mask = [TinySet::EMPTY; BLOCK_NUM_TINYBITSETS];
            let mut expected_mask = [TinySet::EMPTY; BLOCK_NUM_TINYBITSETS];
            let actual_next = all_scorer.fill_bitset_block(min_doc, &mut actual_mask);
            let expected_next = vec_docset.fill_bitset_block(min_doc, &mut expected_mask);
            assert_eq!(actual_mask, expected_mask);
            assert_eq!(actual_next, expected_next);
            assert_eq!(all_scorer.doc(), vec_docset.doc());
            if actual_next == TERMINATED {
                break;
            }
            min_doc = actual_next;
        }
    }
}
