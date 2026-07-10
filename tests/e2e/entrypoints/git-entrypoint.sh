#!/usr/bin/env bash
set -euo pipefail

SCENARIO=${RPI_E2E_SCENARIO:?RPI_E2E_SCENARIO must be set}
APP=/opt/e2e/scenarios/$SCENARIO/app

rm -rf /srv/git/fixture.git /tmp/fixture-work
mkdir -p /srv/git
cp -a "$APP" /tmp/fixture-work
cd /tmp/fixture-work
git init --initial-branch=main
git config user.name 'rpi e2e'
git config user.email 'rpi-e2e@example.invalid'
git add .
git commit -m "fixture: $SCENARIO app"
git clone --bare . /srv/git/fixture.git
exec git daemon --reuseaddr --verbose --export-all --base-path=/srv/git /srv/git
