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

# Do not run what this does not measure. `link_attack_tests` sweeps every
# Unicode scalar through the ACP adapter's two encoders and reads each result
# back with a CommonMark parser — a few seconds normally, and about twenty
# minutes under instrumentation, for a file this gate excludes from its own
# report. It is the ordinary `cargo test` gate's to run, at full strength; here
# it is pure cost. The domain regions it touches are covered by the domain
# tests, which is asserted by this gate still reporting 100%.
#
# Skipped by module name rather than by listing each test, so a sweep added
# beside them is skipped too. A rename does not weaken anything: the gate goes
# back to being slow, and says so by taking twenty minutes.
skip=(--skip link_attack_tests)

# Never clean or instrument the normal/shared workspace target directory.
# The workspace storage dependency is infrastructure, outside the SDK domain gate;
# keep every SDK domain file included at the same 100% thresholds.
CARGO_TARGET_DIR="$coverage_target" cargo llvm-cov -p nessa-sdk --locked \
  --ignore-filename-regex '/(application|infrastructure|tests|examples)/|/nessa-local-storage/' \
  --fail-under-lines 100 \
  --fail-under-functions 100 \
  --fail-under-regions 100 \
  -- "${skip[@]}"
