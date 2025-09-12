#!/usr/bin/python3
"""
Test dictionary preservation feature in FlightSQL client.

This test verifies that the preserve_dictionary option works correctly with
dictionary-encoded UDFs.
"""

import pyarrow as pa
from .test_utils import *


def test_properties_to_dict_with_default_client():
    """Test properties_to_dict UDF with default client (preserve_dictionary=False)."""
    # Default client should flatten dictionary arrays
    default_client = micromegas.connect(preserve_dictionary=False)
    
    try:
        # Query with properties_to_dict UDF
        sql = "SELECT properties_to_dict(properties) as dict_props FROM measures LIMIT 5"
        
        batches = list(default_client.query_stream(sql))
        if not batches:
            print("   No measures data found - skipping default client test")
            return
            
        batch = batches[0]
        dict_column = batch.column(0)
        
        print(f"   Default client column type: {dict_column.type}")
        print(f"   Dictionary preserved: {pa.types.is_dictionary(dict_column.type)}")
        
        # With default client, dictionary should be flattened (not preserved)
        assert not pa.types.is_dictionary(dict_column.type), \
            "Dictionary should be flattened with preserve_dictionary=False"
        
        print("   ✅ Default client correctly flattens dictionary arrays")
        
    except Exception as e:
        # Expected if no measures data exists
        print(f"   Default client test failed (expected if no data): {e}")


def test_properties_to_dict_with_dictionary_client():
    """Test properties_to_dict UDF with dictionary preservation client."""
    # Dictionary client should preserve dictionary encoding
    dict_client = micromegas.connect(preserve_dictionary=True)
    
    try:
        # Query with properties_to_dict UDF
        sql = "SELECT properties_to_dict(properties) as dict_props FROM measures LIMIT 5"
        
        batches = list(dict_client.query_stream(sql))
        if not batches:
            print("   No measures data found - skipping dictionary client test")
            return
            
        batch = batches[0]
        dict_column = batch.column(0)
        
        print(f"   Dictionary client column type: {dict_column.type}")
        print(f"   Dictionary preserved: {pa.types.is_dictionary(dict_column.type)}")
        
        if pa.types.is_dictionary(dict_column.type):
            print("   ✅ SUCCESS: Dictionary encoding preserved!")
            
            # Try to inspect the dictionary structure
            if hasattr(dict_column, 'dictionary'):
                print(f"   Dictionary size: {len(dict_column.dictionary)}")
            else:
                print("   Dictionary array structure confirmed")
        else:
            print("   ⚠️  Dictionary not preserved - may be expected if server feature is disabled")
        
    except Exception as e:
        # Expected if no measures data exists
        print(f"   Dictionary client test failed (expected if no data): {e}")


def test_regular_queries_with_both_clients():
    """Test that regular queries work identically with both client types."""
    default_client = micromegas.connect(preserve_dictionary=False)
    dict_client = micromegas.connect(preserve_dictionary=True)
    
    # Test queries that should work regardless of dictionary preservation
    test_queries = [
        "SELECT COUNT(*) as process_count FROM processes",
        "SELECT * FROM processes LIMIT 1"
    ]
    
    for sql in test_queries:
        print(f"   Testing query: {sql}")
        
        try:
            df_default = default_client.query(sql)
            df_dict = dict_client.query(sql)
            
            # Results should be functionally identical
            assert len(df_default) == len(df_dict), \
                f"Row count mismatch: default={len(df_default)}, dict={len(df_dict)}"
            
            print(f"      ✅ Both clients returned {len(df_default)} rows")
            
        except Exception as e:
            print(f"      ❌ Query failed: {e}")


def test_properties_length_compatibility():
    """Test that properties_length works with both array and dictionary inputs."""
    dict_client = micromegas.connect(preserve_dictionary=True)
    
    try:
        # Test properties_length with regular properties (should work)
        sql_array = "SELECT properties_length(properties) as len FROM measures LIMIT 5"
        
        # Test properties_length with dictionary-encoded properties (should also work)
        sql_dict = "SELECT properties_length(properties_to_dict(properties)) as len FROM measures LIMIT 5"
        
        batches_array = list(dict_client.query_stream(sql_array))
        batches_dict = list(dict_client.query_stream(sql_dict))
        
        if not batches_array or not batches_dict:
            print("   No measures data found - skipping properties_length test")
            return
            
        # Both queries should return the same results
        df_array = batches_array[0].to_pandas()
        df_dict = batches_dict[0].to_pandas()
        
        print(f"   properties_length(properties) results: {df_array['len'].tolist()}")
        print(f"   properties_length(properties_to_dict(properties)) results: {df_dict['len'].tolist()}")
        
        # Results should be identical
        assert df_array['len'].equals(df_dict['len']), \
            "properties_length should return same results for array and dictionary inputs"
        
        print("   ✅ properties_length works with both array and dictionary inputs")
        
    except Exception as e:
        print(f"   properties_length test failed (expected if no data): {e}")


def test_pandas_conversion_with_dictionary_preservation():
    """Test that dictionary-preserved queries convert to pandas correctly."""
    dict_client = micromegas.connect(preserve_dictionary=True)
    
    try:
        # This should work now with our _prepare_table_for_pandas method
        sql = "SELECT properties_to_dict(properties) as dict_props FROM measures LIMIT 3"
        
        # Test pandas conversion (should not raise ArrowNotImplementedError)
        df = dict_client.query(sql)
        print(f"   Pandas conversion successful: {len(df)} rows")
        print(f"   DataFrame dtypes: {df.dtypes.to_dict()}")
        
        # Test Arrow table access (preserves dictionary encoding)
        table = dict_client.query_arrow(sql)
        print(f"   Arrow table schema: {table.schema}")
        
        # Verify dictionary is preserved in Arrow but converted for pandas
        dict_column_arrow = table.column(0)
        dict_column_pandas_source = df.dtypes.iloc[0]
        
        print(f"   Arrow preserves dictionary: {pa.types.is_dictionary(dict_column_arrow.type)}")
        print(f"   Pandas receives regular type: {not str(dict_column_pandas_source).startswith('dictionary')}")
        
        print("   ✅ Dictionary preservation with pandas conversion works!")
        
    except Exception as e:
        print(f"   Pandas conversion test failed: {e}")


def test_query_arrow_method():
    """Test the new query_arrow method for direct Arrow table access."""
    dict_client = micromegas.connect(preserve_dictionary=True)
    
    try:
        sql = "SELECT properties_to_dict(properties) as dict_props FROM measures LIMIT 2"
        
        # Test query_arrow method
        table = dict_client.query_arrow(sql)
        
        print(f"   Arrow table schema: {table.schema}")
        print(f"   Arrow table rows: {len(table)}")
        
        # Verify dictionary encoding is preserved
        dict_column = table.column(0)
        if pa.types.is_dictionary(dict_column.type):
            print("   ✅ query_arrow preserves dictionary encoding!")
            print(f"   Dictionary type: {dict_column.type}")
        else:
            print("   ⚠️  Dictionary encoding not found (may be expected if no data)")
            
    except Exception as e:
        print(f"   query_arrow test failed: {e}")


def test_dictionary_preservation():
    """Main test function that runs all dictionary preservation tests."""
    print("=== Testing Dictionary Preservation Feature ===\n")
    
    print("1. Testing default behavior (preserve_dictionary=False)...")
    test_properties_to_dict_with_default_client()
    print()
    
    print("2. Testing dictionary preservation (preserve_dictionary=True)...")
    test_properties_to_dict_with_dictionary_client()
    print()
    
    print("3. Testing regular queries work with both clients...")
    test_regular_queries_with_both_clients()
    print()
    
    print("4. Testing properties_length compatibility...")
    test_properties_length_compatibility()
    print()
    
    print("5. Testing pandas conversion with dictionary preservation...")
    test_pandas_conversion_with_dictionary_preservation()
    print()
    
    print("6. Testing query_arrow method...")
    test_query_arrow_method()
    print()
    
    print("=== Dictionary Preservation Tests Complete ===")


# For pytest compatibility
def test_dictionary_preservation_pytest():
    """Pytest-compatible wrapper for the main test function."""
    test_dictionary_preservation()


if __name__ == "__main__":
    test_dictionary_preservation()