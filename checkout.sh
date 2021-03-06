#!/usr/bin/env bash
set -eu

dir=$1
url=$2
branch=$3

git clone https://github.com/tezedge/tezedge-fuzzing.git "$dir"
cd $dir
git config -f .gitmodules submodule.code/tezedge.url "$url"
git config -f .gitmodules submodule.code/tezedge.branch "$branch"
git submodule update --init --recursive --remote code/tezedge
cd code/tezedge
git status
