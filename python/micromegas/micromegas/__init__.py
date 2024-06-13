import os
from . import request
from . import client

# hack to allow perfetto proto imports
# you can then import the protos like this: from protos.perfetto.trace import trace_pb2
def load_perfetto_protos():
    import sys
    import pathlib
    perfetto_folder =  pathlib.Path(__file__).parent.absolute() / "thirdparty/perfetto"
    sys.path.append(str(perfetto_folder))
