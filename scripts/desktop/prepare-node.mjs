import { execFileSync } from "node:child_process"
import { createHash } from "node:crypto"
import {
  chmodSync,
  constants,
  cpSync,
  closeSync,
  fstatSync,
  linkSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  openSync,
  readdirSync,
  readSync,
  rmdirSync,
  rmSync,
  unlinkSync,
  writeFileSync,
} from "node:fs"
import { dirname, join, posix, win32 } from "node:path"
import { gunzipSync } from "node:zlib"

export const NODE_VERSION = "26.8.1"

// Official Content-Length and gzip ISIZE values for the pinned archives are
// 57,813,947/223,933,952 (Darwin arm64), 59,214,465/226,483,712 (Darwin x64),
// and 62,281,031/229,468,160 (Linux x64). These ceilings preserve every target
// with headroom while bounding cache reads, downloads, and decompression.
export const MAX_NODE_ARCHIVE_BYTES = 96 * 1024 * 1024
export const MAX_NODE_TAR_BYTES = 384 * 1024 * 1024
const NODE_DOWNLOAD_TIMEOUT_SECONDS = 120
const NODE_DOWNLOAD_PROCESS_TIMEOUT_MS = 125_000
const ARCHIVE_READ_CHUNK_BYTES = 1024 * 1024

// These are the matching entries from Node's official v26.8.1 SHASUMS256.txt.
// They are source data for the build, not values fetched while packaging.
const NODE_ARCHIVES = Object.freeze({
  "darwin-arm64": Object.freeze({
    archive: `node-v${NODE_VERSION}-darwin-arm64.tar.gz`,
    sha256: "6e577fd0d9db776db82306629e441a9dace416702622aebdd171c9dfaa41f4d2",
  }),
  "darwin-x64": Object.freeze({
    archive: `node-v${NODE_VERSION}-darwin-x64.tar.gz`,
    sha256: "fe9c6dbf9c8e1b4443803d75e2a20366e420dae650c747dbb116b22975751baf",
  }),
  "linux-x64": Object.freeze({
    archive: `node-v${NODE_VERSION}-linux-x64.tar.gz`,
    sha256: "b2b76660fa4ded4e0b2a41ee3c0c651cd52ea8170ead91ebac1e147ac3d55643",
  }),
})

/** Select the one reviewed official Node archive for a build host. */
export function nodeArchive(platform, arch) {
  const key = `${platform}-${arch}`
  if (!Object.hasOwn(NODE_ARCHIVES, key))
    throw new Error(`Bundled Node does not support ${platform} ${arch}`)
  const release = NODE_ARCHIVES[key]
  return {
    ...release,
    distribution: release.archive.slice(0, -".tar.gz".length),
    url: `https://nodejs.org/dist/v${NODE_VERSION}/${release.archive}`,
  }
}

function tarText(block, start, length) {
  const field = block.subarray(start, start + length)
  const end = field.indexOf(0)
  return field.subarray(0, end < 0 ? field.length : end).toString("utf8")
}

function tarExtensionText(bytes, kind) {
  const end = bytes.indexOf(0)
  if (end <= 0 || bytes.subarray(end + 1).some((byte) => byte !== 0))
    throw new Error(`Node archive has a malformed GNU ${kind}`)
  return bytes.subarray(0, end).toString("utf8")
}

function tarNumber(block, start, length, name) {
  const field = block.subarray(start, start + length)
  if ((field[0] & 0x80) !== 0) throw new Error(`Node archive has an unsupported ${name}`)
  const text = field.toString("ascii").replace(/\0.*$/, "").trim()
  if (!/^[0-7]+$/.test(text)) throw new Error(`Node archive has an invalid ${name}`)
  const value = Number.parseInt(text, 8)
  if (!Number.isSafeInteger(value)) throw new Error(`Node archive has an invalid ${name}`)
  return value
}

function safeArchivePath(name) {
  const canonical = name.endsWith("/") ? name.slice(0, -1) : name
  if (
    canonical.length === 0 ||
    canonical.includes("\\") ||
    posix.isAbsolute(canonical) ||
    win32.isAbsolute(canonical) ||
    canonical.split("/").some((part) => part === ".." || part === "")
  ) {
    throw new Error(`Node archive contains an unsafe path: ${JSON.stringify(name)}`)
  }
  return canonical
}

function safeLink(entry, target, hardLink, distribution) {
  if (
    target.length === 0 ||
    target.includes("\\") ||
    posix.isAbsolute(target) ||
    win32.isAbsolute(target)
  )
    throw new Error(`Node archive link escapes its distribution: ${entry}`)
  const resolved = hardLink
    ? posix.normalize(target)
    : posix.normalize(posix.join(posix.dirname(entry), target))
  if (!resolved.startsWith(`${distribution}/`))
    throw new Error(`Node archive link escapes its distribution: ${entry}`)
}

/**
 * Read the two files Nessa ships only after validating the complete tar stream.
 * No archive path is handed to an extracting program.
 */
export function readNodeArchive(
  bytes,
  distribution,
  { maxExpandedBytes = MAX_NODE_TAR_BYTES } = {},
) {
  let tar
  try {
    tar = gunzipSync(bytes, { maxOutputLength: maxExpandedBytes })
  } catch (error) {
    if (error?.code === "ERR_BUFFER_TOO_LARGE")
      throw new Error("Node archive expands beyond its allowed size", { cause: error })
    throw new Error("Node archive is not a valid gzip stream", { cause: error })
  }
  if (tar.length % 512 !== 0) throw new Error("Node archive has a malformed ending")
  const selected = new Map([
    [`${distribution}/bin/node`, undefined],
    [`${distribution}/LICENSE`, undefined],
  ])
  let offset = 0
  let ended = false
  let longName
  let longLink
  while (offset + 512 <= tar.length) {
    const header = tar.subarray(offset, offset + 512)
    offset += 512
    if (header.every((byte) => byte === 0)) {
      if (longName || longLink)
        throw new Error("Node archive ends before its GNU extension is applied")
      if (
        offset + 512 > tar.length ||
        !tar.subarray(offset, offset + 512).every((byte) => byte === 0)
      )
        throw new Error("Node archive must end with two zero blocks")
      offset += 512
      ended = true
      break
    }
    const expectedChecksum = tarNumber(header, 148, 8, "header checksum")
    let actualChecksum = 0
    for (let index = 0; index < header.length; index += 1)
      actualChecksum += index >= 148 && index < 156 ? 32 : header[index]
    if (actualChecksum !== expectedChecksum)
      throw new Error("Node archive has an invalid header checksum")

    const prefix = tarText(header, 345, 155)
    const leaf = tarText(header, 0, 100)
    const rawName = prefix ? `${prefix}/${leaf}` : leaf
    const size = tarNumber(header, 124, 12, "entry size")
    const paddedSize = Math.ceil(size / 512) * 512
    if (offset + paddedSize > tar.length) throw new Error("Node archive is truncated")
    if (tar.subarray(offset + size, offset + paddedSize).some((byte) => byte !== 0))
      throw new Error("Node archive entry has non-zero padding")
    const type = String.fromCharCode(header[156] || 48)
    if (type === "L" || type === "K") {
      if ((type === "L" && longName) || (type === "K" && longLink))
        throw new Error("Node archive repeats a GNU extension before using it")
      const value = tarExtensionText(
        tar.subarray(offset, offset + size),
        type === "L" ? "long name" : "long link",
      )
      if (type === "L") longName = value
      else longLink = value
      offset += paddedSize
      continue
    }
    const name = safeArchivePath(longName ?? rawName)
    longName = undefined
    if (!["0", "1", "2", "5"].includes(type))
      throw new Error(`Node archive contains an unsupported entry type: ${name}`)
    if (longLink && type !== "1" && type !== "2")
      throw new Error("Node archive applies a GNU long link to a non-link entry")
    if (type === "1" || type === "2") {
      safeLink(name, longLink ?? tarText(header, 157, 100), type === "1", distribution)
      longLink = undefined
    }
    if (selected.has(name)) {
      if (selected.get(name) !== undefined)
        throw new Error(`Node archive repeats a selected entry: ${name}`)
      if (type !== "0")
        throw new Error(`Node archive selected entry is not a file: ${name}`)
      selected.set(name, Buffer.from(tar.subarray(offset, offset + size)))
    }
    offset += paddedSize
  }
  if (!ended || tar.subarray(offset).some((byte) => byte !== 0))
    throw new Error("Node archive has a malformed ending")
  for (const [name, contents] of selected) {
    if (contents === undefined) throw new Error(`Node archive is missing ${name}`)
  }
  return {
    node: selected.get(`${distribution}/bin/node`),
    license: selected.get(`${distribution}/LICENSE`),
  }
}

export function downloadNodeArchive({
  destination,
  url,
  maxBytes = MAX_NODE_ARCHIVE_BYTES,
  execute = execFileSync,
}) {
  const bytes = execute(
    "curl",
    [
      "--fail",
      "--location",
      "--max-filesize",
      String(maxBytes),
      "--max-time",
      String(NODE_DOWNLOAD_TIMEOUT_SECONDS),
      "--silent",
      "--show-error",
      url,
    ],
    {
      maxBuffer: maxBytes,
      stdio: ["ignore", "pipe", "inherit"],
      timeout: NODE_DOWNLOAD_PROCESS_TIMEOUT_MS,
    },
  )
  if (!Buffer.isBuffer(bytes) || bytes.length > maxBytes)
    throw new Error("Node download exceeded its allowed size")
  writeFileSync(destination, bytes)
}

function cacheEntryKind(stat) {
  if (stat.isSymbolicLink()) return "a symbolic link"
  if (stat.isDirectory()) return "a directory"
  if (stat.isFIFO()) return "a FIFO"
  if (stat.isSocket()) return "a socket"
  if (stat.isCharacterDevice() || stat.isBlockDevice()) return "a device"
  return "a nonregular file"
}

function refuseNonregularCacheEntry(path, stat) {
  throw new Error(`Node archive path is ${cacheEntryKind(stat)}: ${path}`)
}

function pathIdentity(path, { allowAbsent = false } = {}) {
  let pathStat
  try {
    pathStat = lstatSync(path)
  } catch (error) {
    if (allowAbsent && error?.code === "ENOENT") return undefined
    throw error
  }
  if (!pathStat.isFile()) refuseNonregularCacheEntry(path, pathStat)
  return { dev: pathStat.dev, ino: pathStat.ino }
}

function inspectArchive(path, expectedSha256, { allowAbsent = false, identity } = {}) {
  const initialIdentity = pathIdentity(path, { allowAbsent })
  if (!initialIdentity) return undefined
  if (identity && !sameIdentity(initialIdentity, identity))
    throw new Error(`Node archive path identity changed: ${path}`)

  let descriptor
  try {
    descriptor = openSync(
      path,
      constants.O_RDONLY | constants.O_NONBLOCK | constants.O_NOFOLLOW,
    )
  } catch (error) {
    if (allowAbsent && error?.code === "ENOENT") return undefined
    if (error?.code === "ELOOP")
      throw new Error(`Node archive cache entry changed to a symbolic link: ${path}`, {
        cause: error,
      })
    throw new Error(`Node archive path could not be opened safely: ${path}`, {
      cause: error,
    })
  }
  try {
    const before = fstatSync(descriptor)
    if (!before.isFile()) refuseNonregularCacheEntry(path, before)
    const openedIdentity = { dev: before.dev, ino: before.ino }
    if (!sameIdentity(initialIdentity, openedIdentity))
      throw new Error(`Node archive path identity changed while opening: ${path}`)
    if (before.size > MAX_NODE_ARCHIVE_BYTES)
      throw new Error(`Node archive exceeds its compressed-size limit: ${path}`)

    const hash = createHash("sha256")
    const chunks = []
    let size = 0
    while (true) {
      const chunk = Buffer.allocUnsafe(ARCHIVE_READ_CHUNK_BYTES)
      const count = readSync(descriptor, chunk, 0, chunk.length, null)
      if (count === 0) break
      size += count
      if (size > MAX_NODE_ARCHIVE_BYTES)
        throw new Error(`Node archive grew beyond its compressed-size limit: ${path}`)
      const used = chunk.subarray(0, count)
      hash.update(used)
      chunks.push(Buffer.from(used))
    }
    const after = fstatSync(descriptor)
    const finalPathIdentity = pathIdentity(path)
    if (
      !sameIdentity(openedIdentity, { dev: after.dev, ino: after.ino }) ||
      !sameIdentity(openedIdentity, finalPathIdentity) ||
      after.size !== size
    )
      throw new Error(`Node archive path identity changed while reading: ${path}`)
    if (hash.digest("hex") !== expectedSha256)
      throw new Error(`Node archive does not match its pinned digest: ${path}`)
    return { bytes: Buffer.concat(chunks, size), identity: openedIdentity }
  } finally {
    closeSync(descriptor)
  }
}

function sameIdentity(left, right) {
  return left.dev === right.dev && left.ino === right.ino
}

export function nodeCacheObjectName(release) {
  if (!/^[0-9a-f]{64}$/.test(release.sha256))
    throw new Error("Node archive descriptor has an invalid SHA-256 digest")
  return `${release.sha256}.tar.gz`
}

function directoryIdentity(path) {
  const stat = lstatSync(path)
  if (!stat.isDirectory() || stat.isSymbolicLink())
    throw new Error(`Node download stage identity changed: ${path}`)
  return { dev: stat.dev, ino: stat.ino }
}

function cleanupDownloadStage({
  downloaded,
  downloadedIdentity,
  removeDownloaded,
  stage,
  stageIdentity,
}) {
  let currentStage
  try {
    currentStage = directoryIdentity(stage)
  } catch (error) {
    if (error?.code === "ENOENT") return
    throw error
  }
  if (!sameIdentity(currentStage, stageIdentity))
    throw new Error(`Node download stage identity changed: ${stage}`)

  if (downloadedIdentity) {
    let currentDownload
    try {
      currentDownload = pathIdentity(downloaded, { allowAbsent: true })
    } catch (error) {
      throw new Error(`Node download path identity changed: ${downloaded}`, {
        cause: error,
      })
    }
    if (currentDownload) {
      if (!sameIdentity(currentDownload, downloadedIdentity))
        throw new Error(`Node download path identity changed: ${downloaded}`)
      removeDownloaded(downloaded)
    }
  }
  if (readdirSync(stage).length !== 0)
    throw new Error(`Node download stage contains an unowned entry: ${stage}`)
  rmdirSync(stage)
}

/**
 * Return one immutable digest-named cache object.
 *
 * Exclusive publication coordinates cooperative writers. A process with write
 * access to the cache can still replace the object after this function returns;
 * callers trust only the bytes verified and returned by this invocation.
 */
export function acquireVerifiedNodeArchive({
  cache,
  release,
  download = downloadNodeArchive,
  publish = linkSync,
  removeDownloaded = unlinkSync,
}) {
  mkdirSync(cache, { recursive: true })
  if (!lstatSync(cache).isDirectory())
    throw new Error("Node archive cache must be an owned directory")
  const archive = join(cache, nodeCacheObjectName(release))
  let cached
  try {
    cached = inspectArchive(archive, release.sha256, { allowAbsent: true })
  } catch (cause) {
    throw new Error(
      `Node cache object validation failed; this invocation made no mutation at ${archive}`,
      { cause },
    )
  }
  if (cached) return cached.bytes

  const downloadStage = mkdtempSync(join(cache, ".node-download-"))
  const stageIdentity = directoryIdentity(downloadStage)
  const downloaded = join(downloadStage, release.archive)
  let downloadedIdentity
  let result
  let failure
  try {
    download({
      destination: downloaded,
      maxBytes: MAX_NODE_ARCHIVE_BYTES,
      timeoutSeconds: NODE_DOWNLOAD_TIMEOUT_SECONDS,
      url: release.url,
    })
    if (!sameIdentity(directoryIdentity(downloadStage), stageIdentity))
      throw new Error(`Node download stage identity changed: ${downloadStage}`)
    downloadedIdentity = pathIdentity(downloaded)
    const staged = inspectArchive(downloaded, release.sha256, {
      identity: downloadedIdentity,
    })
    try {
      publish(downloaded, archive)
    } catch (error) {
      if (error?.code !== "EEXIST") throw error
      result = inspectArchive(archive, release.sha256).bytes
    }
    if (!result) {
      const published = inspectArchive(archive, release.sha256, {
        identity: staged.identity,
      })
      result = published.bytes
    }
  } catch (error) {
    failure = error
  }
  let cleanupFailure
  try {
    cleanupDownloadStage({
      downloaded,
      downloadedIdentity,
      removeDownloaded,
      stage: downloadStage,
      stageIdentity,
    })
  } catch (error) {
    cleanupFailure = error
  }
  if (failure && cleanupFailure)
    throw new AggregateError(
      [failure, cleanupFailure],
      `Node acquisition and owned-stage cleanup both failed at ${downloadStage}`,
    )
  if (failure)
    throw new Error(
      `Node acquisition failed after its owned stage was removed at ${downloadStage}`,
      { cause: failure },
    )
  if (cleanupFailure)
    throw new Error(
      `Node acquisition succeeded but owned-stage cleanup failed at ${downloadStage}`,
      { cause: cleanupFailure },
    )
  return result
}

/** Acquire, verify, and stage Node plus its license into a desktop runtime. */
export function prepareBundledNode({
  arch,
  cache,
  executable,
  out,
  platform,
  download = downloadNodeArchive,
}) {
  const release = nodeArchive(platform, arch)
  const bytes = acquireVerifiedNodeArchive({ cache, release, download })
  const files = readNodeArchive(bytes, release.distribution)
  const extractionStage = mkdtempSync(join(cache, ".node-extract-"))
  try {
    const stagedNode = join(extractionStage, "node")
    const stagedLicense = join(extractionStage, "LICENSE")
    writeFileSync(stagedNode, files.node)
    chmodSync(stagedNode, 0o755)
    writeFileSync(stagedLicense, files.license)
    mkdirSync(dirname(join(out, executable)), { recursive: true })
    cpSync(stagedNode, join(out, executable))
    cpSync(stagedLicense, join(out, "NODE-LICENSE"))
  } finally {
    rmSync(extractionStage, { recursive: true, force: true })
  }
  return NODE_VERSION
}
