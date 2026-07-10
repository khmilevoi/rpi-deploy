#!/usr/bin/env bash
set -euo pipefail

rm -rf /srv/git/fixture.git /tmp/fixture-work
mkdir -p /srv/git
cp -a /opt/e2e/fixtures/app /tmp/fixture-work
cd /tmp/fixture-work
git init --initial-branch=main
git config user.name 'rpi e2e'
git config user.email 'rpi-e2e@example.invalid'
git add .
git commit -m 'fixture: initial app'
git clone --bare . /srv/git/fixture.git
exec git daemon --reuseaddr --verbose --export-all --base-path=/srv/git /srv/git
