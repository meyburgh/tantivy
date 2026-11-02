use common::{HasLen, TinySet};

use crate::docset::{DocSet, BLOCK_NUM_TINYBITSETS, BLOCK_WINDOW};
use crate::fastfield::AliveBitSet;
use crate::positions::PositionReader;
use crate::postings::compression::COMPRESSION_BLOCK_SIZE;
use crate::postings::{BlockSegmentPostings, Postings};
use crate::{DocId, TERMINATED};

/// `SegmentPostings` represents the inverted list or postings associated with
/// a term in a `Segment`.
///
/// As we iterate through the `SegmentPostings`, the frequencies are optionally decoded.
/// Positions on the other hand, are optionally entirely decoded upfront.
#[derive(Clone)]
pub struct SegmentPostings {
    pub(crate) block_cursor: BlockSegmentPostings,
    cur: usize,
    position_reader: Option<PositionReader>,
}

impl SegmentPostings {
    /// Returns an empty segment postings object
    pub fn empty() -> Self {
        SegmentPostings {
            block_cursor: BlockSegmentPostings::empty(),
            cur: 0,
            position_reader: None,
        }
    }

    /// Compute the number of non-deleted documents.
    ///
    /// This method will clone and scan through the posting lists.
    /// (this is a rather expensive operation).
    pub fn doc_freq_given_deletes(&self, alive_bitset: &AliveBitSet) -> u32 {
        let mut docset = self.clone();
        let mut doc_freq = 0;
        loop {
            let doc = docset.doc();
            if doc == TERMINATED {
                return doc_freq;
            }
            if alive_bitset.is_alive(doc) {
                doc_freq += 1u32;
            }
            docset.advance();
        }
    }

    /// Returns the overall number of documents in the block postings.
    /// It does not take in account whether documents are deleted or not.
    pub fn doc_freq(&self) -> u32 {
        self.block_cursor.doc_freq()
    }

    /// Creates a segment postings object with the given documents
    /// and no frequency encoded.
    ///
    /// This method is mostly useful for unit tests.
    ///
    /// It serializes the doc ids using tantivy's codec
    /// and returns a `SegmentPostings` object that embeds a
    /// buffer with the serialized data.
    #[cfg(test)]
    pub fn create_from_docs(docs: &[u32]) -> SegmentPostings {
        use crate::directory::FileSlice;
        use crate::postings::serializer::PostingsSerializer;
        use crate::schema::IndexRecordOption;
        let mut buffer = Vec::new();
        {
            let mut postings_serializer =
                PostingsSerializer::new(0.0, IndexRecordOption::Basic, None);
            postings_serializer.new_term(docs.len() as u32, false);
            for &doc in docs {
                postings_serializer.write_doc(doc, 1u32);
            }
            postings_serializer
                .close_term(docs.len() as u32, &mut buffer)
                .expect("In memory Serialization should never fail.");
        }
        let block_segment_postings = BlockSegmentPostings::open(
            docs.len() as u32,
            FileSlice::from(buffer),
            IndexRecordOption::Basic,
            IndexRecordOption::Basic,
        )
        .unwrap();
        SegmentPostings::from_block_postings(block_segment_postings, None)
    }

    /// Helper functions to create `SegmentPostings` for tests.
    #[cfg(test)]
    pub fn create_from_docs_and_tfs(
        doc_and_tfs: &[(u32, u32)],
        fieldnorms: Option<&[u32]>,
    ) -> SegmentPostings {
        use crate::directory::FileSlice;
        use crate::fieldnorm::FieldNormReader;
        use crate::postings::serializer::PostingsSerializer;
        use crate::schema::IndexRecordOption;
        use crate::Score;
        let mut buffer: Vec<u8> = Vec::new();
        let fieldnorm_reader = fieldnorms.map(FieldNormReader::for_test);
        let average_field_norm = fieldnorms
            .map(|fieldnorms| {
                if fieldnorms.is_empty() {
                    return 0.0;
                }
                let total_num_tokens: u64 = fieldnorms
                    .iter()
                    .map(|&fieldnorm| fieldnorm as u64)
                    .sum::<u64>();
                total_num_tokens as Score / fieldnorms.len() as Score
            })
            .unwrap_or(0.0);
        let mut postings_serializer = PostingsSerializer::new(
            average_field_norm,
            IndexRecordOption::WithFreqs,
            fieldnorm_reader,
        );
        postings_serializer.new_term(doc_and_tfs.len() as u32, true);
        for &(doc, tf) in doc_and_tfs {
            postings_serializer.write_doc(doc, tf);
        }
        postings_serializer
            .close_term(doc_and_tfs.len() as u32, &mut buffer)
            .unwrap();
        let block_segment_postings = BlockSegmentPostings::open(
            doc_and_tfs.len() as u32,
            FileSlice::from(buffer),
            IndexRecordOption::WithFreqs,
            IndexRecordOption::WithFreqs,
        )
        .unwrap();
        SegmentPostings::from_block_postings(block_segment_postings, None)
    }

    /// Reads a Segment postings from an &[u8]
    ///
    /// * `len` - number of document in the posting lists.
    /// * `data` - data array. The complete data is not necessarily used.
    /// * `freq_handler` - the freq handler is in charge of decoding frequencies and/or positions
    pub(crate) fn from_block_postings(
        segment_block_postings: BlockSegmentPostings,
        position_reader: Option<PositionReader>,
    ) -> SegmentPostings {
        SegmentPostings {
            block_cursor: segment_block_postings,
            cur: 0, // cursor within the block
            position_reader,
        }
    }
}

impl DocSet for SegmentPostings {
    // goes to the next element.
    // next needs to be called a first time to point to the correct element.
    #[inline]
    fn advance(&mut self) -> DocId {
        debug_assert!(self.block_cursor.block_is_loaded());
        if self.cur == COMPRESSION_BLOCK_SIZE - 1 {
            self.cur = 0;
            self.block_cursor.advance();
        } else {
            self.cur += 1;
        }
        self.doc()
    }

    #[inline]
    fn seek(&mut self, target: DocId) -> DocId {
        debug_assert!(self.doc() <= target);
        if self.doc() >= target {
            return self.doc();
        }

        // As an optimization, if the block is already loaded, we can
        // cheaply check the next doc.
        self.cur = (self.cur + 1).min(COMPRESSION_BLOCK_SIZE - 1);
        if self.doc() >= target {
            return self.doc();
        }

        // Delegate block-local search to BlockSegmentPostings::seek, which returns
        // the in-block index of the first doc >= target.
        self.cur = self.block_cursor.seek(target);
        let doc = self.doc();
        debug_assert!(doc >= target);
        doc
    }

    fn fill_bitset_block(
        &mut self,
        min_doc: DocId,
        mask: &mut [TinySet; BLOCK_NUM_TINYBITSETS],
    ) -> DocId {
        self.seek(min_doc);
        let horizon = min_doc + BLOCK_WINDOW;
        let mut cur = self.cur;
        loop {
            let doc = self.block_cursor.doc(cur);
            if doc >= horizon {
                self.cur = cur;
                return doc;
            }
            let delta = doc - min_doc;
            mask[(delta / 64) as usize].insert_mut(delta % 64);
            cur += 1;
            if cur == COMPRESSION_BLOCK_SIZE {
                cur = 0;
                self.block_cursor.advance();
            }
        }
    }

    /// Return the current document's `DocId`.
    #[inline]
    fn doc(&self) -> DocId {
        self.block_cursor.doc(self.cur)
    }

    fn size_hint(&self) -> u32 {
        self.len() as u32
    }
}

impl HasLen for SegmentPostings {
    fn len(&self) -> usize {
        self.block_cursor.doc_freq() as usize
    }
}

impl Postings for SegmentPostings {
    /// Returns the frequency associated with the current document.
    /// If the schema is set up so that no frequency have been encoded,
    /// this method should always return 1.
    ///
    /// # Panics
    ///
    /// Will panics if called without having called advance before.
    fn term_freq(&self) -> u32 {
        debug_assert!(
            // Here we do not use the len of `freqs()`
            // because it is actually ok to request for the freq of doc
            // even if no frequency were encoded for the field.
            //
            // In that case we hit the block just as if the frequency had been
            // decoded. The block is simply prefilled by the value 1.
            self.cur < COMPRESSION_BLOCK_SIZE,
            "Have you forgotten to call `.advance()` at least once before calling `.term_freq()`."
        );
        self.block_cursor.freq(self.cur)
    }

    fn append_positions_with_offset(&mut self, offset: u32, output: &mut Vec<u32>) {
        let term_freq = self.term_freq();
        let prev_len = output.len();
        if let Some(position_reader) = self.position_reader.as_mut() {
            debug_assert!(
                !self.block_cursor.freqs().is_empty(),
                "No positions available"
            );
            let read_offset = self.block_cursor.position_offset()
                + (self.block_cursor.freqs()[..self.cur]
                    .iter()
                    .cloned()
                    .sum::<u32>() as u64);
            // TODO: instead of zeroing the output, we could use MaybeUninit or similar.
            output.resize(prev_len + term_freq as usize, 0u32);
            position_reader.read(read_offset, &mut output[prev_len..]);
            let mut cum = offset;
            for output_mut in output[prev_len..].iter_mut() {
                cum += *output_mut;
                *output_mut = cum;
            }
        }
    }
}

#[cfg(test)]
mod tests {

    use common::{HasLen, TinySet};

    use super::SegmentPostings;
    use crate::docset::{DocSet, BLOCK_NUM_TINYBITSETS, COLLECT_BLOCK_BUFFER_LEN, TERMINATED};
    use crate::fastfield::AliveBitSet;
    use crate::postings::postings::Postings;
    use crate::query::VecDocSet;

    #[test]
    fn test_empty_segment_postings() {
        let mut postings = SegmentPostings::empty();
        assert_eq!(postings.advance(), TERMINATED);
        assert_eq!(postings.advance(), TERMINATED);
        assert_eq!(postings.len(), 0);
    }

    #[test]
    fn test_empty_postings_doc_returns_terminated() {
        let mut postings = SegmentPostings::empty();
        assert_eq!(postings.doc(), TERMINATED);
        assert_eq!(postings.advance(), TERMINATED);
    }

    #[test]
    fn test_empty_postings_doc_term_freq_returns_0() {
        let postings = SegmentPostings::empty();
        assert_eq!(postings.term_freq(), 1);
    }

    #[test]
    fn test_doc_freq() {
        let docs = SegmentPostings::create_from_docs(&[0, 2, 10]);
        assert_eq!(docs.doc_freq(), 3);
        let alive_bitset = AliveBitSet::for_test_from_deleted_docs(&[2], 12);
        assert_eq!(docs.doc_freq_given_deletes(&alive_bitset), 2);
        let all_deleted =
            AliveBitSet::for_test_from_deleted_docs(&[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11], 12);
        assert_eq!(docs.doc_freq_given_deletes(&all_deleted), 0);
    }

    #[test]
    fn test_fill_bitset_block() {
        let docs: Vec<u32> = (0..2_500).step_by(3).collect();
        for min_doc in [0, 1, 63, 64, 65, 127, 128, 1_000, 2_000, 2_500] {
            let mut postings = SegmentPostings::create_from_docs(&docs);
            let mut vec_docset = VecDocSet::from(docs.clone());
            let mut actual_mask = [TinySet::EMPTY; BLOCK_NUM_TINYBITSETS];
            let mut expected_mask = [TinySet::EMPTY; BLOCK_NUM_TINYBITSETS];
            let actual_next = postings.fill_bitset_block(min_doc, &mut actual_mask);
            let expected_next = vec_docset.fill_bitset_block(min_doc, &mut expected_mask);
            assert_eq!(actual_mask, expected_mask);
            assert_eq!(actual_next, expected_next);
            assert_eq!(postings.doc(), vec_docset.doc());
        }

        let mut postings = SegmentPostings::create_from_docs(&docs);
        let mut vec_docset = VecDocSet::from(docs);
        let mut min_doc = 0;
        loop {
            let mut actual_mask = [TinySet::EMPTY; BLOCK_NUM_TINYBITSETS];
            let mut expected_mask = [TinySet::EMPTY; BLOCK_NUM_TINYBITSETS];
            let actual_next = postings.fill_bitset_block(min_doc, &mut actual_mask);
            let expected_next = vec_docset.fill_bitset_block(min_doc, &mut expected_mask);
            assert_eq!(actual_mask, expected_mask);
            assert_eq!(actual_next, expected_next);
            assert_eq!(postings.doc(), vec_docset.doc());
            if actual_next == TERMINATED {
                break;
            }
            min_doc = actual_next;
        }
    }

    #[test]
    fn test_fill_buffer() {
        let docs: Vec<u32> = (0..2_500).step_by(3).collect();
        let mut postings = SegmentPostings::create_from_docs(&docs);
        let mut vec_docset = VecDocSet::from(docs);
        let mut actual_buffer = [0u32; COLLECT_BLOCK_BUFFER_LEN];
        let mut expected_buffer = [0u32; COLLECT_BLOCK_BUFFER_LEN];
        loop {
            let actual_len = postings.fill_buffer(&mut actual_buffer);
            let expected_len = vec_docset.fill_buffer(&mut expected_buffer);
            assert_eq!(actual_len, expected_len);
            assert_eq!(actual_buffer, expected_buffer);
            assert_eq!(postings.doc(), vec_docset.doc());
            if actual_len < COLLECT_BLOCK_BUFFER_LEN {
                break;
            }
        }
        assert_eq!(postings.fill_buffer(&mut actual_buffer), 0);
    }
}
