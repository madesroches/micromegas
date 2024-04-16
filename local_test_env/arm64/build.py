#!/usr/bin/python3
import subprocess

def build():
    subprocess.run("docker build . -t mmarm64:latest", shell=True, check=True)

if __name__ == '__main__':
    build()
