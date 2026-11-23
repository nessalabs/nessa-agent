# Headless Linux `nessa-server` Gateway

This guide covers running a full `nessa-server` gateway headlessly on a Linux
VM, devbox, or container with native Ghostty terminals (`--features nessa-server/terminals`),
non-interactive vault startup, and `wss://` reachability (ADR 1417, [#1418](https://github.com/nessalabs/nessa-agent/issues/1418)).

## 1. Build with terminals

Build the pinned `libghostty-vt` library and compile the `nessa` binary:

```bash
eval "$(scripts/terminal/ghostty.sh)"
cargo build --release --locked -p nessa-server --bin nessa --features nessa-server/terminals
```

Or build the container image from the repository root:

```bash
docker build -f docs/guides/headless-gateway/Dockerfile -t nessa-gateway-headless .
```

## 2. Configure `wss://` reachability (`config.json`)

Under `$NESSA_DATA_DIR/config.json` (`0600`, regular non-symlink file) when
`NESSA_STAGE=prod` (or `$NESSA_DATA_DIR/$NESSA_STAGE/config.json` for
non-`prod` stages), enable native pairing and a web tunnel (`cloudflared` quick
tunnel or a named Cloudflare tunnel):

```json
{
  "native": {
    "listenAddress": "127.0.0.1:0",
    "tunnel": {
      "provider": "cloudflare",
      "mode": "quick"
    }
  }
}
```

For a named Cloudflare tunnel with a stable hostname, set `"mode": "named"`,
`"hostname": "https://gateway.example.com"`, and place the tunnel token in
`$NESSA_DATA_DIR/tunnel/token` (`0600`; or `$NESSA_DATA_DIR/$NESSA_STAGE/tunnel/token`
for non-`prod` stages).

## 3. Optional headless vault unlock

Create a private passphrase file (`0600`, regular non-symlink file, meeting
Nessa's vault passphrase policy):

```bash
install -m 0600 /dev/null /etc/nessa/vault-passphrase
printf '%s\n' '<your-vault-passphrase>' > /etc/nessa/vault-passphrase
```

Start `nessa server` with `--vault-passphrase-file`:

```bash
NESSA_STAGE=prod NESSA_DATA_DIR=/var/lib/nessa \
  nessa server --provision-local --vault-passphrase-file /etc/nessa/vault-passphrase
```

- On first boot (`Unconfigured`), `nessa` initializes and unlocks the encrypted
  vault before binding listeners.
- On subsequent boots (`Locked`), `nessa` unlocks the existing vault.
- Any file, permission, symlink, policy, or wrong-passphrase error fails closed
  before listening.
- On shutdown, the headless vault session closes and wipes key material.

## 4. Verify health and pair a client

Check readiness:

```bash
curl -fsS http://127.0.0.1:18789/health
```

Read the tunnel reachability address and create a device pairing invitation:

```bash
NESSA_STAGE=prod NESSA_DATA_DIR=/var/lib/nessa nessa call server.reach --params '{}'
NESSA_STAGE=prod NESSA_DATA_DIR=/var/lib/nessa nessa call pairing.create --params '{}'
```

Use the reported `wss://` tunnel address and pairing code to pair Phone or
Desktop. Native ⌘J shells and Run blocks run directly inside this VM via
in-process `pty-runtime` + Ghostty and keep running when clients disconnect.
