#!/bin/sh
set -eu
cd "$(dirname "$0")"
if [ ! -x bin/herdr-git-graph ]; then
    sh scripts/build.sh
fi
exec ./bin/herdr-git-graph --demo
