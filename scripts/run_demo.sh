#!/bin/sh
set -eu

cargo test --workspace
cargo run --release -- accuracy --nodes 1000 --degree 10 --probability 0.05 --samples 100 --threads 4 --partitions 4 --seed 42
cargo run --release -- demo --nodes 100000 --degree 10 --probability 0.05 --samples 100 --threads 4 --partitions 4 --seed 42
