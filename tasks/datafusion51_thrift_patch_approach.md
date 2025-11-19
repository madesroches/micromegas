# DataFusion 51.0: Thrift Patching Investigation

## Problem

The `parquet::format` module (with thrift types) is **deprecated in 57.0.0 and will be removed in 59.0.0**.

This means we cannot rely on low-level thrift manipulation as a long-term solution.

## Key Findings

1. ✅ We have `num_rows` in `lakehouse_partitions` table
2. ❌ The `parquet::format` module is deprecated - not a stable API
3. ❌ Direct thrift manipulation is complex and brittle
4. ✅ Arrow 57.0's new parser is 3-4x faster (we want to use it)

## The Real Question

**Why doesn't the old metadata have `num_rows`?**

Let me check what version of arrow was used when we serialized the metadata, and whether the `num_rows` field was optional back then.

## Investigation Needed

1. Check what `serialize_parquet_metadata` does - does it serialize `num_rows`?
2. Check the old arrow version's thrift definition
3. Understand why `num_rows` is missing from our stored metadata

## Hypothesis

The old version (Arrow 56.0 or earlier) may have had `num_rows` as **optional** in the thrift definition, and our serialization code didn't include it.

The new version (Arrow 57.0) made it **required** for correctness.

## Next Steps

1. Examine how we currently serialize metadata (`serialize_parquet_metadata`)
2. Check if we're actually writing `num_rows` when creating new metadata
3. If we ARE writing it now, then only OLD metadata needs migration
4. If we're NOT writing it, we need to fix the serialization first

Then we can choose:
- **Option A**: Lazy migration (fallback to object storage)
- **Option B**: One-time migration script
- **Option C**: SQL update if `num_rows` is in `lakehouse_partitions` and metadata just needs patching at SQL level

Let me investigate the actual serialization code...
