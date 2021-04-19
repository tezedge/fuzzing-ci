#!/usr/bin/env bash
set -eu

dir=$1
url=$2
branch=$3

git clone git@github.com:simplestaking/tezedge-fuzzing.git -b fuzz-coverage $dir
cd $dir
git config -f .gitmodules submodule.code/tezedge.url $url
git config -f .gitmodules submodule.code/tezedge.branch $branch
git submodule update --init --recursive --remote
cd code/tezedge
git status
