#!/usr/bin/env bash
# Run on a disposable agent37-base instance, from a copy of this checkout
# beside a copy of workspace-sdk at the revision Cargo.lock pins. Writes
# the image context (Dockerfile and opt/nessa) to $1. Holds no credentials:
# the context is only built programs and their licenses.
set -euo pipefail
out=${1:?Usage: build.sh <context directory>}
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../.." && pwd)
sdk=${WORKSPACE_SDK:-$repo/../workspace-sdk}
command -v cargo >/dev/null || {
  curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
  . "$HOME/.cargo/env"
}
cd "$repo"
cargo build --release --locked -p nessa-server --bin nessa
cargo build --release --locked -p nessa-mcp
(cd "$sdk" && cargo build --release --locked --no-default-features --features portability --bin workspace-supervisor)
rm -rf "$out" && mkdir -p "$out/opt/nessa/bin" "$out/opt/nessa/harnesses"
cp "$here/Dockerfile" "$out/"
cp target/release/nessa target/release/nessa-mcp "$sdk/target/release/workspace-supervisor" "$out/opt/nessa/bin/"
cp "$repo/docs/guides/headless-gateway/nessa-gateway-bootstrap" "$out/opt/nessa/bin/nessa-gateway-bootstrap"
chmod +x "$out/opt/nessa/bin/nessa-gateway-bootstrap"
cp "$repo/docs/guides/headless-gateway/nessa-server.service" "$out/opt/nessa/nessa-server.service"
cp "$(command -v node)" "$out/opt/nessa/bin/node"
for harness in codex-acp claude-acp; do
  cp crates/nessa-sdk/harnesses/$harness/package.json "$out/opt/nessa/harnesses/$harness.json"
done
# One node_modules holding both harnesses, at the versions their bindings pin.
(cd "$out/opt/nessa/harnesses" && npm init -y >/dev/null \
  && npm install --omit=dev --no-audit --no-fund \
    "@agentclientprotocol/codex-acp@$(node -p 'require("./codex-acp.json").dependencies["@agentclientprotocol/codex-acp"]')" \
    "@agentclientprotocol/claude-agent-acp@$(node -p 'require("./claude-acp.json").dependencies["@agentclientprotocol/claude-agent-acp"]')")
cp crates/nessa-sdk/data/models.json "$out/opt/nessa/models.json"
cat > "$out/opt/nessa/BUILD.json" <<JSON
{"source":"$(git rev-parse HEAD 2>/dev/null || echo unknown)","leaseProtocol":"$("$out/opt/nessa/bin/nessa" env protocol 2>/dev/null || echo unknown)","built":"$(date -u +%FT%TZ)"}
JSON
echo "context ready in $out"
