// Pin the agent runtimes Nessa installs, by measuring them rather than trusting them.
//
// Nessa does not install whatever an agent publishes today; it installs the
// version it has tested, and refuses anything whose archive does not hash to
// what was recorded here. That guarantee is only as good as the digests, so
// they are produced by downloading every build's archive and hashing it —
// never by copying a number out of a release page.
//
//   node scripts/agents/pin-agents.mjs            # pin every agent
//   node scripts/agents/pin-agents.mjs 1.18.31    # pin an exact opencode version
//
// Three agents, and they do not get their versions the same way, because they
// are not in the same situation. Opencode's comes from the registry: Nessa
// chooses which version of it to test. Claude's and Codex's come from the
// harness lockfiles, because their JavaScript ships inside the application and
// expects the native package that `npm ci` resolved beside it — a pin that
// named a different version would install a binary the bundled wrapper is not
// the wrapper for. So those two are read rather than chosen, and the argument
// above applies only to Opencode.
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
export const OPENCODE_BUILDS = [
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

// Every agent Nessa installs, and what is pinned for each.
//
// `install` is the part that changed when Claude and Codex stopped shipping
// inside the application, and it is worth stating plainly, because it is two
// different shapes of release rather than one with an option:
//
//   - "launch"  — the archive holds one program and nothing the program needs.
//                 That is Opencode: `package/bin/opencode` and some metadata
//                 nothing reads. Pinning the metadata would claim it matters.
//   - "package" — the archive is a package whose parts find each other. Codex's
//                 `codex` looks for its ripgrep at `../codex-path/rg` and a zsh
//                 under `../codex-resources/`, both relative to itself, so
//                 installing only the program installs something that starts
//                 and then cannot search or run a command. Every regular file
//                 in the archive is pinned, and the archive's own layout is
//                 kept.
//
// Claude's package is one program and three documents and would be correct
// either way; it is pinned as a package because that is what it is, and because
// the bundled wrapper resolves it as a package directory rather than as a path
// to a binary.
//
// What no build here says is which *platform* other than macOS on Apple silicon
// Claude and Codex are pinned for, and the answer is none. The 467 MB measured
// for this change was measured on a darwin-arm64 build, and no other platform's
// archive has been fetched, listed or checked. Adding one is a line in this
// table and a run of this script; claiming one without that would be claiming a
// measurement nobody took. Opencode keeps its six because they were measured.
export const AGENTS = [
  {
    name: "opencode",
    // The version Nessa chooses to test, from the registry, defaulting to the
    // newest. The argument to this script is this and nothing else.
    version: { from: "registry", package: "opencode-ai" },
    launch: EXECUTABLE,
    install: "launch",
    builds: OPENCODE_BUILDS,
  },
  {
    name: "claude",
    version: {
      from: "lockfile",
      harness: "claude-acp",
      dependency: "@anthropic-ai/claude-agent-sdk-darwin-arm64",
    },
    launch: "package/claude",
    install: "package",
    builds: [
      {
        operatingSystem: "macos",
        architecture: "aarch64",
        libc: null,
        requiresAvx2: false,
        package: "@anthropic-ai/claude-agent-sdk-darwin-arm64",
      },
    ],
  },
  {
    name: "codex",
    version: {
      from: "lockfile",
      harness: "codex-acp",
      // The lockfile's own name for it. There is no `@openai/codex-darwin-arm64`
      // package on the registry: `@openai/codex` declares this as an optional
      // dependency aliased to itself at a platform-suffixed version, so npm
      // writes the alias under this name and resolves it to the tarball of
      // `@openai/codex@<version>-darwin-arm64`. The `package` below is
      // therefore the real coordinate and this is the key to look it up by.
      dependency: "@openai/codex-darwin-arm64",
    },
    launch: "package/vendor/aarch64-apple-darwin/bin/codex",
    install: "package",
    builds: [
      {
        operatingSystem: "macos",
        architecture: "aarch64",
        libc: null,
        requiresAvx2: false,
        package: "@openai/codex",
      },
    ],
  },
]

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

// The digest of the pinned executable, or the reason this archive has none.
//
// Answered as `{ digest }` or `{ refusal }` rather than as a digest or `null`,
// because there are four ways to have none and they are four different things
// for the maintainer to do: chase a packaging change, chase a release that
// shipped a placeholder, chase one whose executable is not a program, or read
// a listing that no longer looks the way this expects. One message covering
// all four has to be vague enough to fit the one that is not true of the
// archive in hand.
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
// The name a listing line gives an entry, whatever kind of entry it is.
//
// The last field is the name only for a regular file. A directory lists with a
// trailing slash, a symbolic link lists as `name -> target`, and a hard link
// as `name link to target`, in both GNU and BSD tar — so reading the last
// field would compare the executable's name against a link's *target* and find
// no entry at all. That is how a package that swapped its binary for a
// launcher symlink would be reported as not holding it, which is the one
// packaging change this refusal most needs to name.
export function storedName(line) {
  const fields = line.trim().split(/\s+/)
  const arrow = fields.indexOf("->")
  const linked = fields.findIndex(
    (field, at) => field === "link" && fields[at + 1] === "to",
  )
  const at = arrow !== -1 ? arrow - 1 : linked !== -1 ? linked - 1 : fields.length - 1
  return fields[at]
}

// The same name, in the spelling the pin uses, so the two can be compared.
// An archive written as `./package` lists every entry that way and a directory
// lists with a trailing slash; neither is a difference in which entry this is.
export function entryName(line) {
  return storedName(line)?.replace(/^\.\//, "").replace(/\/$/, "")
}

// What a listing's mode column says an entry is, for the refusal above.
//
// Only ever read when the entry is *not* a regular file, so the fallback is
// every mode both tars can print that this does not name rather than a case
// believed impossible.
export function kind(line) {
  const mode = line.trim()
  if (mode.startsWith("d")) return "a directory"
  if (mode.startsWith("l")) return "a symbolic link"
  if (mode.startsWith("h")) return "a hard link"
  return "something that is not a file"
}

// Whether a listing line describes a file its owner may run.
//
// The mode column's fourth character, which both GNU and BSD tar print in the
// same place. It is the one fact the archive offers about which of its files
// are programs, and it decides only the difference between a helper and a
// document — never whether a *document* becomes runnable, because the
// installer sets the mode from the pin's role and ignores the archive's
// entirely.
export function ownerMayRun(line) {
  return line.trim()[3] === "x"
}

// What a release installs, read off a listing, or the reason it installs
// nothing.
//
// `install` is the agent's own shape: "launch" pins the one program and
// nothing else, "package" pins every regular file the archive holds. The roles
// come from the listing's mode column rather than from a table, so a release
// that stops shipping its ripgrep executable fails here rather than installing
// one that cannot run.
//
// Sorted by path, which is the order `ReleaseContents` canonicalises to, so
// that the file this writes is the file the server reads back unchanged.
export function releaseFiles(listing, { launch, install }) {
  const lines = listing.split("\n").filter((line) => line.trim() !== "")
  const named = lines.filter((line) => entryName(line) === launch)
  const program = named.find((line) => line.trim().startsWith("-"))
  if (!program) {
    // An archive that does not hold the program and one that holds something
    // else under its name are different things to be told, and the second is
    // the one a maintainer can act on: a release that started shipping a
    // launcher symlink is a packaging change to go and read, not a missing
    // file.
    const [other] = named
    return {
      refusal: other
        ? `holds ${launch} as ${kind(other)} rather than as a regular file`
        : `does not hold ${launch} at all`,
    }
  }
  if (!ownerMayRun(program))
    return { refusal: `holds ${launch}, which is not marked as a program` }
  if (install === "launch") return { files: [{ path: launch, role: "launch" }], program }

  // Every regular file, and nothing else. A release that started shipping a
  // symbolic link inside its package is a packaging change to go and read:
  // the installer refuses a link entry outright, so pinning one would fail for
  // every user rather than here, where somebody can still do something.
  const files = []
  for (const line of lines) {
    const path = entryName(line)
    if (line.trim().startsWith("d")) continue
    if (!line.trim().startsWith("-"))
      return { refusal: `holds ${path} as ${kind(line)} rather than as a regular file` }
    files.push({
      path,
      role: path === launch ? "launch" : ownerMayRun(line) ? "helper" : "document",
    })
  }
  files.sort((left, right) => (left.path < right.path ? -1 : 1))
  return { files, program }
}

export function executableDigest(archive, launch = EXECUTABLE) {
  // Listed with the platform's own tar rather than a dependency: this script
  // runs on a maintainer's machine, not in the app.
  const listing = execFileSync("tar", ["-tvzf", archive], { encoding: "utf8" })
  const entries = listing.split("\n").filter((line) => entryName(line) === launch)
  const named = entries.find((line) => line.trim().startsWith("-"))
  if (!named) {
    const [other] = entries
    return {
      refusal: other
        ? `holds ${launch} as ${kind(other)} rather than as a regular file`
        : `does not hold ${launch} at all`,
    }
  }

  // The size is measured by extracting the entry, not by reading a column out
  // of the listing. GNU tar prints `mode owner/group size date name` and BSD
  // tar prints `mode links owner group size date name`, so the column that
  // holds the size on one holds a link count on the other — and a check that
  // reads the link count would refuse every good archive on macOS.
  //
  // Extracted under the name the listing gave, rather than under the pin's
  // spelling of it, so an entry written as `./package/bin/opencode` is asked
  // for the way it is actually stored.
  const stored = storedName(named)
  const extracted = mkdtempSync(join(tmpdir(), "nessa-entry-"))
  try {
    execFileSync("tar", ["-xzf", archive, "-C", extracted, stored])
    const entry = join(extracted, stored)
    if (statSync(entry).size === 0) return { refusal: `holds ${launch} as an empty file` }
    // Hashed while it is here, because it is the only moment the bytes that
    // will actually be launched exist in this script. The archive digest says
    // nothing about them: two archives can differ in a `package.json` field and
    // hold the same binary, which is the case [`sameBinaryUnderDifferentClaims`]
    // exists to catch.
    const bytes = readFileSync(entry)
    if (!isProgram(bytes))
      return { refusal: `holds ${launch}, whose bytes are not a program` }
    return { digest: createHash("sha256").update(bytes).digest("hex") }
  } finally {
    // The entry is a hundred megabytes, and this runs once per platform.
    rmSync(extracted, { recursive: true, force: true })
  }
}

// Refuse a release whose builds claim different things about the same bytes.
//
// The pin's whole purpose is that a machine is offered a build it can actually
// run, and the two claims that decide that — the C library and whether AVX2 is
// required — are read off the package's *name* in `OPENCODE_BUILDS`. A name is
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
// settle it with the vendor. `pin-agents` says in its own header that the
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

// What the harness lockfiles resolved for a native package.
//
// Read rather than asked of the registry, because this is the whole point of a
// lockfile: it is what `npm ci` installs beside the JavaScript in the bundle,
// so it is what the bundled wrapper expects to find. `resolved` and
// `integrity` come back too, so the coordinate this script then looks up can
// be checked against the one the build actually uses.
export function lockedDependency(document, dependency) {
  const entry = document.packages?.[`node_modules/${dependency}`]
  if (!entry?.version) throw new Error(`${dependency} is not in the lockfile`)
  return { version: entry.version, resolved: entry.resolved, integrity: entry.integrity }
}

// The version to pin for one agent, and what the lockfile said about it.
async function versionOf(agent) {
  if (agent.version.from === "registry") {
    // Encoded, so that an argument with a slash in it asks the registry about a
    // version of this package and not about a different package entirely.
    const requested = encodeURIComponent(process.argv[2] ?? "latest")
    const metadata = await json(`${registry}/${agent.version.package}/${requested}`)
    if (!metadata.version) throw new Error("the registry did not name a version")
    return { version: metadata.version, locked: undefined }
  }
  const lockfile = join(
    root,
    "crates/nessa-sdk/harnesses",
    agent.version.harness,
    "package-lock.json",
  )
  const locked = lockedDependency(
    JSON.parse(readFileSync(lockfile, "utf8")),
    agent.version.dependency,
  )
  return { version: locked.version, locked }
}

// Refuse a pin that would install a different binary than the bundle expects.
//
// The lockfile names the tarball `npm ci` fetches and the checksum it verifies.
// If the coordinate this script looked up resolves anywhere else, the pin and
// the bundled JavaScript are describing two different builds — which is exactly
// the failure unbundling is supposed not to introduce.
export function agreesWithLockfile(locked, dist, named) {
  if (!locked) return
  if (locked.resolved && dist?.tarball && locked.resolved !== dist.tarball)
    throw new Error(
      `${named} resolves to ${dist.tarball}, but the harness lockfile installs ` +
        `${locked.resolved}; the pin and the bundle would be different builds`,
    )
  if (locked.integrity && dist?.integrity && locked.integrity !== dist.integrity)
    throw new Error(`${named} does not match the integrity the harness lockfile recorded`)
}

// Measure every agent's archives and write the pin file.
async function pin() {
  const scratchDirectory = join(root, "target/agent-pins")
  mkdirSync(scratchDirectory, { recursive: true })

  const agents = {}
  for (const agent of AGENTS) {
    const { version, locked } = await versionOf(agent)
    const releases = []
    // What each package's launched program actually is, so the claims each pin
    // makes can be checked against the bytes rather than against the package's
    // name.
    const measured = []
    for (const build of agent.builds) {
      const named = `${build.package}@${version}`
      const detail = await json(
        `${registry}/${build.package}/${encodeURIComponent(version)}`,
      )
      const archive = detail.dist?.tarball
      if (!archive) throw new Error(`${named} has no tarball`)
      // Parsed rather than matched on a prefix, which `https://` alone would
      // pass while naming nothing at all. The same rule is applied again when
      // the pin is compiled in; this is the earlier of the two places.
      if (new URL(archive).protocol !== "https:")
        throw new Error(`${named} is not served over https`)
      agreesWithLockfile(locked, detail.dist, named)
      const scratch = join(scratchDirectory, `${build.package.replace(/\//g, "-")}.tgz`)
      process.stderr.write(`measuring ${named}\n`)
      const digest = await digestOf(archive, scratch)
      // The registry publishes its own checksum for these bytes, in the
      // metadata already in hand. Comparing them is the only thing in this
      // chain that can catch a download that arrived wrong: everything
      // downstream checks that the *same* bytes arrive again, so a bad
      // measurement made here would be pinned permanently and would verify
      // perfectly forever.
      await agreesWithRegistry(scratch, detail.dist, named)
      // Measured from the file rather than read out of the metadata, for the
      // same reason as the digest: the installer holds the fetch to this
      // number, so it has to be the length of the bytes that were hashed.
      const archiveBytes = statSync(scratch).size
      const listing = execFileSync("tar", ["-tvzf", scratch], { encoding: "utf8" })
      const contents = releaseFiles(listing, agent)
      if (contents.refusal) throw new Error(`${named} ${contents.refusal}`)
      const program = executableDigest(scratch, agent.launch)
      if (program.refusal) throw new Error(`${named} ${program.refusal}`)
      rmSync(scratch, { force: true })
      measured.push({
        package: build.package,
        libc: build.libc,
        requiresAvx2: build.requiresAvx2,
        digest: program.digest,
      })
      releases.push({
        operatingSystem: build.operatingSystem,
        architecture: build.architecture,
        // Written out even when there is nothing to require, because this file
        // is read by people reviewing a pin: an entry that simply omits them
        // leaves "this build runs anywhere on its platform" and "whoever
        // generated this forgot" looking identical.
        libc: build.libc,
        requiresAvx2: build.requiresAvx2,
        version,
        archiveUrl: archive,
        archiveBytes,
        archiveDigest: digest,
        files: contents.files,
      })
    }
    sameBinaryUnderDifferentClaims(measured)
    agents[agent.name] = releases
    process.stderr.write(
      `pinned ${agent.name} ${version} across ${releases.length} builds\n`,
    )
  }

  const destination = join(root, "crates/nessa-server/data/agent-releases.json")
  mkdirSync(resolve(destination, ".."), { recursive: true })
  const document = {
    comment:
      "Generated by scripts/agents/pin-agents.mjs. Digests and sizes are measured from the published archives; do not edit by hand.",
    // Sorted, so that adding an agent does not reorder the file.
    agents: Object.fromEntries(
      Object.keys(agents)
        .sort()
        .map((name) => [name, agents[name]]),
    ),
  }
  writeFileSync(destination, `${JSON.stringify(document, null, 2)}\n`)
}

const invoked = process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href
if (invoked) await pin()
