/**
 * What an agent's command touches, read from its words so the card can say
 * it at a glance (`approval-card.tsx`): the programs it names and the sites it
 * names. The command itself stays the authority, and the card always shows
 * it: the gist only points at parts of it. A program can run another that
 * no word names (`rg --pre`, a hook, a file the same command wrote to a
 * folder on `PATH`), so the gist never claims to list everything it runs.
 *
 * A command arrives as the agent's shell words (`cargo test -p gateway`) or
 * as a tool and its JSON input (`mcp__nessa__shell {"command":"curl …"}`).
 * A tool whose input carries a shell script — a `command` string, or a
 * `["bash", "-lc", "…"]` list — is read as that script; any other tool is
 * only its name.
 *
 * The reading is a sketch, not a shell. It follows plain words, quotes,
 * escapes and the operators between programs (`;`, `&&`, `||`, `|`, `&`, a
 * newline); where it meets anything else — a substitution, a subshell, a
 * heredoc, a comment, an ANSI-C quote, a quote left open — it is not
 * `lined`, and the script is shown exactly as sent.
 */

/** A program the command runs, and how many times. */
export interface GistProgram {
  readonly name: string
  readonly count: number
}

export interface CommandGist {
  /** The tool the command calls, when it is a tool and its input. */
  readonly tool?: string
  /**
   * The shell script it runs, when there is one: one step per line where the
   * script is `lined` (`scriptLines`), and exactly as sent where it is not.
   */
  readonly script?: string
  /** The input's other fields, shown beside the script: `[name, JSON value]`. */
  readonly fields: readonly (readonly [string, string])[]
  /** The programs, in the order they first run. */
  readonly programs: readonly GistProgram[]
  /**
   * The hosts of the web addresses the script names (`https://…`), in their
   * ASCII form, in the order they first appear. A host written without a
   * scheme (`curl example.com`) is not among them; the program that reaches
   * it is among `programs`.
   */
  readonly hosts: readonly string[]
  /**
   * The sketch followed every quote and operator in the script, so `script`
   * is set one step per line. When false, `script` is exactly as sent, and
   * `programs` may leave out a program the sketch could not see.
   */
  readonly lined: boolean
}

const shells = new Set(["bash", "sh", "zsh", "dash", "fish", "ksh"])

/** A shell list such as `["bash", "-lc", "…"]` is its script; any other list has none. */
function scriptOf(value: unknown): string | undefined {
  if (typeof value === "string") return value
  if (!Array.isArray(value) || value.length !== 3) return undefined
  const [shell, flag, script] = value as unknown[]
  if (typeof shell !== "string" || typeof flag !== "string" || typeof script !== "string")
    return undefined
  if (!shells.has(shell.split("/").at(-1) ?? "") || !/^-[a-z]*c[a-z]*$/.test(flag))
    return undefined
  return script
}

/** The tool and its JSON input, when the command is that; otherwise nothing. */
function toolCall(
  command: string,
): { tool: string; input: Record<string, unknown> } | undefined {
  const space = command.indexOf(" ")
  if (space <= 0) return undefined
  const rest = command.slice(space + 1).trim()
  if (!rest.startsWith("{")) return undefined
  try {
    const input = JSON.parse(rest) as unknown
    if (input === null || typeof input !== "object" || Array.isArray(input))
      return undefined
    return { tool: command.slice(0, space), input: input as Record<string, unknown> }
  } catch {
    return undefined
  }
}

interface Word {
  readonly text: string
  /** It began a new program: the first word after an operator. */
  readonly starts: boolean
}

interface Reading {
  readonly words: readonly Word[]
  /** Nothing in the script was outside what the sketch reads. */
  readonly lined: boolean
}

/** The script's words, unquoted, each marked where a new program starts. */
function read(script: string): Reading {
  const words: Word[] = []
  let lined = true
  let text = ""
  let inWord = false
  let starts = true
  let quote: "'" | '"' | undefined
  let afterRedirect = false
  const end = () => {
    if (inWord) {
      words.push({ text, starts })
      starts = false
    }
    text = ""
    inWord = false
  }
  for (let index = 0; index < script.length; index++) {
    const char = script[index] ?? ""
    const next = script[index + 1]
    // Whether the character just before was an unquoted, unescaped `<` or `>`.
    const wasRedirect = afterRedirect
    afterRedirect = false
    // A substitution runs a program inside a word, quoted (`"$(…)"`) or not.
    if (
      quote !== "'" &&
      (char === "`" || (char === "$" && (next === "(" || next === "'" || next === '"')))
    )
      lined = false
    if (quote) {
      if (char === quote) quote = undefined
      else if (char === "\\" && quote === '"' && next !== undefined)
        text += script[++index]
      else text += char
      continue
    }
    if (char === "'" || char === '"') {
      quote = char
      inWord = true
      continue
    }
    if (char === "\\" && next !== undefined) {
      if (next !== "\n") text += next
      index++
      inWord = inWord || next !== "\n"
      continue
    }
    if (char === "#" && !inWord) lined = false
    if (char === "<" && (next === "<" || next === "(")) lined = false
    if (char === ">" && next === "(") lined = false
    if (char === "(" || char === ")") lined = false
    // `2>&1` and `&>` redirect; they do not start a program. Only an unquoted,
    // unescaped `<` or `>` just before makes the `&` a redirect's.
    const redirect = char === "&" && (wasRedirect || next === ">")
    afterRedirect = char === "<" || char === ">"
    if (";&|\n()".includes(char) && !redirect) {
      end()
      starts = true
      continue
    }
    if (char === " " || char === "\t") {
      end()
      continue
    }
    text += char
    inWord = true
  }
  if (quote) lined = false
  end()
  return { words, lined }
}

/** `NAME=value` before a program sets its environment; it is not the program. */
const assignment = /^[A-Za-z_][A-Za-z0-9_]*=/

/** Web addresses anywhere in a word: `--url=https://…` names its host too. */
const addresses = /\b(?:https?|wss?|ftp):\/\/[^\s'"<>`]+/gi

/** The hosts a word names, each as the browser spells it (`xn--…` for a look-alike). */
function hostsIn(word: string): string[] {
  const hosts: string[] = []
  for (const [address] of word.matchAll(addresses)) {
    try {
      const host = new URL(address).hostname
      if (host.length > 0) hosts.push(host)
    } catch {
      // Not an address a browser could open: no host to name.
    }
  }
  return hosts
}

/**
 * The script with one step per line: a line break after each `;`, `&&` and
 * `||` outside quotes, and before each `|`, which is indented under the
 * step it feeds. Only the unquoted spaces beside those operators change,
 * which a shell does not read, so the lines mean what the script means. For
 * a `lined` script only, whose quotes this reading follows.
 */
export function scriptLines(script: string): string {
  let out = ""
  let quote: "'" | '"' | undefined
  // An escaped space (`a\\ `) is part of a word, and stays.
  const trimmed = (text: string) => {
    while (/[ \t]$/.test(text) && !text.endsWith("\\ ") && !text.endsWith("\\\t"))
      text = text.slice(0, -1)
    return text
  }
  const skipSpaces = (index: number) => {
    while (script[index + 1] === " " || script[index + 1] === "\t") index++
    return index
  }
  for (let index = 0; index < script.length; index++) {
    const char = script[index] ?? ""
    const next = script[index + 1]
    if (quote) {
      out += char
      if (char === "\\" && quote === '"' && next !== undefined) out += script[++index]
      else if (char === quote) quote = undefined
      continue
    }
    if (char === "'" || char === '"') quote = char
    if (char === "\\" && next !== undefined) {
      out += char + script[++index]
      continue
    }
    if ((char === "&" && next === "&") || (char === "|" && next === "|")) {
      out = `${trimmed(out)} ${char}${next}\n`
      index = skipSpaces(index + 1)
      continue
    }
    if (char === ";" && next !== ";") {
      out = `${trimmed(out)};\n`
      index = skipSpaces(index)
      continue
    }
    if (char === "|" && next !== "&") {
      out = `${trimmed(out)}\n  | `
      index = skipSpaces(index)
      continue
    }
    out += char
  }
  return out.trim()
}

/** What the command runs and names (`CommandGist`). */
export function commandGist(command: string): CommandGist {
  const call = toolCall(command)
  const script = call ? scriptOf(call.input.command) : command
  const fields = call
    ? Object.entries(call.input)
        .filter(([name]) => script === undefined || name !== "command")
        .map(([name, value]) => [name, JSON.stringify(value)] as const)
    : []
  const reading = script === undefined ? { words: [], lined: false } : read(script)
  const programs = new Map<string, number>()
  const hosts: string[] = []
  let awaiting = true
  for (const word of reading.words) {
    if (word.starts) awaiting = true
    // `NAME=value` before a program sets its environment; the next word is the program.
    if (awaiting && !assignment.test(word.text)) {
      awaiting = false
      programs.set(word.text, (programs.get(word.text) ?? 0) + 1)
    }
    for (const host of hostsIn(word.text)) if (!hosts.includes(host)) hosts.push(host)
  }
  return {
    ...(call ? { tool: call.tool } : {}),
    ...(script !== undefined
      ? { script: reading.lined ? scriptLines(script) : script }
      : {}),
    fields,
    programs: [...programs].map(([name, count]) => ({ name, count })),
    hosts,
    lined: reading.lined,
  }
}
