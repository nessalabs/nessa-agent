import { test } from "node:test"
import assert from "node:assert/strict"
import { spawn } from "node:child_process"
import { randomUUID } from "node:crypto"
import { once } from "node:events"
import { browse } from "./mdns-probe.mjs"

test(
  "macOS Bonjour browse correlates a live registration's port and gateway, then observes withdrawal",
  {
    skip: process.platform !== "darwin",
    timeout: 15_000,
  },
  async () => {
    const gateway = randomUUID()
    // Spaces exercise dns-sd's escaped instance names. An unrelated real
    // gateway may be present; matching only a service count would hide failure.
    const registration = spawn(
      "/usr/bin/dns-sd",
      [
        "-R",
        `phone probe ${gateway}`,
        "_nessa-link._tcp",
        "local.",
        "47999",
        `gw=${gateway}`,
      ],
      { stdio: "ignore" },
    )
    const closed = once(registration, "close")
    try {
      await once(registration, "spawn")
      const found = await browse("_nessa-link._tcp.local", 3_000)
      const own = found.find((each) => each.txt.gw === gateway)
      assert.equal(own?.port, 47999)
    } finally {
      registration.kill()
      await closed
    }
    const after = await browse("_nessa-link._tcp.local", 3_000)
    assert.equal(
      after.some((each) => each.txt.gw === gateway),
      false,
    )
  },
)
