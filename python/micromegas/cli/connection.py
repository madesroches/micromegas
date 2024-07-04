import importlib
import os

def connect():
    micromegas_module_name = os.environ.get(
        "MICROMEGAS_PYTHON_MODULE_WRAPPER", "micromegas"
    )
    micromegas_module = importlib.import_module(micromegas_module_name)
    client = micromegas_module.connect()
    return client
    
