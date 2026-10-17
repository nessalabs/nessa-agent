/**
 * What an agent's command touches, read from its words so the card can say
 * it at a glance (`approval-card.tsx`): the programs it runs and the sites it
 * names. The command itself stays the authority; the gist only points at it,
 * and the card's Details show it whole.
 *
 * A command arrives as the agent's shell words (`cargo test -p gateway`) or
 * as a tool and its JSON input (`mcp__nessa__shell {"command":"curl …"}`).
 * A tool whose input carries a shell script — a `command` string, or a
 * `["bash", "-lc", "…"]` list — is read as that script; any other tool is
 * only its name.
 *
 * The reading is a sketch, not a shell. It knows plain words, quotes,
 * escapes, and the operators that start a new program (`;`, `&&`, `||`,
 * `|`, `&`, a newline). Anything else can run a program it would not see —
 * a substitution (`$(…)`, backticks, `<(…)`), a block (`if`, `for`, `{ }`),
 * a heredoc, a comment, a program that runs another (`sudo`, `xargs`,
 * `bash -c`, `python`). Then the gist is not `plain`, and the card opens the whole command at
 * once rather than let the summary stand for it.
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
   * gist is `plain` (`scriptLines`), and exactly as sent where it is not.
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
   * Every program the script runs is among `programs`: it has only what the
   * sketch reads. When false, the gist may leave a program out, and the
   * whole command is what the person reads first.
   */
  readonly plain: boolean
}

const shells = new Set(["bash", "sh", "zsh", "dash", "fish", "ksh"])

/** Words a shell reads as its own grammar, not as a program to run. */
const keywords = new Set([
  "if",
  "then",
  "elif",
  "else",
  "fi",
  "for",
  "while",
  "until",
  "do",
  "done",
  "case",
  "esac",
  "select",
  "function",
  "in",
  "{",
  "}",
  "!",
  "[[",
  "]]",
  "time",
  "coproc",
])

/** Programs that run another program, or code, they are given. */
const runners = new Set([
  "sudo",
  "doas",
  "su",
  "env",
  "xargs",
  "parallel",
  "eval",
  "exec",
  "source",
  ".",
  "command",
  "builtin",
  "nohup",
  "nice",
  "ionice",
  "timeout",
  "watch",
  "strace",
  "chroot",
  "unshare",
  "flock",
  "script",
  "ssh",
  "docker",
  "podman",
  "kubectl",
  "awk",
  "gawk",
  "find",
  "make",
  "npx",
  "pnpx",
  "bunx",
  "python",
  "python3",
  "node",
  "deno",
  "bun",
  "perl",
  "ruby",
  "php",
  "lua",
  "osascript",
  "pwsh",
  ...shells,
])

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
  readonly plain: boolean
}

/** The script's words, unquoted, each marked where a new program starts. */
function read(script: string): Reading {
  const words: Word[] = []
  let plain = true
  let text = ""
  let inWord = false
  let starts = true
  let quote: "'" | '"' | undefined
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
    // A substitution runs a program inside a word, quoted (`"$(…)"`) or not.
    if (
      quote !== "'" &&
      (char === "`" || (char === "$" && (next === "(" || next === "'" || next === '"')))
    )
      plain = false
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
    if (char === "#" && !inWord) plain = false
    if (char === "<" && (next === "<" || next === "(")) plain = false
    if (char === ">" && next === "(") plain = false
    if (char === "(" || char === ")") plain = false
    // `2>&1` and `&>` redirect; they do not start a program.
    const redirect = char === "&" && (/[<>]$/.test(text) || next === ">")
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
  if (quote) plain = false
  end()
  return { words, plain }
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
 * a plain script only, whose quotes this reading follows.
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
  const reading = script === undefined ? { words: [], plain: true } : read(script)
  let plain = reading.plain
  const programs = new Map<string, number>()
  const hosts: string[] = []
  let awaiting = true
  for (const word of reading.words) {
    if (word.starts) awaiting = true
    if (awaiting && !assignment.test(word.text)) {
      awaiting = false
      if (keywords.has(word.text) || runners.has(word.text.split("/").at(-1) ?? ""))
        plain = false
      programs.set(word.text, (programs.get(word.text) ?? 0) + 1)
    }
    for (const host of hostsIn(word.text)) if (!hosts.includes(host)) hosts.push(host)
  }
  return {
    ...(call ? { tool: call.tool } : {}),
    ...(script !== undefined ? { script: plain ? scriptLines(script) : script } : {}),
    fields,
    programs: [...programs].map(([name, count]) => ({ name, count })),
    hosts,
    plain,
  }
}
