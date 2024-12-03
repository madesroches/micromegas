#!/usr/bin/bash
set -e
yarn install --pure-lockfile
yarn build
mage -v
zip -r micromegas-datasource.zip micromegas-datasource
