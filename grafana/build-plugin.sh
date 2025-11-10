#!/usr/bin/bash
set -e
yarn install --pure-lockfile
yarn build
mage -v
mage build:generateManifestFile
cp -r dist micromegas-micromegas-datasource
zip -r micromegas-micromegas-datasource.zip micromegas-micromegas-datasource
