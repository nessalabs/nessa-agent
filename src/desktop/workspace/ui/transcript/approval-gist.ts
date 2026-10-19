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
  /** The programs its words name, in the order they first appear. */
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
   * is set one step per line. When false, `script` is exactly as sent and
   * `programs` is empty: the sketch cannot tell which words are programs.
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

/** An operator between steps, where `scriptLines` may break the line. */
interface Break {
  /** Where the operator starts and ends in the script. */
  readonly start: number
  readonly end: number
  readonly operator: ";" | "&&" | "||" | "|" | "|&"
}

interface Reading {
  readonly words: readonly Word[]
  /** The operators it read between steps, in order. */
  readonly breaks: readonly Break[]
  /** Nothing in the script was outside what the sketch reads. */
  readonly lined: boolean
}

/** The script's words, unquoted, each marked where a new program starts. */
export function read(script: string): Reading {
  const words: Word[] = []
  const breaks: Break[] = []
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
    // A substitution runs a program inside a word, quoted (`"$(…)"`) or not;
    // inside `${…}` and `$[…]` the quotes and operators are not the ones read here.
    if (
      quote !== "'" &&
      (char === "`" ||
        (char === "$" &&
          (next === "(" || next === "'" || next === '"' || next === "{" || next === "[")))
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
    // In `[[ … ]]`, `|`, `<` and `>` are not operators.
    if (char === "[" && next === "[" && !inWord) lined = false
    // `2>&1`, `&>` and `>|` redirect; they do not start a program. Only an
    // unquoted, unescaped `<` or `>` just before makes the `&` or `|` a redirect's.
    const redirect =
      (char === "&" && (wasRedirect || next === ">")) ||
      (char === "|" && wasRedirect && script[index - 1] === ">")
    afterRedirect = char === "<" || char === ">"
    if (";&|\n()".includes(char) && !redirect) {
      end()
      // An operator with no command before it, or `;;` outside a `case`, is a
      // shell's syntax error: it runs only what came before, and the lines
      // would join what it never runs.
      if (char !== "\n" && starts) lined = false
      starts = true
      const pair = char + (next ?? "")
      if (pair === ";;") lined = false
      if (pair === "&&" || pair === "||" || pair === "|&" || pair === ";;") {
        if (pair !== ";;") breaks.push({ start: index, end: index + 2, operator: pair })
        index++
      } else if (char === ";" || char === "|")
        breaks.push({ start: index, end: index + 1, operator: char })
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
  return { words, breaks, lined }
}

/** Reserved words after which the next word is a program. */
const leadsProgram = new Set([
  "if",
  "then",
  "else",
  "elif",
  "while",
  "until",
  "do",
  "!",
  "time",
  "{",
])
/** Reserved words that close what they opened; no program follows them. */
const endsCompound = new Set(["fi", "done", "}", "esac"])
/** Reserved words whose next words are names and values, not a program. */
const opensList = new Set(["for", "select", "case", "function"])

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

/** Spaces, tabs and newlines that end `text`, where no backslash escapes them. */
function trimEnd(text: string): string {
  while (/[ \t\n]$/.test(text)) {
    const slashes = /\\*$/.exec(text.slice(0, -1))?.[0].length ?? 0
    if (slashes % 2 === 1) break
    text = text.slice(0, -1)
  }
  return text
}

/**
 * The script with one step per line: a line break after each `;`, `&&`,
 * `||` and `|`, the step a pipe feeds indented under it — at the operators
 * `read` found, and only there. Only the unquoted spaces
 * beside those operators change, which a shell does not read, so the lines
 * mean what the script means. A script `read` cannot follow is returned as
 * it came.
 */
export function scriptLines(script: string): string {
  const reading = read(script)
  if (!reading.lined) return script
  let out = ""
  let from = 0
  for (const { start, end, operator } of reading.breaks) {
    const step = trimEnd(script.slice(from, start))
    // A pipe ends its line too, as a shell reads it, and the step it feeds is indented.
    out +=
      operator === "|" || operator === "|&"
        ? `${step} ${operator}\n  `
        : `${step}${operator === ";" ? "" : " "}${operator}\n`
    // The spaces and newlines after an operator are not read; each step starts its own line.
    from = end
    while (/[ \t\n]/.test(script[from] ?? "")) from++
  }
  out += trimEnd(script.slice(from))
  // Only unescaped ASCII spaces at the ends: any other is part of a word.
  return trimEnd(out.replace(/^[ \t\n]+/, ""))
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
      // A reserved word is the shell's own, not a program: after `then` or
      // `do` the next word is the program, and after `for` or `case` none is.
      if (leadsProgram.has(word.text)) continue
      awaiting = false
      if (endsCompound.has(word.text) || opensList.has(word.text)) continue
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
    // Where the sketch lost the script's thread, its programs may be stray words.
    programs: reading.lined
      ? [...programs].map(([name, count]) => ({ name, count }))
      : [],
    hosts,
    lined: reading.lined,
  }
}
