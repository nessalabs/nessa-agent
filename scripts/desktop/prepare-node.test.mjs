import assert from "node:assert/strict"
import { createHash } from "node:crypto"
import {
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  symlinkSync,
  truncateSync,
  writeFileSync,
} from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import test from "node:test"
import { gzipSync } from "node:zlib"

import {
  NODE_VERSION,
  MAX_NODE_ARCHIVE_BYTES,
  acquireVerifiedNodeArchive,
  downloadNodeArchive,
  nodeArchive,
  readNodeArchive,
} from "./prepare-node.mjs"

function field(block, offset, length, value) {
  block.write(value, offset, Math.min(length, Buffer.byteLength(value)), "utf8")
}

function tar(entries, { endingBlocks = 2, trailing = Buffer.alloc(0) } = {}) {
  const blocks = []
  for (const entry of entries) {
    const contents = Buffer.from(entry.contents ?? "")
    const header = Buffer.alloc(512)
    field(header, 0, 100, entry.name)
    field(header, 100, 8, "0000755\0")
    field(header, 108, 8, "0000000\0")
    field(header, 116, 8, "0000000\0")
    field(header, 124, 12, `${contents.length.toString(8).padStart(11, "0")}\0`)
    field(header, 136, 12, "00000000000\0")
    header.fill(32, 148, 156)
    header[156] = (entry.type ?? "0").charCodeAt(0)
    field(header, 157, 100, entry.link ?? "")
    field(header, 257, 6, "ustar\0")
    field(header, 263, 2, "00")
    const checksum = [...header].reduce((sum, byte) => sum + byte, 0)
    field(header, 148, 8, `${checksum.toString(8).padStart(6, "0")}\0 `)
    blocks.push(header, contents, Buffer.alloc((512 - (contents.length % 512)) % 512))
  }
  if (endingBlocks > 0) blocks.push(Buffer.alloc(endingBlocks * 512))
  blocks.push(trailing)
  return gzipSync(Buffer.concat(blocks))
}

const distribution = `node-v${NODE_VERSION}-linux-x64`
const selected = [
  { name: `${distribution}/bin/node`, contents: "node" },
  { name: `${distribution}/LICENSE`, contents: "license" },
]

test("the reviewed Node release table is exact and closed", () => {
  assert.deepEqual(nodeArchive("darwin", "arm64"), {
    archive: "node-v26.8.1-darwin-arm64.tar.gz",
    distribution: "node-v26.8.1-darwin-arm64",
    sha256: "6e577fd0d9db776db82306629e441a9dace416702622aebdd171c9dfaa41f4d2",
    url: "https://nodejs.org/dist/v26.8.1/node-v26.8.1-darwin-arm64.tar.gz",
  })
  assert.equal(
    nodeArchive("darwin", "x64").sha256,
    "fe9c6dbf9c8e1b4443803d75e2a20366e420dae650c747dbb116b22975751baf",
  )
  assert.equal(
    nodeArchive("linux", "x64").sha256,
    "b2b76660fa4ded4e0b2a41ee3c0c651cd52ea8170ead91ebac1e147ac3d55643",
  )
  for (const pair of [
    ["linux", "arm64"],
    ["darwin", "ia32"],
    ["win32", "x64"],
    ["constructor", "prototype"],
  ])
    assert.throws(() => nodeArchive(...pair), /does not support/)
})

test("archive validation returns only the exact Node and license files", () => {
  const files = readNodeArchive(
    tar([
      { name: `${distribution}/`, type: "5" },
      ...selected,
      { name: `${distribution}/bin/corepack`, type: "2", link: "../lib/corepack.js" },
      {
        name: `${distribution}/LICENSE-copy`,
        type: "1",
        link: `${distribution}/LICENSE`,
      },
      {
        name: "././@LongLink",
        type: "L",
        contents: `${distribution}/${"a".repeat(260)}\0`,
      },
      { name: `${distribution}/placeholder`, contents: "ignored" },
    ]),
    distribution,
  )
  assert.equal(files.node.toString(), "node")
  assert.equal(files.license.toString(), "license")
})

test("archive validation accepts two zero end blocks and zero padding", () => {
  const files = readNodeArchive(tar(selected, { endingBlocks: 3 }), distribution)
  assert.equal(files.node.toString(), "node")
  assert.equal(files.license.toString(), "license")
})

test("archive validation rejects unsafe or contradictory entries", async (t) => {
  const cases = [
    ["missing descriptor", [selected[0]], /missing .*LICENSE/],
    ["duplicate selection", [...selected, selected[0]], /repeats a selected entry/],
    [
      "wrong selected type",
      [{ ...selected[0], type: "2", link: "node-real" }, selected[1]],
      /selected entry is not a file/,
    ],
    ["absolute path", [...selected, { name: "/outside" }], /unsafe path/],
    ["drive-absolute path", [...selected, { name: "C:/outside" }], /unsafe path/],
    [
      "traversal path",
      [...selected, { name: `${distribution}/../outside` }],
      /unsafe path/,
    ],
    [
      "escaping symlink",
      [
        ...selected,
        { name: `${distribution}/bin/link`, type: "2", link: "../../../outside" },
      ],
      /link escapes/,
    ],
    [
      "symlink escaping its distribution but not the archive root",
      [
        ...selected,
        { name: `${distribution}/bin/link`, type: "2", link: "../../outside" },
      ],
      /link escapes/,
    ],
    [
      "escaping hardlink",
      [...selected, { name: `${distribution}/bin/link`, type: "1", link: "../outside" }],
      /link escapes/,
    ],
    [
      "hardlink escaping its distribution but not the archive root",
      [
        ...selected,
        {
          name: `${distribution}/bin/link`,
          type: "1",
          link: `${distribution}/../outside`,
        },
      ],
      /link escapes/,
    ],
    [
      "traversal through a GNU long name",
      [
        ...selected,
        { name: "././@LongLink", type: "L", contents: `${distribution}/../outside\0` },
        { name: `${distribution}/placeholder` },
      ],
      /unsafe path/,
    ],
    [
      "unsupported type",
      [...selected, { name: `${distribution}/device`, type: "3" }],
      /unsupported entry type/,
    ],
  ]
  for (const [name, entries, message] of cases)
    await t.test(name, () =>
      assert.throws(() => readNodeArchive(tar(entries), distribution), message),
    )
  assert.throws(
    () => readNodeArchive(Buffer.from("not gzip"), distribution),
    /valid gzip/,
  )
  assert.throws(
    () => readNodeArchive(tar(selected, { endingBlocks: 0 }), distribution),
    /malformed ending/,
  )
  assert.throws(
    () => readNodeArchive(tar(selected, { endingBlocks: 1 }), distribution),
    /two zero blocks/,
  )
  assert.throws(
    () =>
      readNodeArchive(tar(selected, { trailing: Buffer.alloc(512, 1) }), distribution),
    /malformed ending/,
  )
  assert.throws(
    () => readNodeArchive(tar(selected, { trailing: Buffer.from([1]) }), distribution),
    /malformed ending/,
  )
  assert.throws(
    () =>
      readNodeArchive(gzipSync(Buffer.alloc(1025)), distribution, {
        maxExpandedBytes: 1024,
      }),
    /expands beyond its allowed size/,
  )
})

test("a corrupt cache is replaced only by verified downloaded bytes", (t) => {
  const cache = mkdtempSync(join(tmpdir(), "nessa-node-cache-"))
  t.after(() => rmSync(cache, { recursive: true, force: true }))
  const good = tar(selected)
  const release = {
    archive: "node-fixture.tar.gz",
    sha256: createHash("sha256").update(good).digest("hex"),
    url: "https://example.invalid/node-fixture.tar.gz",
  }
  writeFileSync(join(cache, release.archive), "corrupt")
  let downloads = 0
  const bytes = acquireVerifiedNodeArchive({
    cache,
    release,
    download({ destination, url }) {
      downloads += 1
      assert.equal(url, release.url)
      writeFileSync(destination, good)
    },
  })
  assert.equal(downloads, 1)
  assert.deepEqual(bytes, good)
  assert.deepEqual(readFileSync(join(cache, release.archive)), good)
  assert.deepEqual(readdirSync(cache), [release.archive])
})

test("a corrupt download is neither cached nor left staged", (t) => {
  const cache = mkdtempSync(join(tmpdir(), "nessa-node-download-"))
  t.after(() => rmSync(cache, { recursive: true, force: true }))
  const release = {
    archive: "node-fixture.tar.gz",
    sha256: "0".repeat(64),
    url: "https://example.invalid/node-fixture.tar.gz",
  }
  assert.throws(
    () =>
      acquireVerifiedNodeArchive({
        cache,
        release,
        download({ destination }) {
          writeFileSync(destination, "corrupt")
        },
      }),
    /invalid, oversized, or corrupt/,
  )
  assert.deepEqual(readdirSync(cache), [])
})

test("an oversized cache entry is rejected before reading and replaced", (t) => {
  const cache = mkdtempSync(join(tmpdir(), "nessa-node-oversized-cache-"))
  t.after(() => rmSync(cache, { recursive: true, force: true }))
  const good = tar(selected)
  const release = {
    archive: "node-fixture.tar.gz",
    sha256: createHash("sha256").update(good).digest("hex"),
    url: "https://example.invalid/node-fixture.tar.gz",
  }
  const archive = join(cache, release.archive)
  writeFileSync(archive, "")
  truncateSync(archive, MAX_NODE_ARCHIVE_BYTES + 1)
  let downloads = 0
  const bytes = acquireVerifiedNodeArchive({
    cache,
    release,
    download({ destination, maxBytes }) {
      downloads += 1
      assert.equal(maxBytes, MAX_NODE_ARCHIVE_BYTES)
      writeFileSync(destination, good)
    },
  })
  assert.equal(downloads, 1)
  assert.deepEqual(bytes, good)
  assert.deepEqual(readFileSync(archive), good)
})

test("an oversized unknown-length download is rejected and removed", (t) => {
  const cache = mkdtempSync(join(tmpdir(), "nessa-node-oversized-download-"))
  t.after(() => rmSync(cache, { recursive: true, force: true }))
  assert.throws(
    () =>
      acquireVerifiedNodeArchive({
        cache,
        release: {
          archive: "node-fixture.tar.gz",
          sha256: "0".repeat(64),
          url: "https://example.invalid/node-fixture.tar.gz",
        },
        download({ destination, maxBytes }) {
          assert.equal(maxBytes, MAX_NODE_ARCHIVE_BYTES)
          writeFileSync(destination, "")
          truncateSync(destination, maxBytes + 1)
        },
      }),
    /invalid, oversized, or corrupt/,
  )
  assert.deepEqual(readdirSync(cache), [])
})

test("the downloader enforces transfer and process time ceilings", () => {
  let executed = false
  assert.throws(
    () =>
      downloadNodeArchive({
        destination: "/owned-stage/node.tar.gz",
        url: "https://example.invalid/node.tar.gz",
        execute(command, arguments_, options) {
          executed = true
          assert.equal(command, "curl")
          assert.deepEqual(arguments_.slice(0, 6), [
            "--fail",
            "--location",
            "--max-filesize",
            String(MAX_NODE_ARCHIVE_BYTES),
            "--max-time",
            "120",
          ])
          assert.equal(options.timeout, 125_000)
          throw new Error("download timed out")
        },
      }),
    /timed out/,
  )
  assert.equal(executed, true)
})

test("a timed-out download leaves no cache or staging entry", (t) => {
  const cache = mkdtempSync(join(tmpdir(), "nessa-node-timeout-"))
  t.after(() => rmSync(cache, { recursive: true, force: true }))
  assert.throws(
    () =>
      acquireVerifiedNodeArchive({
        cache,
        release: {
          archive: "node-fixture.tar.gz",
          sha256: "0".repeat(64),
          url: "https://example.invalid/node-fixture.tar.gz",
        },
        download({ timeoutSeconds }) {
          assert.equal(timeoutSeconds, 120)
          throw new Error("download timed out")
        },
      }),
    /timed out/,
  )
  assert.deepEqual(readdirSync(cache), [])
})

test("a competing valid write during corrupt-cache recovery stays safe", (t) => {
  const cache = mkdtempSync(join(tmpdir(), "nessa-node-corrupt-race-"))
  t.after(() => rmSync(cache, { recursive: true, force: true }))
  const good = tar(selected)
  const release = {
    archive: "node-fixture.tar.gz",
    sha256: createHash("sha256").update(good).digest("hex"),
    url: "https://example.invalid/node-fixture.tar.gz",
  }
  const archive = join(cache, release.archive)
  writeFileSync(archive, "corrupt")
  const bytes = acquireVerifiedNodeArchive({
    cache,
    release,
    download({ destination }) {
      writeFileSync(archive, good)
      writeFileSync(destination, good)
    },
  })
  assert.deepEqual(bytes, good)
  assert.deepEqual(readFileSync(archive), good)
  assert.deepEqual(readdirSync(cache), [release.archive])
})

test("concurrent absent writers publish only verified bytes", (t) => {
  const cache = mkdtempSync(join(tmpdir(), "nessa-node-absent-race-"))
  t.after(() => rmSync(cache, { recursive: true, force: true }))
  const good = tar(selected)
  const release = {
    archive: "node-fixture.tar.gz",
    sha256: createHash("sha256").update(good).digest("hex"),
    url: "https://example.invalid/node-fixture.tar.gz",
  }
  let innerBytes
  const outerBytes = acquireVerifiedNodeArchive({
    cache,
    release,
    download({ destination }) {
      innerBytes = acquireVerifiedNodeArchive({
        cache,
        release,
        download({ destination: innerDestination }) {
          writeFileSync(innerDestination, good)
        },
      })
      writeFileSync(destination, good)
    },
  })
  assert.deepEqual(innerBytes, good)
  assert.deepEqual(outerBytes, good)
  assert.deepEqual(readFileSync(join(cache, release.archive)), good)
  assert.deepEqual(readdirSync(cache), [release.archive])
})

test("a redirected cache is refused before download or external writes", (t) => {
  const root = mkdtempSync(join(tmpdir(), "nessa-node-cache-root-"))
  t.after(() => rmSync(root, { recursive: true, force: true }))
  const external = join(root, "external")
  const cache = join(root, "cache")
  mkdirSync(external)
  writeFileSync(join(external, "marker"), "unchanged")
  symlinkSync(external, cache)
  let downloaded = false
  assert.throws(
    () =>
      acquireVerifiedNodeArchive({
        cache,
        release: {
          archive: "node-fixture.tar.gz",
          sha256: "0".repeat(64),
          url: "https://example.invalid/node-fixture.tar.gz",
        },
        download() {
          downloaded = true
        },
      }),
    /owned directory/,
  )
  assert.equal(downloaded, false)
  assert.equal(readFileSync(join(external, "marker"), "utf8"), "unchanged")
})
