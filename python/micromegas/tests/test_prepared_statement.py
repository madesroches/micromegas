from .test_utils import *


def test_prepared_statement():
    sql = "select count(*) from log_entries"
    prepared_statement = client.prepare_statement(sql)
    print(prepared_statement.query)  # b'select count(*) from log_entries'
    print(
        type(prepared_statement.dataset_schema), prepared_statement.dataset_schema
    )  # <class 'pyarrow.lib.Schema'> count(*): int64 not null
    # todo: execute prepared statement
