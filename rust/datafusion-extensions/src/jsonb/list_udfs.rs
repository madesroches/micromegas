//! List-returning JSONB scalar UDFs: `jsonb_entries`, `jsonb_elements`, `jsonb_path_elements`.
//!
//! Unlike the other scalar JSONB UDFs, these return a plain Arrow `List` (never
//! `Dictionary<Int32, List<…>>`), because `unnest()` cannot expand a dictionary-wrapped list —
//! see the plan this module implements. `properties` columns are dictionary-encoded, so a
//! dictionary fast path builds the list once per unique blob and expands it to row count via
//! `take`, instead of re-parsing the same bytes on every row.

use crate::binary_column_accessor::create_binary_accessor;
use crate::jsonb::extract::{EntryList, array_elements, object_or_array_entries, path_select_all};
use datafusion::arrow::array::{
    Array, ArrayBuilder, ArrayRef, BinaryBuilder, DictionaryArray, GenericBinaryArray, ListBuilder,
    StringArray, StringBuilder, StructBuilder,
};
use datafusion::arrow::compute::take;
use datafusion::arrow::datatypes::{DataType, Field, Fields, Int32Type};
use datafusion::common::{Result, exec_err};
use datafusion::error::DataFusionError;
use datafusion::logical_expr::{
    ColumnarValue, ScalarFunctionArgs, ScalarUDF, ScalarUDFImpl, Signature, Volatility,
};
use jsonb::jsonpath::{JsonPath, parse_json_path};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

// --- shared field/type helpers ---

fn entry_struct_fields() -> Vec<Field> {
    vec![
        Field::new("key", DataType::Utf8, false),
        Field::new("value", DataType::Binary, true),
    ]
}

fn entries_list_field() -> Arc<Field> {
    Arc::new(Field::new_list_field(
        DataType::Struct(Fields::from(entry_struct_fields())),
        false,
    ))
}

fn entries_return_type() -> DataType {
    DataType::List(entries_list_field())
}

fn binary_list_field() -> Arc<Field> {
    Arc::new(Field::new_list_field(DataType::Binary, true))
}

fn binary_list_return_type() -> DataType {
    DataType::List(binary_list_field())
}

// --- shared dictionary fast path ---

/// The distinct `dict.values()` indices referenced by a non-null key in this batch.
fn referenced_value_indices(dict_array: &DictionaryArray<Int32Type>) -> HashSet<i32> {
    let keys = dict_array.keys();
    (0..keys.len())
        .filter(|&i| !keys.is_null(i))
        .map(|i| keys.value(i))
        .collect()
}

fn dict_values_binary_array(
    dict_array: &DictionaryArray<Int32Type>,
) -> Result<&GenericBinaryArray<i32>> {
    dict_array
        .values()
        .as_any()
        .downcast_ref::<GenericBinaryArray<i32>>()
        .ok_or_else(|| DataFusionError::Internal("dictionary values are not a binary array".into()))
}

/// Build a plain `List` array positionally aligned with `dict_array.values()` — one entry per
/// value slot, parsing only the slots referenced by a non-null key in this batch (gating on key
/// nullity and referenced-index membership only, not `values.is_null`, matching
/// `DictionaryBinaryAccessor` and the row-by-row path) — then expand it to row count via `take`.
///
/// `append_slot` is called only for referenced slots; it must call `list_builder.append(true)` or
/// `list_builder.append(false)` exactly once (via the underlying `ListBuilder`, reached through
/// its `&mut ListBuilder<T>` argument).
fn dict_fast_path<T, F>(
    dict_array: &DictionaryArray<Int32Type>,
    mut list_builder: ListBuilder<T>,
    mut append_slot: F,
) -> Result<ArrayRef>
where
    T: ArrayBuilder,
    F: FnMut(&mut ListBuilder<T>, &[u8]) -> Result<()>,
{
    let binary_values = dict_values_binary_array(dict_array)?;
    let referenced = referenced_value_indices(dict_array);
    for idx in 0..binary_values.len() {
        if referenced.contains(&(idx as i32)) {
            append_slot(&mut list_builder, binary_values.value(idx))?;
        } else {
            list_builder.append(false);
        }
    }
    let list_array = list_builder.finish();
    let taken = take(&list_array, dict_array.keys(), None)
        .map_err(|e| DataFusionError::ArrowError(Box::new(e), None))?;
    Ok(taken)
}

fn append_binary_slot(list_builder: &mut ListBuilder<BinaryBuilder>, items: Option<Vec<Vec<u8>>>) {
    match items {
        Some(values) => {
            for v in values {
                list_builder.values().append_value(v);
            }
            list_builder.append(true);
        }
        None => list_builder.append(false),
    }
}

fn append_entries_to_struct_builder(
    struct_builder: &mut StructBuilder,
    entries: EntryList,
) -> Result<()> {
    for (key, value) in entries {
        struct_builder
            .field_builder::<StringBuilder>(0)
            .ok_or_else(|| {
                DataFusionError::Internal("failed to get jsonb_entries key builder".into())
            })?
            .append_value(&key);
        struct_builder
            .field_builder::<BinaryBuilder>(1)
            .ok_or_else(|| {
                DataFusionError::Internal("failed to get jsonb_entries value builder".into())
            })?
            .append_value(&value);
        struct_builder.append(true);
    }
    Ok(())
}

fn append_entries_slot(
    list_builder: &mut ListBuilder<StructBuilder>,
    entries: Option<EntryList>,
) -> Result<()> {
    match entries {
        Some(entries) => {
            append_entries_to_struct_builder(list_builder.values(), entries)?;
            list_builder.append(true);
        }
        None => list_builder.append(false),
    }
    Ok(())
}

/// Build `jsonb_entries`'/`jsonb_elements`'s list array: dictionary fast path when the input is
/// `Dictionary<Int32, Binary>`, row-by-row via `create_binary_accessor` otherwise.
fn build_binary_list<F>(array: &ArrayRef, func_name: &str, mut extract: F) -> Result<ArrayRef>
where
    F: FnMut(&[u8]) -> Result<Option<Vec<Vec<u8>>>>,
{
    if let DataType::Dictionary(_, value_type) = array.data_type()
        && matches!(value_type.as_ref(), DataType::Binary)
    {
        let dict_array = array
            .as_any()
            .downcast_ref::<DictionaryArray<Int32Type>>()
            .ok_or_else(|| DataFusionError::Internal("error casting dictionary array".into()))?;
        let list_builder = ListBuilder::new(BinaryBuilder::new()).with_field(binary_list_field());
        return dict_fast_path(dict_array, list_builder, |builder, bytes| {
            append_binary_slot(builder, extract(bytes)?);
            Ok(())
        });
    }

    let accessor = create_binary_accessor(array).map_err(|e| {
        DataFusionError::Execution(format!(
            "Invalid input type for {func_name}: {e}. Expected Binary or Dictionary<Int32, Binary>"
        ))
    })?;
    let mut list_builder = ListBuilder::new(BinaryBuilder::new()).with_field(binary_list_field());
    for i in 0..accessor.len() {
        if accessor.is_null(i) {
            list_builder.append(false);
        } else {
            append_binary_slot(&mut list_builder, extract(accessor.value(i))?);
        }
    }
    Ok(Arc::new(list_builder.finish()))
}

fn build_entries_list(array: &ArrayRef) -> Result<ArrayRef> {
    if let DataType::Dictionary(_, value_type) = array.data_type()
        && matches!(value_type.as_ref(), DataType::Binary)
    {
        let dict_array = array
            .as_any()
            .downcast_ref::<DictionaryArray<Int32Type>>()
            .ok_or_else(|| DataFusionError::Internal("error casting dictionary array".into()))?;
        let capacity = dict_array.values().len();
        let list_builder =
            ListBuilder::new(StructBuilder::from_fields(entry_struct_fields(), capacity))
                .with_field(entries_list_field());
        return dict_fast_path(dict_array, list_builder, |builder, bytes| {
            append_entries_slot(builder, object_or_array_entries(bytes)?)
        });
    }

    let accessor = create_binary_accessor(array).map_err(|e| {
        DataFusionError::Execution(format!(
            "Invalid input type for jsonb_entries: {e}. Expected Binary or Dictionary<Int32, Binary>"
        ))
    })?;
    let mut list_builder = ListBuilder::new(StructBuilder::from_fields(
        entry_struct_fields(),
        accessor.len(),
    ))
    .with_field(entries_list_field());
    for i in 0..accessor.len() {
        if accessor.is_null(i) {
            list_builder.append(false);
        } else {
            append_entries_slot(
                &mut list_builder,
                object_or_array_entries(accessor.value(i))?,
            )?;
        }
    }
    Ok(Arc::new(list_builder.finish()))
}

// --- jsonb_entries ---

/// A scalar UDF that expands a JSONB object or array into a list of `{key, value}` entries.
///
/// Accepts both `Binary` and `Dictionary<Int32, Binary>` inputs. For objects, `key` is the field
/// name; for arrays, `key` is the element index as a string. Returns NULL for a JSON scalar or a
/// NULL input (zero rows under `unnest`, same as an empty list); errors if the JSONB bytes fail
/// to decode.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct JsonbEntries {
    signature: Signature,
}

impl JsonbEntries {
    pub fn new() -> Self {
        Self {
            signature: Signature::any(1, Volatility::Immutable),
        }
    }
}

impl Default for JsonbEntries {
    fn default() -> Self {
        Self::new()
    }
}

impl ScalarUDFImpl for JsonbEntries {
    fn name(&self) -> &str {
        "jsonb_entries"
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn return_type(&self, _arg_types: &[DataType]) -> Result<DataType> {
        Ok(entries_return_type())
    }

    fn invoke_with_args(&self, args: ScalarFunctionArgs) -> Result<ColumnarValue> {
        let args = ColumnarValue::values_to_arrays(&args.args)?;
        if args.len() != 1 {
            return exec_err!("wrong number of arguments to jsonb_entries()");
        }
        Ok(ColumnarValue::Array(build_entries_list(&args[0])?))
    }
}

/// Creates a user-defined function that expands a JSONB object or array into `{key, value}` entries.
pub fn make_jsonb_entries_udf() -> ScalarUDF {
    ScalarUDF::new_from_impl(JsonbEntries::new())
}

// --- jsonb_elements ---

/// A scalar UDF that expands a JSONB array into a list of its elements.
///
/// Accepts both `Binary` and `Dictionary<Int32, Binary>` inputs. Returns NULL for a JSON object
/// or scalar, or for a NULL input (zero rows under `unnest`, same as an empty array); errors if
/// the JSONB bytes fail to decode.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct JsonbElements {
    signature: Signature,
}

impl JsonbElements {
    pub fn new() -> Self {
        Self {
            signature: Signature::any(1, Volatility::Immutable),
        }
    }
}

impl Default for JsonbElements {
    fn default() -> Self {
        Self::new()
    }
}

impl ScalarUDFImpl for JsonbElements {
    fn name(&self) -> &str {
        "jsonb_elements"
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn return_type(&self, _arg_types: &[DataType]) -> Result<DataType> {
        Ok(binary_list_return_type())
    }

    fn invoke_with_args(&self, args: ScalarFunctionArgs) -> Result<ColumnarValue> {
        let args = ColumnarValue::values_to_arrays(&args.args)?;
        if args.len() != 1 {
            return exec_err!("wrong number of arguments to jsonb_elements()");
        }
        Ok(ColumnarValue::Array(build_binary_list(
            &args[0],
            "jsonb_elements",
            array_elements,
        )?))
    }
}

/// Creates a user-defined function that expands a JSONB array into its elements.
pub fn make_jsonb_elements_udf() -> ScalarUDF {
    ScalarUDF::new_from_impl(JsonbElements::new())
}

// --- jsonb_path_elements ---

/// Determine whether every row's `path` value is non-null and equal to the same string — the
/// only case where a per-unique-blob dictionary shortcut can apply without per-row path nullity
/// to reconcile against a per-unique-blob result. Returns `None` (disqualifying the fast path) as
/// soon as a NULL or a differing value is seen, including when the array is empty.
fn constant_non_null_path(paths: &StringArray) -> Option<&str> {
    let mut constant: Option<&str> = None;
    for i in 0..paths.len() {
        if paths.is_null(i) {
            return None;
        }
        let v = paths.value(i);
        match constant {
            None => constant = Some(v),
            Some(c) if c == v => {}
            Some(_) => return None,
        }
    }
    constant
}

/// Dictionary fast path for a `path` that is constant and non-null across the whole batch: parse
/// the path lazily, only once the first referenced slot needs it (so an all-null-key dictionary
/// never parses `path` at all, matching `eval_jsonb_path_query`'s behavior when no row has both a
/// non-null JSONB and a non-null path), then reuse the parsed `JsonPath` for every subsequent
/// unique blob.
fn dict_fast_path_path_elements<'p>(
    dict_array: &DictionaryArray<Int32Type>,
    path_str: &'p str,
) -> Result<ArrayRef> {
    let list_builder = ListBuilder::new(BinaryBuilder::new()).with_field(binary_list_field());
    let mut parsed_path: Option<JsonPath<'p>> = None;
    dict_fast_path(dict_array, list_builder, |builder, bytes| {
        if parsed_path.is_none() {
            let parsed = parse_json_path(path_str.as_bytes()).map_err(|e| {
                DataFusionError::Execution(format!(
                    "jsonb_path_elements: invalid JSONPath '{path_str}': {e}"
                ))
            })?;
            parsed_path = Some(parsed);
        }
        let json_path = parsed_path.as_ref().expect("just set");
        let items = path_select_all(bytes, json_path)?;
        for item in items {
            builder.values().append_value(item);
        }
        builder.append(true);
        Ok(())
    })
}

/// A scalar UDF that expands all matches of a JSONPath expression on a JSONB value into a list.
///
/// Accepts both `Binary` and `Dictionary<Int32, Binary>` for the JSONB argument; `path` is
/// `Utf8`. A NULL `path` produces a NULL list for that row (matching `jsonb_path_query`). A path
/// with zero matches produces an empty list, not NULL.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct JsonbPathElements {
    signature: Signature,
}

impl JsonbPathElements {
    pub fn new() -> Self {
        Self {
            signature: Signature::any(2, Volatility::Immutable),
        }
    }
}

impl Default for JsonbPathElements {
    fn default() -> Self {
        Self::new()
    }
}

impl ScalarUDFImpl for JsonbPathElements {
    fn name(&self) -> &str {
        "jsonb_path_elements"
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn return_type(&self, _arg_types: &[DataType]) -> Result<DataType> {
        Ok(binary_list_return_type())
    }

    fn invoke_with_args(&self, args: ScalarFunctionArgs) -> Result<ColumnarValue> {
        let args = ColumnarValue::values_to_arrays(&args.args)?;
        if args.len() != 2 {
            return exec_err!("wrong number of arguments to jsonb_path_elements()");
        }

        let paths = args[1]
            .as_any()
            .downcast_ref::<StringArray>()
            .ok_or_else(|| {
                DataFusionError::Execution(
                    "second argument to jsonb_path_elements must be a string".into(),
                )
            })?;

        if let DataType::Dictionary(_, value_type) = args[0].data_type()
            && matches!(value_type.as_ref(), DataType::Binary)
            && let Some(constant_path) = constant_non_null_path(paths)
        {
            let dict_array = args[0]
                .as_any()
                .downcast_ref::<DictionaryArray<Int32Type>>()
                .ok_or_else(|| {
                    DataFusionError::Internal("error casting dictionary array".into())
                })?;
            return Ok(ColumnarValue::Array(dict_fast_path_path_elements(
                dict_array,
                constant_path,
            )?));
        }

        let accessor = create_binary_accessor(&args[0]).map_err(|e| {
            DataFusionError::Execution(format!(
                "Invalid input type for jsonb_path_elements: {e}. Expected Binary or Dictionary<Int32, Binary>"
            ))
        })?;
        let mut list_builder =
            ListBuilder::new(BinaryBuilder::new()).with_field(binary_list_field());
        let mut path_cache: HashMap<&str, JsonPath> = HashMap::new();
        for i in 0..accessor.len() {
            if accessor.is_null(i) || paths.is_null(i) {
                list_builder.append(false);
                continue;
            }
            let path_str = paths.value(i);
            if !path_cache.contains_key(path_str) {
                let parsed = parse_json_path(path_str.as_bytes()).map_err(|e| {
                    DataFusionError::Execution(format!(
                        "jsonb_path_elements: invalid JSONPath '{path_str}': {e}"
                    ))
                })?;
                path_cache.insert(path_str, parsed);
            }
            let json_path = path_cache.get(path_str).expect("just inserted");
            let items = path_select_all(accessor.value(i), json_path)?;
            for item in items {
                list_builder.values().append_value(item);
            }
            list_builder.append(true);
        }
        Ok(ColumnarValue::Array(Arc::new(list_builder.finish())))
    }
}

/// Creates a user-defined function that expands all JSONPath matches from a JSONB value into a list.
pub fn make_jsonb_path_elements_udf() -> ScalarUDF {
    ScalarUDF::new_from_impl(JsonbPathElements::new())
}
