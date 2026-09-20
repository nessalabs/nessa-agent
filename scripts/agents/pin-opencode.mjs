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
import {
  createReadStream,
  createWriteStream,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs"
import { tmpdir } from "node:os"
import { dirname, resolve, join } from "node:path"
import { fileURLToPath, pathToFileURL } from "node:url"
import { pipeline } from "node:stream/promises"
import { Readable } from "node:stream"

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..")
const registry = "https://registry.npmjs.org"

// Every build Nessa will install Opencode from, named the way Rust names the
// operating system and architecture so that the server can match on its own
// target without a translation table.
//
// There is one npm package per build, not per platform, and the difference
// matters: Opencode publishes nine of them for four platforms, because a build
// is also fixed to a C library and to a processor baseline, and every one of
// them holds a binary called `opencode`. Choosing by operating system and
// architecture alone picks one of up to four at random from the machine's point
// of view, and two of those four do not start at all — a glibc build on a
// musl-only machine dies in the loader, and an AVX2 build on a processor
// without it dies on an illegal instruction.
//
// `libc` is null where the platform has only one, which is every platform here
// but Linux.
//
// Six packages, not nine: the three `-baseline` ones are deliberately absent,
// and the reason is that nobody can currently say what they need. Opencode
// names them as the builds for a processor without AVX2, and at 1.18.31 each
// publishes an executable byte-identical to its plain sibling — linux-x64 and
// linux-x64-baseline both sha256 f9dab322…, darwin-x64 and its baseline
// 9cd3d83b…, linux-x64-musl and its baseline b4a7415a…. One file cannot both
// need AVX2 and not need it, so one claim in each pair is false, and which one
// cannot be read off the bytes:
//
//   - The file does contain AVX2 — 2,151 AVX2-only instructions in the linux
//     build. But they are 0.015% of its 14.2 million, and they sit in four
//     twentieths of `.text` with 90% in two. That is the shape of
//     runtime-dispatched SIMD kernels (simdutf, zlib-ng, the engine's string
//     paths), not of a whole program compiled for Haswell, and it is evidence
//     for `requiresAvx2: false` on both.
//   - Evidence, not proof. Clustering is consistent with a `cpuid` check in
//     front of every one of those sites; it does not demonstrate one. Settling
//     it needs the binary run on a processor without AVX2, and no such machine
//     or emulator was available here.
//
// So the pin says neither. It offers no x86-64 build to a machine without
// AVX2, which leaves such a machine with "nessa has no tested opencode release
// this machine can run" — true under either reading, since no build has been
// run on one. Claiming `false` would be asserting the untested half; claiming
// a `-baseline` package would be offering bytes under a name that does not
// describe them. This is the one resolution that cannot end in an illegal
// instruction after a 180 MB download.
//
// `sameBinaryUnderDifferentClaims` is what found this and is why it stays: it
// hashes what each archive actually holds and refuses a release whose builds
// say different things about the same bytes. Running the generator against
// 1.18.31 with the baseline packages in this table throws, by design. When a
// later release publishes baseline builds that differ from their siblings —
// or when somebody runs this one on a pre-AVX2 machine and reports what
// happened — they belong back in this table, with `requiresAvx2` set to
// whatever that settled.
//
// Windows is deliberately absent. Opencode publishes builds for it, but nothing
// in Nessa launches an agent runtime on Windows yet, and pinning a platform
// that is never installed would be claiming a test that never ran.
//
// One thing this table still cannot say, and which therefore is not checked
// before a runtime is launched: a glibc build has a minimum glibc *version*, so
// an old distribution gets a `GLIBC_2.xx not found` from the loader. That is
// the failure this whole mechanism exists to prevent, on machines old enough
// that the vendor does not describe them either. Saying so here is the honest
// alternative to implying the check is complete.
export const PLATFORMS = [
  {
    operatingSystem: "macos",
    architecture: "aarch64",
    libc: null,
    requiresAvx2: false,
    package: "opencode-darwin-arm64",
  },
  {
    operatingSystem: "macos",
    architecture: "x86_64",
    libc: null,
    requiresAvx2: true,
    package: "opencode-darwin-x64",
  },
  {
    operatingSystem: "linux",
    architecture: "aarch64",
    libc: "gnu",
    requiresAvx2: false,
    package: "opencode-linux-arm64",
  },
  {
    operatingSystem: "linux",
    architecture: "aarch64",
    libc: "musl",
    requiresAvx2: false,
    package: "opencode-linux-arm64-musl",
  },
  {
    operatingSystem: "linux",
    architecture: "x86_64",
    libc: "gnu",
    requiresAvx2: true,
    package: "opencode-linux-x64",
  },
  {
    operatingSystem: "linux",
    architecture: "x86_64",
    libc: "musl",
    requiresAvx2: true,
    package: "opencode-linux-x64-musl",
  },
]

// Where the executable sits inside every one of those packages. Asserted below
// rather than assumed: if a future release moves it, this script fails instead
// of writing a pin that installs nothing.
export const EXECUTABLE = "package/bin/opencode"

// What the first bytes of a program look like on the platforms this script
// pins. ELF for Linux; Mach-O for macOS, thin in either byte order and fat,
// because a universal binary is a legal thing for a vendor to ship even where
// Opencode currently does not.
//
// Deliberately no PE: no Windows build is pinned, and listing a magic number
// for a platform nothing has ever generated would read as support that does
// not exist.
const PROGRAM_MAGIC = [
  [0x7f, 0x45, 0x4c, 0x46],
  [0xcf, 0xfa, 0xed, 0xfe],
  [0xfe, 0xed, 0xfa, 0xcf],
  [0xca, 0xfe, 0xba, 0xbe],
  [0xbe, 0xba, 0xfe, 0xca],
]

// Whether `bytes` begins the way a program does.
function isProgram(bytes) {
  return PROGRAM_MAGIC.some((magic) => magic.every((byte, at) => bytes[at] === byte))
}

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
//
// And the bytes are checked to be a program, because the worst version of this
// failure is the one that succeeds. Every one of these archives holds a
// `package/package.json`; an entry naming that passes the mode check, passes
// the size check, unpacks, records and gets reported as the installed runtime
// — a hundred and forty bytes of JSON announced as the tested Opencode. Four
// magic bytes are the whole of the check and they are already in hand here,
// which is the only moment in this script they are.
export function executableDigest(archive) {
  // Listed with the platform's own tar rather than a dependency: this script
  // runs on a maintainer's machine, not in the app.
  const listing = execFileSync("tar", ["-tvzf", archive], { encoding: "utf8" })
  const named = listing.split("\n").find((line) => {
    const fields = line.trim().split(/\s+/)
    const [mode] = fields
    if (!mode || !mode.startsWith("-")) return false
    return fields.at(-1)?.replace(/^\.\//, "") === EXECUTABLE
  })
  if (!named) return null

  // The size is measured by extracting the entry, not by reading a column out
  // of the listing. GNU tar prints `mode owner/group size date name` and BSD
  // tar prints `mode links owner group size date name`, so the column that
  // holds the size on one holds a link count on the other — and a check that
  // reads the link count would refuse every good archive on macOS.
  //
  // Extracted under the name the listing gave, rather than under the pin's
  // spelling of it, so an entry written as `./package/bin/opencode` is asked
  // for the way it is actually stored.
  const stored = named.trim().split(/\s+/).at(-1)
  const extracted = mkdtempSync(join(tmpdir(), "nessa-entry-"))
  try {
    execFileSync("tar", ["-xzf", archive, "-C", extracted, stored])
    const entry = join(extracted, stored)
    if (statSync(entry).size === 0) return null
    // Hashed while it is here, because it is the only moment the bytes that
    // will actually be launched exist in this script. The archive digest says
    // nothing about them: two archives can differ in a `package.json` field and
    // hold the same binary, which is the case [`sameBinaryUnderDifferentClaims`]
    // exists to catch.
    const bytes = readFileSync(entry)
    if (!isProgram(bytes)) return null
    return createHash("sha256").update(bytes).digest("hex")
  } finally {
    // The entry is a hundred megabytes, and this runs once per platform.
    rmSync(extracted, { recursive: true, force: true })
  }
}

// Refuse a release whose builds claim different things about the same bytes.
//
// The pin's whole purpose is that a machine is offered a build it can actually
// run, and the two claims that decide that — the C library and whether AVX2 is
// required — are read off the package's *name* in `PLATFORMS`. A name is not a
// measurement. Opencode publishes `opencode-linux-x64` and
// `opencode-linux-x64-baseline`, and if the two hold the same executable then
// one of the two pins is stating something that is not true of the file it
// points at: either a processor without AVX2 is being offered a build that
// needs it, which is the illegal instruction this table exists to prevent, or
// a requirement is being claimed that the bytes do not have and the preference
// between the pair decides nothing.
//
// Which of the two it is cannot be told from here, and that is the point: this
// stops rather than guesses, and names the packages so whoever is pinning can
// settle it with the vendor. `pin-opencode` says in its own header that the
// digests are measured; this is what makes the requirements measured too.
export function sameBinaryUnderDifferentClaims(measured) {
  const claim = (build) => `${build.libc ?? "no libc"}, avx2=${build.requiresAvx2}`
  const byBinary = new Map()
  for (const build of measured) {
    const seen = byBinary.get(build.digest)
    if (!seen) {
      byBinary.set(build.digest, build)
      continue
    }
    if (claim(seen) === claim(build)) continue
    throw new Error(
      `${build.package} and ${seen.package} hold the same ${EXECUTABLE} ` +
        `(sha256 ${build.digest}) but are pinned as "${claim(build)}" and ` +
        `"${claim(seen)}"; one of those claims is not true of the file. ` +
        `Settle which with the vendor before pinning this release.`,
    )
  }
}

// The `integrity` algorithms this script knows how to check.
const INTEGRITY_ALGORITHMS = ["sha512", "sha384", "sha256"]

// Check a downloaded archive against the checksums npm published for it.
//
// Both fields are optional in the registry's own schema, so a missing one is
// not a failure — but a present one that disagrees is, because the two ways to
// get here are a download that arrived wrong and an archive that is not the one
// the metadata describes, and neither should become a pin.
export async function agreesWithRegistry(archive, dist, named) {
  const integrity = dist?.integrity
  const [algorithm, expected] =
    typeof integrity === "string" ? integrity.split("-", 2) : []
  const shasum = dist?.shasum

  // A closed set, because `createHash` throws on a name it does not know and a
  // registry that starts publishing a new one should not break pinning.
  //
  // `split("-", 2)` truncates rather than rejoining, so a hash containing a
  // hyphen would be compared against its own first segment and refused. That
  // is the safe direction, and it is unreachable anyway: `integrity` is
  // standard base64, whose alphabet has no hyphen.
  const checkable = INTEGRITY_ALGORITHMS.includes(algorithm) && expected
  if (!checkable && typeof shasum !== "string") {
    // Said out loud rather than passed over, because a skipped check and a
    // passed one look the same from outside, and this is the only check in the
    // chain that can catch a download that arrived wrong.
    process.stderr.write(`${named} published no checksum to cross-check against\n`)
    return
  }

  // Streamed, for the reason `digestOf` gives: an archive is around fifty
  // megabytes and neither pass over it should hold one in memory. Held by name
  // rather than in a list, so that neither check depends on whether the other
  // one is running.
  const integrityHash = checkable ? createHash(algorithm) : undefined
  const shasumHash = typeof shasum === "string" ? createHash("sha1") : undefined
  await pipeline(createReadStream(archive), async function (source) {
    for await (const chunk of source) {
      integrityHash?.update(chunk)
      shasumHash?.update(chunk)
    }
  })

  if (integrityHash && integrityHash.digest("base64") !== expected)
    throw new Error(`${named} does not match the integrity the registry published`)
  if (shasumHash && shasumHash.digest("hex") !== shasum)
    throw new Error(`${named} does not match the shasum the registry published`)
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
  // What each package's executable actually is, so the claims each pin makes
  // can be checked against the bytes rather than against the package's name.
  const measured = []
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
    await agreesWithRegistry(scratch, detail.dist, `${platform.package}@${version}`)
    const executableDigestValue = executableDigest(scratch)
    if (!executableDigestValue)
      throw new Error(
        `${platform.package}@${version} does not contain ${EXECUTABLE} as a file`,
      )
    rmSync(scratch, { force: true })
    measured.push({
      package: platform.package,
      libc: platform.libc,
      requiresAvx2: platform.requiresAvx2,
      digest: executableDigestValue,
    })
    releases.push({
      operatingSystem: platform.operatingSystem,
      architecture: platform.architecture,
      // Written out even when there is nothing to require, because this file is
      // read by people reviewing a pin: an entry that simply omits them leaves
      // "this build runs anywhere on its platform" and "whoever generated this
      // forgot" looking identical.
      libc: platform.libc,
      requiresAvx2: platform.requiresAvx2,
      version,
      archiveUrl: archive,
      archiveDigest: digest,
      executable: EXECUTABLE,
    })
  }

  sameBinaryUnderDifferentClaims(measured)

  const destination = join(root, "crates/nessa-server/data/agent-releases.json")
  mkdirSync(resolve(destination, ".."), { recursive: true })
  const document = {
    comment:
      "Generated by scripts/agents/pin-opencode.mjs. Digests are measured from the published archives; do not edit by hand.",
    agents: { opencode: releases },
  }
  writeFileSync(destination, `${JSON.stringify(document, null, 2)}\n`)
  process.stderr.write(`pinned opencode ${version} across ${releases.length} builds\n`)
}

const invoked = process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href
if (invoked) await pin()
