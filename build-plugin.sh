#!/usr/bin/bash
set -e
yarn install --pure-lockfile
yarn build
mage -v
mage build:generateManifestFile
cp -r dist micromegas-datasource
zip -r micromegas-datasource.zip micromegas-datasource
