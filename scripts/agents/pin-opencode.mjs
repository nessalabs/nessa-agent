// Pin the Opencode release Nessa installs, by measuring it rather than trusting it.
//
// Nessa does not install whatever Opencode publishes today; it installs the
// version it has tested, and refuses anything whose archive does not hash to
// what was recorded here. That guarantee is only as good as the digests, so
// they are produced by downloading every platform's archive and hashing it —
// never by copying a number out of a release page.
//
//   node scripts/agents/pin-opencode.mjs            # pin the latest release
//   node scripts/agents/pin-opencode.mjs 1.18.31    # pin an exact version
//
// Writes crates/nessa-server/data/agent-releases.json, which is compiled into
// the server. Review the diff: a changed digest with an unchanged version means
// a published archive was replaced, which is not something to wave through.
import { execFileSync } from "node:child_process"
import { createHash } from "node:crypto"
import { mkdirSync, readFileSync, writeFileSync, createWriteStream, rmSync } from "node:fs"
import { dirname, resolve, join } from "node:path"
import { fileURLToPath, pathToFileURL } from "node:url"
import { pipeline } from "node:stream/promises"
import { Readable } from "node:stream"

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..")
const registry = "https://registry.npmjs.org"

// The platforms Nessa will install Opencode on, named the way Rust names them
// so that the server can match on its own target without a translation table.
// Opencode publishes one npm package per platform, each holding a single
// native binary.
const PLATFORMS = [
  { operatingSystem: "macos", architecture: "aarch64", package: "opencode-darwin-arm64" },
  { operatingSystem: "macos", architecture: "x86_64", package: "opencode-darwin-x64" },
  { operatingSystem: "linux", architecture: "aarch64", package: "opencode-linux-arm64" },
  { operatingSystem: "linux", architecture: "x86_64", package: "opencode-linux-x64" },
]

// Where the executable sits inside every one of those packages. Asserted below
// rather than assumed: if a future release moves it, this script fails instead
// of writing a pin that installs nothing.
const EXECUTABLE = "package/bin/opencode"

async function json(url) {
  const response = await fetch(url)
  if (!response.ok) throw new Error(`${url} answered ${response.status}`)
  return response.json()
}

async function digestOf(url, scratch) {
  const response = await fetch(url)
  if (!response.ok) throw new Error(`${url} answered ${response.status}`)
  const hash = createHash("sha256")
  const file = createWriteStream(scratch)
  // Hashed while it streams to disk rather than read back for it, so neither
  // pass holds the archive in memory. The listing below re-reads the file, so
  // what it inspects is the measured archive only as long as nothing else on
  // this machine writes to the scratch directory while the script runs — which
  // is a maintainer's own machine, and is why that is acceptable here and not
  // in the installer.
  await pipeline(
    Readable.fromWeb(response.body),
    async function* (source) {
      for await (const chunk of source) {
        hash.update(chunk)
        yield chunk
      }
    },
    file,
  )
  return hash.digest("hex")
}

// Whether the archive holds the pinned executable as a regular file with bytes
// in it.
//
// A name in the listing is not enough. A symbolic link, a hard link and a
// directory all list under their own name and all carry no data, so an archive
// holding one of those would pin cleanly and install an empty file. `-tvzf`
// prints the mode, whose first character says which of those it is.
//
// The size is checked for the same reason, and it is not redundant with the
// mode. The installer refuses a zero-length entry outright, so a release whose
// executable is a placeholder would pin here and then fail for every user on
// every platform — which is the exact failure this function exists to make
// impossible. The maintainer running this script is the only person who can
// still do something about it.
export function containsExecutable(archive) {
  // Listed with the platform's own tar rather than a dependency: this script
  // runs on a maintainer's machine, not in the app.
  const listing = execFileSync("tar", ["-tvzf", archive], { encoding: "utf8" })
  return listing.split("\n").some((line) => {
    const fields = line.trim().split(/\s+/)
    const [mode] = fields
    if (!mode || !mode.startsWith("-")) return false
    if (fields.at(-1)?.replace(/^\.\//, "") !== EXECUTABLE) return false
    // `-tvzf` prints mode, owner, size, then the date and the name. Both GNU
    // and BSD tar put the size third from the left, and an unparsable one is
    // treated as a listing this check cannot vouch for.
    const size = Number(fields[2])
    return Number.isInteger(size) && size > 0
  })
}

// The `integrity` algorithms this script knows how to check.
const INTEGRITY_ALGORITHMS = ["sha512", "sha384", "sha256"]

// Check a downloaded archive against the checksums npm published for it.
//
// Both fields are optional in the registry's own schema, so a missing one is
// not a failure — but a present one that disagrees is, because the two ways to
// get here are a download that arrived wrong and an archive that is not the one
// the metadata describes, and neither should become a pin.
export function agreesWithRegistry(archive, dist, named) {
  const bytes = readFileSync(archive)
  const integrity = dist?.integrity
  if (typeof integrity === "string") {
    const [algorithm, expected] = integrity.split("-", 2)
    // A closed set, because `createHash` throws on a name it does not know and
    // a registry that starts publishing a new one should not break pinning.
    if (INTEGRITY_ALGORITHMS.includes(algorithm) && expected) {
      const actual = createHash(algorithm).update(bytes).digest("base64")
      if (actual !== expected)
        throw new Error(`${named} does not match the integrity the registry published`)
    }
  }
  const shasum = dist?.shasum
  if (typeof shasum === "string") {
    const actual = createHash("sha1").update(bytes).digest("hex")
    if (actual !== shasum)
      throw new Error(`${named} does not match the shasum the registry published`)
  }
}

// Measure every platform's archive and write the pin file.
async function pin() {
  // Encoded, so that an argument with a slash in it asks the registry about a
  // version of this package and not about a different package entirely.
  const requested = encodeURIComponent(process.argv[2] ?? "latest")
  const metadata = await json(`${registry}/opencode-ai/${requested}`)
  const version = metadata.version
  if (!version) throw new Error("the registry did not name a version")

  const scratchDirectory = join(root, "target/agent-pins")
  mkdirSync(scratchDirectory, { recursive: true })

  const releases = []
  for (const platform of PLATFORMS) {
    const detail = await json(`${registry}/${platform.package}/${version}`)
    const archive = detail.dist?.tarball
    if (!archive) throw new Error(`${platform.package}@${version} has no tarball`)
    // Parsed rather than matched on a prefix, which `https://` alone would
    // pass while naming nothing at all. The same rule is applied again when the
    // pin is compiled in; this is the earlier of the two places.
    if (new URL(archive).protocol !== "https:")
      throw new Error(`${platform.package}@${version} is not served over https`)
    const scratch = join(scratchDirectory, `${platform.package}.tgz`)
    process.stderr.write(`measuring ${platform.package}@${version}\n`)
    const digest = await digestOf(archive, scratch)
    // The registry publishes its own checksum for these bytes, in the metadata
    // already in hand. Comparing them is the only thing in this chain that can
    // catch a download that arrived wrong: everything downstream checks that
    // the *same* bytes arrive again, so a bad measurement made here would be
    // pinned permanently and would verify perfectly forever.
    agreesWithRegistry(scratch, detail.dist, `${platform.package}@${version}`)
    if (!containsExecutable(scratch))
      throw new Error(
        `${platform.package}@${version} does not contain ${EXECUTABLE} as a file`,
      )
    rmSync(scratch, { force: true })
    releases.push({
      operatingSystem: platform.operatingSystem,
      architecture: platform.architecture,
      version,
      archiveUrl: archive,
      archiveDigest: digest,
      executable: EXECUTABLE,
    })
  }

  const destination = join(root, "crates/nessa-server/data/agent-releases.json")
  mkdirSync(resolve(destination, ".."), { recursive: true })
  const document = {
    comment:
      "Generated by scripts/agents/pin-opencode.mjs. Digests are measured from the published archives; do not edit by hand.",
    agents: { opencode: releases },
  }
  writeFileSync(destination, `${JSON.stringify(document, null, 2)}\n`)
  process.stderr.write(`pinned opencode ${version} for ${releases.length} platforms\n`)
}

const invoked = process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href
if (invoked) await pin()
