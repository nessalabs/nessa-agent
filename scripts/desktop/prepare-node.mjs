import { execFileSync } from "node:child_process"
import { createHash } from "node:crypto"
import {
  chmodSync,
  cpSync,
  existsSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  renameSync,
  rmSync,
  writeFileSync,
} from "node:fs"
import { dirname, join, posix, win32 } from "node:path"
import { gunzipSync } from "node:zlib"

export const NODE_VERSION = "26.8.1"

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

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex")
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
export function readNodeArchive(bytes, distribution) {
  let tar
  try {
    tar = gunzipSync(bytes)
  } catch (error) {
    throw new Error("Node archive is not a valid gzip stream", { cause: error })
  }
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

function downloadNodeArchive({ destination, url }) {
  execFileSync("curl", ["--fail", "--location", "--output", destination, url], {
    stdio: "inherit",
  })
}

/** Return pinned archive bytes, replacing only a corrupt entry in the owned cache. */
export function acquireVerifiedNodeArchive({ cache, release, download }) {
  mkdirSync(cache, { recursive: true })
  if (!lstatSync(cache).isDirectory())
    throw new Error("Node archive cache must be an owned directory")
  const archive = join(cache, release.archive)
  if (existsSync(archive)) {
    const stat = lstatSync(archive)
    if (!stat.isFile() || sha256(readFileSync(archive)) !== release.sha256)
      rmSync(archive, { recursive: true })
  }
  if (!existsSync(archive)) {
    const downloadStage = mkdtempSync(join(cache, ".node-download-"))
    try {
      const downloaded = join(downloadStage, release.archive)
      download({ destination: downloaded, url: release.url })
      if (!lstatSync(downloaded).isFile())
        throw new Error("Node download did not produce a regular archive")
      const bytes = readFileSync(downloaded)
      if (sha256(bytes) !== release.sha256)
        throw new Error("Node archive checksum mismatch")
      renameSync(downloaded, archive)
    } finally {
      rmSync(downloadStage, { recursive: true, force: true })
    }
  }
  const bytes = readFileSync(archive)
  if (sha256(bytes) !== release.sha256) throw new Error("Node archive checksum mismatch")
  return bytes
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
