use crate::arrow_properties::serialize_property_set_to_jsonb;
use crate::properties::property_set::PropertySet;
use anyhow::Result;
use datafusion::arrow::array::{BinaryArray, DictionaryArray, Int32Array};
use datafusion::arrow::datatypes::Int32Type;
use datafusion::common::DataFusionError;
use std::collections::HashMap;
use std::sync::Arc;

/// A wrapper around raw pointers that implements Send/Sync for use in HashMap keys.
///
/// This is safe because:
/// 1. We only use the pointer for identity comparison (equality/hashing)
/// 2. We never dereference the pointer
/// 3. The parse arena keeps the underlying objects alive for the whole block,
///    so addresses stay stable and unique while comparisons happen; after the
///    block is parsed the pointers are never touched again.
/// 4. The cache is scoped to single block processing (no cross-thread sharing)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ObjectPointer(*const ());

unsafe impl Send for ObjectPointer {}
unsafe impl Sync for ObjectPointer {}

/// Custom dictionary builder for PropertySet → JSONB encoding with pointer-based deduplication.
///
/// This builder eliminates redundant JSONB serialization and dictionary hash lookups
/// for duplicate PropertySets by using PropertySet's `Arc<Object>` pointer addresses as keys.
///
/// Performance benefits over Arrow's BinaryDictionaryBuilder:
/// - Eliminates content-based hashing: Arrow's builder hashes JSONB bytes for deduplication
/// - Pointer-based deduplication: O(1) pointer comparison vs O(n) content hash
/// - Serialization only when needed: Only serialize PropertySet on first encounter
/// - Memory efficiency: Shared PropertySet references, single JSONB copy per unique set
///
/// # Invariant (must not outlive a single parse arena)
///
/// The pointer keys are only unique while every appended `PropertySet` borrows the
/// same parse arena. A dropped arena's address can be recycled by the next block's
/// arena, so a builder instance must be fed exactly one block / one arena: construct
/// it fresh in each block processor and `finish()` it before the arena is dropped.
/// Reusing one builder across arenas would let a recycled address alias a stale
/// entry and emit the wrong JSONB. `append_property_set` debug-asserts this
/// invariant so any future cross-arena reuse fails loudly in debug/test builds.
pub struct PropertySetJsonbDictionaryBuilder {
    /// Maps `Arc<Object>` pointer to dictionary index (avoids content hashing)
    pointer_to_index: HashMap<ObjectPointer, i32>,
    /// Pre-serialized JSONB values in dictionary
    jsonb_values: Vec<Vec<u8>>,
    /// Dictionary keys (indices) for each appended entry
    keys: Vec<Option<i32>>,
}

impl PropertySetJsonbDictionaryBuilder {
    /// Create a new builder with the specified capacity hint
    pub fn new(capacity: usize) -> Self {
        Self {
            pointer_to_index: HashMap::with_capacity(capacity),
            jsonb_values: Vec::with_capacity(capacity),
            keys: Vec::with_capacity(capacity),
        }
    }

    /// Append PropertySet using pointer-based deduplication
    ///
    /// For cache hits: reuses existing dictionary index (no serialization)
    /// For cache misses: serializes once and stores in dictionary
    pub fn append_property_set(&mut self, property_set: &PropertySet<'_>) -> Result<()> {
        let ptr = ObjectPointer(property_set.object_ptr());

        match self.pointer_to_index.get(&ptr) {
            Some(&index) => {
                // Cache hit: reuse existing dictionary index (no serialization).
                // Invariant: an equal pointer must mean equal content. This holds only
                // while all appended sets come from the same parse arena; if a builder
                // is ever reused across arenas, a recycled address could alias a stale
                // entry here. Verify in debug builds so that misuse fails loudly instead
                // of silently emitting the wrong JSONB. Compiled out of release builds.
                #[cfg(debug_assertions)]
                {
                    let expected = serialize_property_set_to_jsonb(property_set)?;
                    debug_assert_eq!(
                        expected, self.jsonb_values[index as usize],
                        "pointer-dedup collision: arena address reused across blocks"
                    );
                }
                self.keys.push(Some(index));
            }
            None => {
                // Cache miss: serialize once and store in dictionary
                let jsonb_bytes = serialize_property_set_to_jsonb(property_set)?;
                let new_index = self.jsonb_values.len() as i32;

                self.jsonb_values.push(jsonb_bytes);
                self.pointer_to_index.insert(ptr, new_index);
                self.keys.push(Some(new_index));
            }
        }
        Ok(())
    }

    /// Append a null value
    pub fn append_null(&mut self) {
        self.keys.push(None);
    }

    /// Finish building and return the DictionaryArray
    ///
    /// Output is identical to Arrow's BinaryDictionaryBuilder for compatibility
    pub fn finish(self) -> Result<DictionaryArray<Int32Type>> {
        let keys = Int32Array::from(self.keys);
        // Convert Vec<Vec<u8>> to Vec<&[u8]> for BinaryArray::from_vec
        let byte_slices: Vec<&[u8]> = self.jsonb_values.iter().map(|v| v.as_slice()).collect();
        let values = Arc::new(BinaryArray::from_vec(byte_slices));
        DictionaryArray::try_new(keys, values)
            .map_err(|e| DataFusionError::ArrowError(Box::new(e), None).into())
    }

    /// Get the current number of appended entries
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// Check if the builder is empty
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
}
