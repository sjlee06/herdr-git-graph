#!/bin/sh
set -eu
cd "$(dirname "$0")"
if [ ! -x bin/herdr-git-graph ]; then
    sh scripts/install.sh
fi
exec ./bin/herdr-git-graph --demo
