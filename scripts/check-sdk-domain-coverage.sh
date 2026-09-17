#!/usr/bin/env bash
# Require full source coverage for every nessa-sdk domain context.
# Prerequisites: rustup component add llvm-tools-preview
#               cargo install cargo-llvm-cov --version 0.6.16 --locked
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
coverage_target=$(mktemp -d "${TMPDIR:-/tmp}/nessa-sdk-domain-coverage.XXXXXX")
trap 'rm -rf -- "$coverage_target"' EXIT

# Coverage instrumentation slows ACP cleanup handshakes enough that parallel
# contract tests can consume each other's short scheduling margin. Run the test
# binaries serially so this domain gate measures code paths deterministically.
: "${RUST_TEST_THREADS:=1}"
export RUST_TEST_THREADS

# Never clean or instrument the normal/shared workspace target directory.
# The workspace storage dependency is infrastructure, outside the SDK domain gate;
# keep every SDK domain file included at the same 100% thresholds.
CARGO_TARGET_DIR="$coverage_target" cargo llvm-cov -p nessa-sdk --locked \
  --ignore-filename-regex '/(application|infrastructure|tests|examples)/|/nessa-local-storage/' \
  --fail-under-lines 100 \
  --fail-under-functions 100 \
  --fail-under-regions 100
