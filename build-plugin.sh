#!/usr/bin/bash
set -e
yarn install --pure-lockfile
yarn build
mage -v
pushd dist
tar -czvf ../micromegas-datasource.tar.gz *
popd
