import assert from "node:assert/strict"
import { existsSync, mkdtempSync, readFileSync, readdirSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import test from "node:test"
import { deflateSync } from "node:zlib"

import {
  decodeWebdriverScreenshot,
  nativeSmokeEvidenceLimits,
  renderNativeSmokeError,
  retainNativeSmokeFailure,
} from "./native-smoke-evidence.mjs"

const onePixelPng = Buffer.from(
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=",
  "base64",
)

const crcTable = Array.from({ length: 256 }, (_, value) => {
  let crc = value
  for (let bit = 0; bit < 8; bit += 1)
    crc = crc & 1 ? 0xedb88320 ^ (crc >>> 1) : crc >>> 1
  return crc >>> 0
})

function crc32(value) {
  let crc = 0xffffffff
  for (const byte of value) crc = crcTable[(crc ^ byte) & 0xff] ^ (crc >>> 8)
  return (crc ^ 0xffffffff) >>> 0
}

function pngChunk(name, payload) {
  const type = Buffer.from(name)
  const chunk = Buffer.alloc(12 + payload.length)
  chunk.writeUInt32BE(payload.length, 0)
  type.copy(chunk, 4)
  payload.copy(chunk, 8)
  chunk.writeUInt32BE(crc32(Buffer.concat([type, payload])), 8 + payload.length)
  return chunk
}

function nearLimitPng() {
  const width = 1300
  const height = 1300
  const scanlines = Buffer.alloc(height * (1 + width * 3))
  let state = 0x5eed1234
  let offset = 0
  for (let row = 0; row < height; row += 1) {
    scanlines[offset] = 0
    offset += 1
    for (let column = 0; column < width * 3; column += 1) {
      state ^= state << 13
      state ^= state >>> 17
      state ^= state << 5
      scanlines[offset] = state & 0xff
      offset += 1
    }
  }
  const header = Buffer.alloc(13)
  header.writeUInt32BE(width, 0)
  header.writeUInt32BE(height, 4)
  header.set([8, 2, 0, 0, 0], 8)
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    pngChunk("IHDR", header),
    pngChunk("IDAT", deflateSync(scanlines)),
    pngChunk("IEND", Buffer.alloc(0)),
  ])
}

test("failure retention writes only the selected bounded evidence", () => {
  const root = mkdtempSync(join(tmpdir(), "nessa-native-evidence-"))
  try {
    const metadata = {
      lifecyclePhase: "WebDriver session requested",
      sessionCreated: false,
      pids: { application: 1234 },
    }
    const retained = retainNativeSmokeFailure(root, "isolated-instance", {
      logs: "[driver] startup cause\n",
      metadata,
    })

    assert.deepEqual(readdirSync(retained).sort(), ["harness.log", "metadata.json"])
    assert.equal(
      readFileSync(join(retained, "harness.log"), "utf8"),
      "[driver] startup cause\n",
    )
    assert.deepEqual(
      JSON.parse(readFileSync(join(retained, "metadata.json"), "utf8")),
      metadata,
    )
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})

test("nested primary, cleanup, and artifact failures retain their causes", () => {
  const timeout = new DOMException("automation handshake timed out", "TimeoutError")
  const primary = new Error("create WebDriver session failed", { cause: timeout })
  const cleanup = new Error("driver process group cleanup could not be verified")
  const artifact = new Error("failure artifact write was refused")
  const combined = new AggregateError([
    new AggregateError([primary, cleanup], "request and cleanup failed"),
    artifact,
  ])
  cleanup.cause = combined

  const rendered = renderNativeSmokeError(combined)

  assert.match(rendered, /create WebDriver session failed/)
  assert.match(rendered, /automation handshake timed out/)
  assert.match(rendered, /driver process group cleanup could not be verified/)
  assert.match(rendered, /failure artifact write was refused/)
  assert.match(rendered, /\[cycle\]/)
  assert.ok(Buffer.byteLength(rendered) <= nativeSmokeEvidenceLimits.errorBytes)
})

test("recursive rendering stops traversing after its item limit", () => {
  const failures = Array.from(
    { length: nativeSmokeEvidenceLimits.errorItems * 10 },
    (_, index) => new Error(`cleanup ${index}`),
  )
  const rendered = renderNativeSmokeError(new AggregateError(failures))

  assert.match(rendered, /cleanup 0/)
  assert.doesNotMatch(rendered, /cleanup 100/)
  assert.equal(rendered.match(/additional errors omitted/g)?.length, 1)
})

test("oversized logs and error metadata stay within UTF-8 byte budgets", () => {
  const root = mkdtempSync(join(tmpdir(), "nessa-native-evidence-large-"))
  try {
    const retained = retainNativeSmokeFailure(root, "bounded-instance", {
      logs: `discarded-prefix-${"🐇".repeat(40_000)}-retained-tail`,
      metadata: {
        lifecyclePhase: "session".repeat(10_000),
        error: `failure-${'\\"🐇'.repeat(30_000)}`,
        instance: "native-smoke",
        executableArtifacts: {
          application: "/tmp/" + "app".repeat(20_000),
          gateway: "/tmp/gateway",
        },
      },
    })
    const logs = readFileSync(join(retained, "harness.log"))
    const metadata = readFileSync(join(retained, "metadata.json"))

    assert.ok(logs.byteLength <= nativeSmokeEvidenceLimits.logsBytes)
    assert.ok(metadata.byteLength <= nativeSmokeEvidenceLimits.metadataBytes)
    assert.match(logs.toString("utf8"), /retained-tail$/)
    assert.doesNotThrow(() => JSON.parse(metadata.toString("utf8")))
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})

test("page, window, and PNG evidence survive within their explicit bounds", () => {
  const root = mkdtempSync(join(tmpdir(), "nessa-native-evidence-page-"))
  try {
    const screenshot = decodeWebdriverScreenshot(onePixelPng.toString("base64"))
    const retained = retainNativeSmokeFailure(root, "page-instance", {
      logs: "timeout\n",
      screenshot,
      metadata: {
        lastPanelObservation: {
          url: `tauri://localhost/${"🐇".repeat(2_000)}`,
          title: "Nessa",
          readyState: "complete",
          surface: null,
          root: { present: true, width: 400, height: 320, display: "block" },
          fallback: { present: false },
          connectionText: "Connecting to the local server…",
          bodyText: "wrong page evidence ".repeat(500),
          viewport: { width: 1440, height: 900 },
        },
        windows: {
          current: "main",
          handles: Array.from({ length: 30 }, (_, index) => `window-${index}`),
        },
      },
    })
    const metadata = readFileSync(join(retained, "metadata.json"))
    const selected = JSON.parse(metadata.toString("utf8"))

    assert.ok(metadata.byteLength <= nativeSmokeEvidenceLimits.metadataBytes)
    assert.equal(
      selected.lastPanelObservation.connectionText,
      "Connecting to the local server…",
    )
    assert.match(selected.lastPanelObservation.bodyText, /^wrong page evidence/)
    assert.deepEqual(selected.windows.handles.slice(0, 2), ["window-0", "window-1"])
    assert.equal(selected.windows.handles.length, 16)
    assert.deepEqual(readFileSync(join(retained, "webview.png")), onePixelPng)
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})

test("invalid and oversized screenshots leave no partial evidence directory", () => {
  const root = mkdtempSync(join(tmpdir(), "nessa-native-evidence-png-"))
  try {
    assert.throws(() => decodeWebdriverScreenshot("not-base64"), /base64/)
    assert.throws(() => decodeWebdriverScreenshot("AAAA!!!!"), /canonical base64/)
    assert.throws(
      () =>
        retainNativeSmokeFailure(root, "invalid", {
          logs: "failure",
          metadata: {},
          screenshot: Buffer.from("not a PNG"),
        }),
      /bounded PNG/,
    )
    const oversized = Buffer.alloc(nativeSmokeEvidenceLimits.screenshotBytes + 1)
    onePixelPng.copy(oversized, 0, 0, 8)
    assert.throws(
      () =>
        retainNativeSmokeFailure(root, "oversized", {
          logs: "failure",
          metadata: {},
          screenshot: oversized,
        }),
      /bounded PNG/,
    )
    assert.equal(existsSync(join(root, "invalid")), false)
    assert.equal(existsSync(join(root, "oversized")), false)
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})

test("a valid near-limit PNG is decoded with bounded linear validation", () => {
  const screenshot = nearLimitPng()

  assert.ok(screenshot.length > 5_000_000)
  assert.ok(screenshot.length < nativeSmokeEvidenceLimits.screenshotBytes)
  assert.deepEqual(decodeWebdriverScreenshot(screenshot.toString("base64")), screenshot)
})
