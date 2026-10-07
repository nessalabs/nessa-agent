#!/bin/bash
set -u
export CARGO_HOME=/tmp/nessa-cargo RUSTUP_HOME=/tmp/nessa-rustup PATH=/tmp/nessa-cargo/bin:$PATH CARGO_TARGET_DIR=/workspace/nessa-agent/target CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=2
run_gate() {
  gate="$1"
  shift
  /tmp/nessa-browser-libs/usr/bin/tini -s -- "$@" > "/tmp/650-binding-race-evidence/$gate.log" 2>&1
  result=$?
  printf '%s %s\n' "$gate" "$result" >> /tmp/650-binding-race-evidence/gate-status.txt
  if [ "$result" -ne 0 ]; then tail -80 "/tmp/650-binding-race-evidence/$gate.log"; exit "$result"; fi
  printf 'PASS %s\n' "$gate"
}
run_gate fmt cargo fmt --all -- --check
run_gate sdk-docs node scripts/check-sdk-docs.mjs
run_gate architecture-tests node --test scripts/architecture/*.test.mjs
run_gate architecture node scripts/check-architecture.mjs
run_gate six-clippy cargo clippy -p nessa-local-storage -p nessa-auth -p nessa-server -p nessa-protocol -p nessa-client-core -p nessa-sdk --all-targets -- -D warnings
run_gate sdk-full cargo test -p nessa-sdk
run_gate six-tests node scripts/cargo-test-parallel.mjs --concurrency 2 -- -p nessa-local-storage -p nessa-auth -p nessa-server -p nessa-protocol -p nessa-client-core -p nessa-sdk
run_gate rustdoc env 'RUSTDOCFLAGS=-D warnings' cargo doc -p nessa-sdk --no-deps
