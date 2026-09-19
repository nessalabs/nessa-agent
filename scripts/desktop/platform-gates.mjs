/**
 * Finds code the desktop host compiles on every platform but only macOS can
 * reach.
 *
 * `-D warnings` makes dead code an error, and the host is full of macOS-only
 * adapters. A helper written beside one of them, without the gate the adapter
 * itself carries, compiles fine here and fails the Windows and Linux jobs —
 * twice on one branch so far, `KEYCHAIN_SERVICE` and then `stage_port`. The
 * compiler is the authority on this and cannot be consulted: checking another
 * target needs a cross-toolchain, because the updater's dependencies build C.
 *
 * So this reads the source instead, and is honest about being a heuristic. It
 * answers one question — is every reference to this thing inside code only
 * macOS builds — and reports what it finds rather than proving anything. It
 * looks at whole modules and at items declared at the top level of a file; it
 * does not look inside `impl` blocks, and it cannot see a name reached through
 * a macro or a trait object. A finding is a thing to go and read.
 *
 * References from `#[cfg(test)]` scopes do not count as reaching an item,
 * which is what the compiler does: `cargo clippy --all-targets` still computes
 * dead code for the build without tests, and that is the build that ships.
 */

/** A `#[cfg(...)]` that only holds on macOS. */
const MACOS_GATE = /#\[cfg\((?:[^)]*\W)?target_os\s*=\s*"macos"/
const TEST_GATE = /#\[cfg\((?:[^)]*\W)?test\W/

/** `mod name;` — a module in another file, not an inline `mod name { }`. */
const MODULE_DECLARATION = /^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+([a-z_][a-z0-9_]*)\s*;/

/** What a file or item is compiled for. */
export const EVERY_PLATFORM = "every-platform"
export const MACOS_ONLY = "macos-only"
export const TEST_ONLY = "test-only"

/**
 * The attributes immediately above `index`, as one string.
 *
 * Attributes stack, and a doc comment between them does not break the run, so
 * the gate may not be on the line above the item.
 */
function attributesAbove(lines, index) {
  const collected = []
  for (let above = index - 1; above >= 0; above -= 1) {
    const line = lines[above].trim()
    if (line.startsWith("#[") || line.startsWith("#![")) collected.push(line)
    else if (line.startsWith("//") || line === "") continue
    else break
  }
  return collected.join("\n")
}

/**
 * Every `mod name;` a file declares, with what it is compiled for.
 *
 * @returns {{ name: string, gate: string }[]}
 */
export function declaredModules(source) {
  const lines = source.split("\n")
  const found = []
  lines.forEach((line, index) => {
    const match = MODULE_DECLARATION.exec(line)
    if (!match) return
    const attributes = attributesAbove(lines, index)
    found.push({
      name: match[1],
      gate: gateFrom(attributes),
    })
  })
  return found
}

/**
 * What a run of attributes says a thing is compiled for.
 *
 * One place, because the next gate worth reading — a Linux-only adapter is the
 * obvious one — has to be added here rather than found in two chains that must
 * agree.
 */
function gateFrom(attributes) {
  if (MACOS_GATE.test(attributes)) return MACOS_ONLY
  if (TEST_GATE.test(attributes)) return TEST_ONLY
  return EVERY_PLATFORM
}

/** Where a module's file sits, given the file that declares it. */
export function modulePaths(declaringPath, name) {
  const directory =
    declaringPath.endsWith("/main.rs") || declaringPath.endsWith("/mod.rs")
      ? declaringPath.replace(/\/[^/]+$/, "")
      : declaringPath.replace(/\.rs$/, "")
  return [`${directory}/${name}.rs`, `${directory}/${name}/mod.rs`]
}

/**
 * What each file is compiled for, walking out from the crate root.
 *
 * A module inherits its parent's gate: everything under a macOS-only module is
 * macOS-only too, however its own declaration reads.
 *
 * @param {Map<string, string>} sources path → contents
 * @param {string} root the crate root's path
 */
export function fileGates(sources, root) {
  const gates = new Map([[root, EVERY_PLATFORM]])
  const queue = [root]
  while (queue.length > 0) {
    const path = queue.shift()
    const parent = gates.get(path)
    for (const { name, gate } of declaredModules(sources.get(path) ?? "")) {
      const child = modulePaths(path, name).find((candidate) => sources.has(candidate))
      if (!child || gates.has(child)) continue
      gates.set(child, parent === EVERY_PLATFORM ? gate : parent)
      queue.push(child)
    }
  }
  return gates
}

/** An item declared at the top level of a file. */
const ITEM_DECLARATION =
  /^\s*(?:pub(?:\([^)]*\))?\s+)?(?:unsafe\s+|async\s+|extern\s+"[^"]*"\s+)*(fn|const|static|struct|enum|union|trait|type|mod)\s+([A-Za-z_][A-Za-z0-9_]*)/

/**
 * The top-level items a file declares, each with the span it occupies and what
 * it is compiled for.
 *
 * Depth is counted in braces, so only declarations at the outermost level are
 * items here — a method inside an `impl` is not one, because a name used only
 * through a trait is beyond what reading the source can settle.
 */
export function topLevelItems(source, fileGate = EVERY_PLATFORM) {
  const lines = source.split("\n")
  const items = []
  let depth = 0
  let open = null
  lines.forEach((line, index) => {
    if (depth === 0 && open === null) {
      const match = ITEM_DECLARATION.exec(line)
      if (match) {
        const attributes = attributesAbove(lines, index)
        const own = gateFrom(attributes)
        open = {
          kind: match[1],
          name: match[2],
          from: index,
          to: index,
          gate: fileGate === EVERY_PLATFORM ? own : fileGate,
        }
      }
    }
    const before = depth
    for (const character of line) {
      if (character === "{") depth += 1
      else if (character === "}") depth -= 1
    }
    if (open && depth === 0 && (before > 0 || line.includes(";") || line.includes("}"))) {
      open.to = index
      items.push(open)
      open = null
    }
  })
  if (open) {
    open.to = lines.length - 1
    items.push(open)
  }
  return items
}

/**
 * Whether a line of a file is inside code every platform builds.
 *
 * Anything not covered by a top-level item — a `use`, an inline `impl` — is
 * treated as the file's own gate.
 */
function gateOfLine(items, fileGate, index) {
  const holder = items.find((item) => index >= item.from && index <= item.to)
  return holder ? holder.gate : fileGate
}

/**
 * Items compiled everywhere that only macOS-only code refers to.
 *
 * @param {Map<string, string>} sources path → contents
 * @param {string} root the crate root's path
 * @param {string[]} spared names that are referenced by something other than
 *   this crate's own source — `main`, and anything an attribute wires up.
 */
/**
 * Names nothing in this crate calls, and which are reached anyway.
 *
 * `main` is the entry point. Anything else added here is a claim that a macro
 * or an attribute reaches it, which is the one thing reading source cannot see
 * — so it is a constant with a reason beside it rather than a parameter no
 * caller passes and a failure message that tells the reader to edit a default.
 */
const REACHED_FROM_OUTSIDE = ["main"]

export function unreachableOffMacos(sources, root, spared = REACHED_FROM_OUTSIDE) {
  const gates = fileGates(sources, root)
  const parsed = new Map()
  for (const [path, source] of sources) {
    const gate = gates.get(path) ?? EVERY_PLATFORM
    parsed.set(path, {
      gate,
      items: topLevelItems(source, gate),
      lines: source.split("\n"),
    })
  }

  /** Does any line every platform builds mention `name`, outside `skip`? */
  const mentionedOffMacos = (name, skip) => {
    const word = new RegExp(`\\b${name}\\b`)
    for (const [path, file] of parsed) {
      for (let index = 0; index < file.lines.length; index += 1) {
        if (skip(path, index)) continue
        if (gateOfLine(file.items, file.gate, index) !== EVERY_PLATFORM) continue
        // Comments mention names for the reader, not the compiler.
        if (word.test(file.lines[index].replace(/\/\/.*$/, ""))) return true
      }
    }
    return false
  }

  const findings = []
  for (const [path, file] of parsed) {
    if (file.gate !== EVERY_PLATFORM) continue
    for (const item of file.items) {
      if (item.gate !== EVERY_PLATFORM || spared.includes(item.name)) continue
      // A module's own file is where its name is spelled out, not where
      // anything reaches for it; for every other item its own span is the
      // declaration, which cannot be what keeps it alive.
      const ownFile =
        item.kind === "mod"
          ? modulePaths(path, item.name).find((candidate) => sources.has(candidate))
          : undefined
      const declaration = (at, index) =>
        at === ownFile || (at === path && index >= item.from && index <= item.to)
      if (mentionedOffMacos(item.name, declaration)) continue
      findings.push({ path, name: item.name, kind: item.kind, line: item.from + 1 })
    }
  }
  return findings
}
