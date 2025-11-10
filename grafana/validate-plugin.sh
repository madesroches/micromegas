#!/usr/bin/bash
docker run --pull=always -v ./micromegas-micromegas-datasource.zip:/archive.zip grafana/plugin-validator-cli /archive.zip
