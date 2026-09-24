import assert from "node:assert/strict"
import { execFileSync } from "node:child_process"
import { createHash } from "node:crypto"
import {
  existsSync,
  linkSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  renameSync,
  rmSync,
  symlinkSync,
  truncateSync,
  unlinkSync,
  writeFileSync,
} from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import test from "node:test"
import { gzipSync } from "node:zlib"

import {
  NODE_VERSION,
  MAX_NODE_ARCHIVE_BYTES,
  acquireVerifiedNodeArchive,
  downloadNodeArchive,
  nodeArchive,
  nodeCacheObjectName,
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
    blocks.push(
      header,
      contents,
      Buffer.alloc((512 - (contents.length % 512)) % 512, entry.paddingByte ?? 0),
    )
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

function fixtureRelease(bytes) {
  return {
    archive: "node-fixture.tar.gz",
    sha256: createHash("sha256").update(bytes).digest("hex"),
    url: "https://example.invalid/node-fixture.tar.gz",
  }
}

function cacheObject(cache, release) {
  return join(cache, nodeCacheObjectName(release))
}

function captureError(run) {
  let captured
  assert.throws(run, (error) => {
    captured = error
    return true
  })
  return captured
}

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
  assert.equal(
    nodeCacheObjectName(nodeArchive("linux", "x64")),
    "b2b76660fa4ded4e0b2a41ee3c0c651cd52ea8170ead91ebac1e147ac3d55643.tar.gz",
  )
  assert.throws(() => nodeCacheObjectName({ sha256: "not-a-digest" }), /invalid SHA-256/)
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
    [
      "non-zero entry padding",
      [{ ...selected[0], paddingByte: 1 }, selected[1]],
      /non-zero padding/,
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

test("an absent digest object is exclusively published and reopened", (t) => {
  const cache = mkdtempSync(join(tmpdir(), "nessa-node-cache-"))
  t.after(() => rmSync(cache, { recursive: true, force: true }))
  const good = tar(selected)
  const release = fixtureRelease(good)
  let publishedIdentity
  const bytes = acquireVerifiedNodeArchive({
    cache,
    release,
    download({ destination, url }) {
      assert.equal(url, release.url)
      writeFileSync(destination, good)
    },
    publish(source, destination) {
      linkSync(source, destination)
      const sourceStat = lstatSync(source)
      const destinationStat = lstatSync(destination)
      publishedIdentity = [sourceStat.dev, sourceStat.ino]
      assert.deepEqual([destinationStat.dev, destinationStat.ino], publishedIdentity)
    },
  })
  assert.deepEqual(bytes, good)
  assert.deepEqual(readFileSync(cacheObject(cache, release)), good)
  assert.deepEqual(readdirSync(cache), [nodeCacheObjectName(release)])
  assert.equal(existsSync(join(cache, release.archive)), false)
  const published = lstatSync(cacheObject(cache, release))
  assert.deepEqual([published.dev, published.ino], publishedIdentity)
})

test("an existing valid digest object is reused without download", (t) => {
  const cache = mkdtempSync(join(tmpdir(), "nessa-node-existing-"))
  t.after(() => rmSync(cache, { recursive: true, force: true }))
  const good = tar(selected)
  const release = fixtureRelease(good)
  writeFileSync(cacheObject(cache, release), good)
  const bytes = acquireVerifiedNodeArchive({
    cache,
    release,
    download() {
      assert.fail("valid immutable object must not download")
    },
  })
  assert.deepEqual(bytes, good)
})

test("an archive-name entry is neither fallback nor migration source", (t) => {
  const cache = mkdtempSync(join(tmpdir(), "nessa-node-old-name-"))
  t.after(() => rmSync(cache, { recursive: true, force: true }))
  const good = tar(selected)
  const release = fixtureRelease(good)
  const oldPath = join(cache, release.archive)
  writeFileSync(oldPath, "preserve")
  const bytes = acquireVerifiedNodeArchive({
    cache,
    release,
    download({ destination }) {
      writeFileSync(destination, good)
    },
  })
  assert.deepEqual(bytes, good)
  assert.equal(readFileSync(oldPath, "utf8"), "preserve")
  assert.deepEqual(readFileSync(cacheObject(cache, release)), good)
})

test("invalid regular digest objects refuse unchanged without download", async (t) => {
  const good = tar(selected)
  const release = fixtureRelease(good)
  for (const kind of ["digest", "oversized"]) {
    await t.test(kind, () => {
      const cache = mkdtempSync(join(tmpdir(), "nessa-node-invalid-cache-"))
      t.after(() => rmSync(cache, { recursive: true, force: true }))
      const archive = cacheObject(cache, release)
      if (kind === "digest") writeFileSync(archive, "corrupt")
      else {
        writeFileSync(archive, "")
        truncateSync(archive, MAX_NODE_ARCHIVE_BYTES + 1)
      }
      const before = lstatSync(archive)
      let downloaded = false
      const error = captureError(() =>
        acquireVerifiedNodeArchive({
          cache,
          release,
          download() {
            downloaded = true
          },
        }),
      )
      assert.match(error.message, /validation failed; this invocation made no mutation/)
      assert.equal(error.message.includes(archive), true)
      assert.match(
        error.cause.message,
        kind === "digest" ? /does not match its pinned digest/ : /compressed-size limit/,
      )
      const after = lstatSync(archive)
      assert.equal(downloaded, false)
      assert.deepEqual(
        [after.dev, after.ino, after.size],
        [before.dev, before.ino, before.size],
      )
    })
  }
})

test("a corrupt staged download is removed without publishing", (t) => {
  const cache = mkdtempSync(join(tmpdir(), "nessa-node-download-"))
  t.after(() => rmSync(cache, { recursive: true, force: true }))
  const release = {
    archive: "node-fixture.tar.gz",
    sha256: "0".repeat(64),
    url: "https://example.invalid/node-fixture.tar.gz",
  }
  const error = captureError(() =>
    acquireVerifiedNodeArchive({
      cache,
      release,
      download({ destination }) {
        writeFileSync(destination, "corrupt")
      },
    }),
  )
  assert.match(error.message, /acquisition failed after its owned stage was removed/)
  assert.match(error.cause.message, /does not match its pinned digest/)
  assert.deepEqual(readdirSync(cache), [])
})

test("an oversized unknown-length download is rejected and removed", (t) => {
  const cache = mkdtempSync(join(tmpdir(), "nessa-node-oversized-download-"))
  t.after(() => rmSync(cache, { recursive: true, force: true }))
  const error = captureError(() =>
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
  )
  assert.match(error.message, /acquisition failed after its owned stage was removed/)
  assert.match(error.cause.message, /compressed-size limit/)
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
          assert.equal(arguments_.includes("--max-filesize"), true)
          assert.equal(arguments_.includes(String(MAX_NODE_ARCHIVE_BYTES)), true)
          assert.equal(arguments_.includes("--max-time"), true)
          assert.equal(arguments_.includes("120"), true)
          assert.equal(arguments_.includes("--output"), false)
          assert.equal(options.maxBuffer, MAX_NODE_ARCHIVE_BYTES)
          assert.deepEqual(options.stdio, ["ignore", "pipe", "inherit"])
          assert.equal(options.timeout, 125_000)
          throw new Error("download timed out")
        },
      }),
    /timed out/,
  )
  assert.equal(executed, true)
})

test("an unknown-length transfer overflow writes no destination", (t) => {
  const root = mkdtempSync(join(tmpdir(), "nessa-node-transfer-overflow-"))
  t.after(() => rmSync(root, { recursive: true, force: true }))
  const destination = join(root, "node-fixture.tar.gz")
  assert.throws(
    () =>
      downloadNodeArchive({
        destination,
        url: "https://example.invalid/chunked",
        execute(command, arguments_, options) {
          assert.equal(command, "curl")
          assert.equal(arguments_.includes("--output"), false)
          assert.equal(options.maxBuffer, MAX_NODE_ARCHIVE_BYTES)
          const error = new Error("stdout maxBuffer length exceeded")
          error.code = "ENOBUFS"
          throw error
        },
      }),
    (error) => error.code === "ENOBUFS",
  )
  assert.equal(existsSync(destination), false)
})

test("a bounded successful transfer writes the exact returned bytes", (t) => {
  const root = mkdtempSync(join(tmpdir(), "nessa-node-transfer-success-"))
  t.after(() => rmSync(root, { recursive: true, force: true }))
  const destination = join(root, "node-fixture.tar.gz")
  const expected = Buffer.from([0, 1, 2, 255])
  downloadNodeArchive({
    destination,
    url: "https://example.invalid/node.tar.gz",
    execute(_command, _arguments, options) {
      assert.equal(options.maxBuffer, MAX_NODE_ARCHIVE_BYTES)
      return expected
    },
  })
  assert.deepEqual(readFileSync(destination), expected)
})

test("a timed-out download leaves no cache or staging entry", (t) => {
  const cache = mkdtempSync(join(tmpdir(), "nessa-node-timeout-"))
  t.after(() => rmSync(cache, { recursive: true, force: true }))
  const error = captureError(() =>
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
  )
  assert.match(error.message, /acquisition failed after its owned stage was removed/)
  assert.match(error.cause.message, /download timed out/)
  assert.deepEqual(readdirSync(cache), [])
})

test("concurrent absent writers publish only verified bytes", (t) => {
  const cache = mkdtempSync(join(tmpdir(), "nessa-node-absent-race-"))
  t.after(() => rmSync(cache, { recursive: true, force: true }))
  const good = tar(selected)
  const release = fixtureRelease(good)
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
  assert.deepEqual(readFileSync(cacheObject(cache, release)), good)
  assert.deepEqual(readdirSync(cache), [nodeCacheObjectName(release)])
})

test("nonregular cache entries are refused without mutation", async (t) => {
  const good = tar(selected)
  const release = {
    archive: "node-fixture.tar.gz",
    sha256: createHash("sha256").update(good).digest("hex"),
    url: "https://example.invalid/node-fixture.tar.gz",
  }
  for (const kind of ["symlink", "directory", "FIFO"]) {
    await t.test(
      kind,
      {
        skip:
          kind === "FIFO" && process.platform === "win32"
            ? "Windows does not provide the POSIX mkfifo utility"
            : false,
      },
      () => {
        const root = mkdtempSync(join(tmpdir(), "nessa-node-nonregular-"))
        t.after(() => rmSync(root, { recursive: true, force: true }))
        const cache = join(root, "cache")
        const archive = cacheObject(cache, release)
        mkdirSync(cache)
        if (kind === "symlink") {
          const external = join(root, "external")
          writeFileSync(external, "preserve")
          symlinkSync(external, archive)
        } else if (kind === "directory") {
          mkdirSync(archive)
          writeFileSync(join(archive, "marker"), "preserve")
        } else {
          execFileSync("mkfifo", [archive])
        }
        let downloaded = false
        const error = captureError(() =>
          acquireVerifiedNodeArchive({
            cache,
            release,
            download() {
              downloaded = true
            },
          }),
        )
        assert.match(error.message, /validation failed; this invocation made no mutation/)
        assert.equal(error.message.includes(archive), true)
        assert.match(
          error.cause.message,
          new RegExp(kind === "symlink" ? "symbolic link" : kind),
        )
        assert.equal(downloaded, false)
        if (kind === "symlink") assert.equal(readFileSync(archive, "utf8"), "preserve")
        if (kind === "directory")
          assert.equal(readFileSync(join(archive, "marker"), "utf8"), "preserve")
      },
    )
  }
})

test("a nonregular replacement during download is preserved and refused", (t) => {
  const cache = mkdtempSync(join(tmpdir(), "nessa-node-download-race-"))
  t.after(() => rmSync(cache, { recursive: true, force: true }))
  const good = tar(selected)
  const release = {
    archive: "node-fixture.tar.gz",
    sha256: createHash("sha256").update(good).digest("hex"),
    url: "https://example.invalid/node-fixture.tar.gz",
  }
  const archive = cacheObject(cache, release)
  const error = captureError(() =>
    acquireVerifiedNodeArchive({
      cache,
      release,
      download({ destination }) {
        mkdirSync(archive)
        writeFileSync(join(archive, "marker"), "preserve")
        writeFileSync(destination, good)
      },
    }),
  )
  assert.match(error.message, /acquisition failed after its owned stage was removed/)
  assert.match(error.cause.message, /directory/)
  assert.equal(readFileSync(join(archive, "marker"), "utf8"), "preserve")
})

test("a nonregular replacement at exclusive publication is preserved", (t) => {
  const cache = mkdtempSync(join(tmpdir(), "nessa-node-publication-race-"))
  t.after(() => rmSync(cache, { recursive: true, force: true }))
  const good = tar(selected)
  const release = {
    archive: "node-fixture.tar.gz",
    sha256: createHash("sha256").update(good).digest("hex"),
    url: "https://example.invalid/node-fixture.tar.gz",
  }
  const archive = cacheObject(cache, release)
  const error = captureError(() =>
    acquireVerifiedNodeArchive({
      cache,
      release,
      download({ destination }) {
        writeFileSync(destination, good)
      },
      publish(source, destination) {
        assert.equal(existsSync(source), true)
        assert.equal(destination, archive)
        mkdirSync(destination)
        writeFileSync(join(destination, "marker"), "preserve")
        const publicationError = new Error("destination appeared")
        publicationError.code = "EEXIST"
        throw publicationError
      },
    }),
  )
  assert.match(error.message, /acquisition failed after its owned stage was removed/)
  assert.match(error.cause.message, /directory/)
  assert.equal(readFileSync(join(archive, "marker"), "utf8"), "preserve")
})

test("a verified publication winner is accepted without overwrite", (t) => {
  const cache = mkdtempSync(join(tmpdir(), "nessa-node-publication-winner-"))
  t.after(() => rmSync(cache, { recursive: true, force: true }))
  const good = tar(selected)
  const release = {
    archive: "node-fixture.tar.gz",
    sha256: createHash("sha256").update(good).digest("hex"),
    url: "https://example.invalid/node-fixture.tar.gz",
  }
  const archive = cacheObject(cache, release)
  const bytes = acquireVerifiedNodeArchive({
    cache,
    release,
    download({ destination }) {
      writeFileSync(destination, good)
    },
    publish(source, destination) {
      writeFileSync(destination, good)
      assert.notDeepEqual(
        [lstatSync(source).dev, lstatSync(source).ino],
        [lstatSync(destination).dev, lstatSync(destination).ino],
      )
      const error = new Error("destination appeared")
      error.code = "EEXIST"
      throw error
    },
  })
  assert.deepEqual(bytes, good)
  assert.deepEqual(readFileSync(archive), good)
})

test("a publisher reporting success with a corrupt destination is refused", (t) => {
  const cache = mkdtempSync(join(tmpdir(), "nessa-node-publication-invalid-"))
  t.after(() => rmSync(cache, { recursive: true, force: true }))
  const good = tar(selected)
  const release = fixtureRelease(good)
  const archive = cacheObject(cache, release)
  const error = captureError(() =>
    acquireVerifiedNodeArchive({
      cache,
      release,
      download({ destination }) {
        writeFileSync(destination, good)
      },
      publish(_source, destination) {
        writeFileSync(destination, "preserve")
      },
    }),
  )
  assert.match(error.message, /acquisition failed after its owned stage was removed/)
  assert.match(error.cause.message, /path identity changed/)
  assert.equal(readFileSync(archive, "utf8"), "preserve")
})

test("a publisher reporting success with a valid different inode is refused", (t) => {
  const cache = mkdtempSync(join(tmpdir(), "nessa-node-publication-substitute-"))
  t.after(() => rmSync(cache, { recursive: true, force: true }))
  const good = tar(selected)
  const release = fixtureRelease(good)
  const archive = cacheObject(cache, release)
  const error = captureError(() =>
    acquireVerifiedNodeArchive({
      cache,
      release,
      download({ destination }) {
        writeFileSync(destination, good)
      },
      publish(source, destination) {
        writeFileSync(destination, good)
        assert.notDeepEqual(
          [lstatSync(source).dev, lstatSync(source).ino],
          [lstatSync(destination).dev, lstatSync(destination).ino],
        )
      },
    }),
  )
  assert.match(error.message, /acquisition failed after its owned stage was removed/)
  assert.match(error.cause.message, /path identity changed/)
  assert.deepEqual(readFileSync(archive), good)
})

test("a substituted download stage is preserved without following it", (t) => {
  const root = mkdtempSync(join(tmpdir(), "nessa-node-stage-substitution-"))
  t.after(() => rmSync(root, { recursive: true, force: true }))
  const cache = join(root, "cache")
  const external = join(root, "external")
  mkdirSync(cache)
  mkdirSync(external)
  writeFileSync(join(external, "marker"), "preserve")
  const good = tar(selected)
  const release = fixtureRelease(good)
  let stage
  let moved
  const error = captureError(() =>
    acquireVerifiedNodeArchive({
      cache,
      release,
      download({ destination }) {
        writeFileSync(destination, good)
        stage = dirname(destination)
        moved = `${stage}-moved`
        renameSync(stage, moved)
        symlinkSync(external, stage)
      },
    }),
  )
  assert.equal(error instanceof AggregateError, true)
  assert.match(error.message, /acquisition and owned-stage cleanup both failed/)
  assert.equal(error.errors.length, 2)
  assert.match(error.errors[0].message, /stage identity changed/)
  assert.match(error.errors[1].message, /stage identity changed/)
  assert.equal(lstatSync(stage).isSymbolicLink(), true)
  assert.deepEqual(readFileSync(join(moved, release.archive)), good)
  assert.equal(readFileSync(join(external, "marker"), "utf8"), "preserve")
})

test("a substituted staged path is preserved without following it", (t) => {
  const root = mkdtempSync(join(tmpdir(), "nessa-node-path-substitution-"))
  t.after(() => rmSync(root, { recursive: true, force: true }))
  const cache = join(root, "cache")
  const external = join(root, "external")
  mkdirSync(cache)
  writeFileSync(external, "preserve")
  const good = tar(selected)
  const release = fixtureRelease(good)
  let destination
  const error = captureError(() =>
    acquireVerifiedNodeArchive({
      cache,
      release,
      download({ destination: path }) {
        destination = path
        symlinkSync(external, destination)
      },
    }),
  )
  assert.equal(error instanceof AggregateError, true)
  assert.match(error.message, /acquisition and owned-stage cleanup both failed/)
  assert.equal(error.errors.length, 2)
  assert.match(error.errors[0].message, /symbolic link/)
  assert.match(error.errors[1].message, /stage contains an unowned entry/)
  assert.equal(lstatSync(destination).isSymbolicLink(), true)
  assert.equal(readFileSync(external, "utf8"), "preserve")
})

test("a source substituted during publication is preserved and never followed", (t) => {
  const root = mkdtempSync(join(tmpdir(), "nessa-node-source-substitution-"))
  t.after(() => rmSync(root, { recursive: true, force: true }))
  const cache = join(root, "cache")
  const external = join(root, "external")
  mkdirSync(cache)
  writeFileSync(external, "preserve")
  const good = tar(selected)
  const release = fixtureRelease(good)
  let source
  const error = captureError(() =>
    acquireVerifiedNodeArchive({
      cache,
      release,
      download({ destination }) {
        writeFileSync(destination, good)
      },
      publish(staged, destination) {
        source = staged
        unlinkSync(staged)
        symlinkSync(external, staged)
        writeFileSync(destination, good)
      },
    }),
  )
  assert.equal(error instanceof AggregateError, true)
  assert.match(error.message, /acquisition and owned-stage cleanup both failed/)
  assert.equal(error.errors.length, 2)
  assert.match(error.errors[0].message, /path identity changed/)
  assert.match(error.errors[1].message, /download path identity changed/)
  assert.equal(lstatSync(source).isSymbolicLink(), true)
  assert.deepEqual(readFileSync(cacheObject(cache, release)), good)
  assert.equal(readFileSync(external, "utf8"), "preserve")
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
