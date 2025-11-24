#!/usr/bin/bash
set -e
yarn install --pure-lockfile
yarn build
mage -v
mage build:generateManifestFile
rm -rf micromegas-micromegas-datasource micromegas-micromegas-datasource.zip
cp -r dist micromegas-micromegas-datasource
zip -r micromegas-micromegas-datasource.zip micromegas-micromegas-datasource
