#!/bin/sh
set -eu

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)

cd "$project_dir/frontend"
npm install
npm run build

cd "$project_dir"
cargo run --release -- serve

