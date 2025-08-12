#!/bin/python3
from rust_command import run_command
import subprocess
import sys


def run_coverage():
    """Run Rust code coverage using cargo-tarpaulin"""
    try:
        # Check if tarpaulin is installed, install if not
        print("Checking if cargo-tarpaulin is installed...")
        run_command("cargo tarpaulin --version")
        print("cargo-tarpaulin is already installed.")
    except subprocess.CalledProcessError:
        print("cargo-tarpaulin not found. Installing...")
        print("This may take several minutes as it needs to compile from source...")
        try:
            run_command("cargo install cargo-tarpaulin")
            print("cargo-tarpaulin installed successfully.")
        except subprocess.CalledProcessError as e:
            print(f"Failed to install cargo-tarpaulin: {e}")
            print("You can install it manually with: cargo install cargo-tarpaulin")
            sys.exit(1)

    # Run coverage with HTML and XML output
    print("Running code coverage...")
    try:
        run_command("cargo tarpaulin --out Html --out Xml --timeout 120")
        print("Coverage report generated:")
        print("- HTML report: rust/tarpaulin-report.html")
        print("- XML report: rust/cobertura.xml")
    except subprocess.CalledProcessError as e:
        print(f"Coverage generation failed: {e}")
        sys.exit(1)


if __name__ == "__main__":
    run_coverage()
