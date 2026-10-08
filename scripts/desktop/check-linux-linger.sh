#!/usr/bin/env bash
# Prove SetUserLinger for a throwaway user. Never the account running this script.
set -euo pipefail

if [ "$(id -un)" = "lt" ]; then
  echo "refusing to enable linger for the account running this script" >&2
  exit 1
fi
if id lt >/dev/null 2>&1; then
  echo "user lt already exists; refusing to reuse an account" >&2
  exit 1
fi

cleanup() {
  status=$?
  sudo loginctl disable-linger lt >/dev/null 2>&1 || true
  sudo loginctl terminate-user lt >/dev/null 2>&1 || true
  sudo userdel -r lt >/dev/null 2>&1 || true
  exit "$status"
}
trap cleanup EXIT

sudo useradd -m lt
sudo loginctl disable-linger lt || true

executable="$(
  cargo test -p nessa-app --no-default-features --no-run --message-format=json -- linger_live \
    | node --input-type=module -e '
      let last = ""
      const chunks = []
      for await (const chunk of process.stdin) chunks.push(chunk)
      for (const line of Buffer.concat(chunks).toString("utf8").split("\n")) {
        if (!line) continue
        let message
        try {
          message = JSON.parse(line)
        } catch {
          continue
        }
        if (
          message?.reason === "compiler-artifact" &&
          message?.target?.name === "nessa-app" &&
          message?.profile?.test === true &&
          typeof message.executable === "string"
        ) {
          last = message.executable
        }
      }
      process.stdout.write(last)
    '
)"
if [ -z "${executable}" ] || [ ! -x "${executable}" ]; then
  echo "linger live test binary was not built" >&2
  exit 1
fi
install -m 0755 "${executable}" /tmp/nessa-linger-live

echo "linger_live starting for lt"
sudo systemd-run --wait --pipe --collect \
  -p User=lt \
  -p PAMName=login \
  -E NESSA_LINGER_ACCEPTANCE=1 \
  /tmp/nessa-linger-live --ignored linger_live
if [ ! -f /var/lib/systemd/linger/lt ]; then
  echo "linger file missing after linger_live" >&2
  exit 1
fi
echo "linger_live: SetUserLinger succeeded and /var/lib/systemd/linger/lt exists"

sudo install -d -o lt -g lt -m 0755 /home/lt/.config/systemd/user
sudo tee /home/lt/.config/systemd/user/nessa-gateway-dev.service >/dev/null <<'EOF'
[Unit]
Description=Nessa gateway stand-in for the linger check
[Service]
ExecStart=/bin/sleep infinity
[Install]
WantedBy=default.target
EOF
sudo chown lt:lt /home/lt/.config/systemd/user/nessa-gateway-dev.service
sudo systemd-run --wait --pipe --collect \
  -p User=lt \
  -p PAMName=login \
  /bin/bash -lc 'systemctl --user daemon-reload && systemctl --user enable --now nessa-gateway-dev.service'
for _ in $(seq 1 20); do
  if sudo systemctl --user -M lt@ is-active nessa-gateway-dev.service; then
    echo "linger kept nessa-gateway-dev.service active after the login session ended"
    exit 0
  fi
  sleep 0.5
done
echo "nessa-gateway-dev.service was not active after the login session ended" >&2
sudo systemctl --user -M lt@ status nessa-gateway-dev.service >&2 || true
exit 1
