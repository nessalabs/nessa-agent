/** A present local path cannot safely anchor private data. */
export class NessaPrivateFileUnsafeError extends Error {
  constructor() {
    super("Local private-file path is unsafe")
    this.name = "NessaPrivateFileUnsafeError"
  }
}

type NodeFileTools = {
  fs: typeof import("node:fs/promises")
  path: typeof import("node:path")
  constants: typeof import("node:fs").constants
}

function unsafe(condition: boolean): void {
  if (condition) throw new NessaPrivateFileUnsafeError()
}

async function validateAcquisition(
  tools: NodeFileTools,
  root: string,
  uid: number,
): Promise<string> {
  const { fs, path } = tools
  unsafe(!path.isAbsolute(root))
  const original: string[] = []
  for (let current = root; ; current = path.dirname(current)) {
    original.push(current)
    if (current === path.dirname(current)) break
  }
  original.reverse()
  for (const [index, current] of original.entries()) {
    const stat = await fs.lstat(current)
    const isRoot = index === original.length - 1
    unsafe(
      isRoot
        ? stat.isSymbolicLink() ||
            !stat.isDirectory() ||
            stat.uid !== uid ||
            (stat.mode & 0o077) !== 0
        : !stat.isDirectory() && !stat.isSymbolicLink(),
    )
    if (stat.isDirectory() && !isRoot) {
      unsafe(stat.uid !== 0 && stat.uid !== uid)
      const writableByAnotherUser = (stat.mode & 0o022) !== 0
      const sticky = (stat.mode & 0o1000) !== 0
      unsafe(writableByAnotherUser && !sticky)
    }
    if (stat.isSymbolicLink()) unsafe(stat.uid !== 0 && stat.uid !== uid)
  }
  return fs.realpath(root)
}

/**
 * Read one Unix private file through a namespace protected from other OS users.
 *
 * The acquisition path may contain a system or current-user symlink only when
 * its parent prevents another user from replacing it. The selected root and
 * every namespace directory below it must be real, current-user, mode-0700
 * directories. The leaf is opened nonblocking with `O_NOFOLLOW`, then its
 * ownership, mode, type, link count, and size are checked on that same handle.
 *
 * This boundary does not resist a privileged process or a concurrent process
 * running under the same uid. Such a process can already read a mode-0600
 * credential directly. Hosts requiring stronger namespace confinement inject
 * their own source; the native host uses the Rust `openat` adapter.
 */
export async function readUnixPrivateFile(
  tools: NodeFileTools,
  root: string,
  relative: string,
  uid: number,
  maximumBytes: number,
): Promise<string> {
  const { fs, path, constants } = tools
  unsafe(
    path.isAbsolute(relative) || relative.split(path.sep).some((part) => part === ".."),
  )
  const canonicalRoot = await validateAcquisition(tools, root, uid)
  const file = path.join(canonicalRoot, relative)
  let directory = canonicalRoot
  for (const component of path.dirname(relative).split(path.sep).filter(Boolean)) {
    directory = path.join(directory, component)
    const stat = await fs.lstat(directory)
    unsafe(
      stat.isSymbolicLink() ||
        !stat.isDirectory() ||
        stat.uid !== uid ||
        (stat.mode & 0o077) !== 0,
    )
  }
  const handle = await fs.open(
    file,
    constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK,
  )
  try {
    const stat = await handle.stat()
    unsafe(
      !stat.isFile() ||
        stat.nlink !== 1 ||
        stat.uid !== uid ||
        (stat.mode & 0o077) !== 0 ||
        stat.size < 1 ||
        stat.size > maximumBytes,
    )
    return await handle.readFile("utf8")
  } finally {
    await handle.close()
  }
}
