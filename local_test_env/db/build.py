#!/usr/bin/python3
import subprocess

def build():
    subprocess.run("docker build . -t teledb:latest", shell=True, check=True)

if __name__ == '__main__':
    build()
