import pyarrow

# based on https://github.com/apache/arrow-rs/blob/main/arrow-ipc/src/reader.rs

class ArrayReader:
    def __init__(self, schema, nodes, buffers):
        self.schema = schema
        self.nodes = nodes
        self.current_node = 0
        self.buffers = buffers
        self.current_buffer = 0

    def next_node(self):
        assert self.current_node < len(self.nodes)
        node = self.nodes[self.current_node]
        self.current_node += 1
        return node

    def next_buffer(self):
        assert self.current_buffer < len(self.buffers)
        buffer = self.buffers[self.current_buffer]
        self.current_buffer += 1
        return buffer


def create_primitive_array(node, data_type, null_buffer, data_buffer):
    return pyarrow.NumericArray.from_buffers(
        data_type, node.Length(), [null_buffer, data_buffer], node.NullCount()
    )


def create_string_array(node, data_type, null_buffer, offset_buffer, data_buffer):
    return pyarrow.NumericArray.from_buffers(
        data_type,
        node.Length(),
        [null_buffer, offset_buffer, data_buffer],
        node.NullCount(),
    )


def read_column(reader, arrow_field):
    if arrow_field.type in [
        pyarrow.string(),
        pyarrow.binary(),
        pyarrow.large_binary(),
        pyarrow.large_string(),
    ]:
        return create_string_array(
            reader.next_node(),
            arrow_field.type,
            reader.next_buffer(),
            reader.next_buffer(),
            reader.next_buffer(),
        )
    elif pyarrow.types.is_primitive(arrow_field.type):
        return create_primitive_array(
            reader.next_node(),
            arrow_field.type,
            reader.next_buffer(),
            reader.next_buffer(),
        )
    elif pyarrow.types.is_list(arrow_field.type):
        list_node = reader.next_node()
        list_buffers = [reader.next_buffer(), reader.next_buffer()]
        values = read_column(reader, arrow_field.type.value_field)
        return pyarrow.ListArray.from_buffers(
            arrow_field.type,
            list_node.Length(),
            list_buffers,
            list_node.NullCount(),
            0,
            [values],
        )
    elif pyarrow.types.is_struct(arrow_field.type):
        struct_node = reader.next_node()
        null_buffer = reader.next_buffer()
        children = []
        for child_field in arrow_field.type.fields:
            child_column = read_column(reader, child_field)
            children.append(child_column)
        return pyarrow.StructArray.from_buffers(
            arrow_field.type,
            struct_node.Length(),
            [null_buffer],
            struct_node.NullCount(),
            0,
            children,
        )
    else:
        raise RuntimeError("unsupported arrow field type {}".format(arrow_field.type))


def read_record_batch(arrow_schema, nodes, buffers):
    reader = ArrayReader(arrow_schema, nodes, buffers)
    columns = []
    for arrow_field in arrow_schema:
        column = read_column(reader, arrow_field)
        columns.append(column)
    return pyarrow.RecordBatch.from_arrays(columns, schema=arrow_schema)
