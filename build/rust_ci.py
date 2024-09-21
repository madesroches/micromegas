#!/bin/python3
from rust_command import run_command

run_command("cargo fmt --check")
run_command("cargo test")
