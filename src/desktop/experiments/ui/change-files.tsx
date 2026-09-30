import { useMemo, useState } from "react"
import { VirtualList } from "@nessa-ui/react/virtual-list"
import {
  changeTotals,
  type Change,
  type FileChange,
  type FileStatus,
} from "../model/experiment"
import { useOpenInEditor } from "../adapters/react/experiments-provider"
import { count, exact, plural } from "../../ui/format"
import { tooltip } from "../../ui/tooltip"

/** The files a change shows first, busiest first. */
const filesShown = 6
/** Past this many files, the change is also told by the folders it landed in. */
const foldersFrom = 40
const foldersShown = 5
const fileRow = 32

const statusLetters: Record<FileStatus, string> = {
  added: "A",
  modified: "M",
  deleted: "D",
  renamed: "R",
}

const statusLabels: Record<FileStatus, string> = {
  added: "Added",
  modified: "Modified",
  deleted: "Deleted",
  renamed: "Renamed",
}

/**
 * What a run changed, at any size, without its lines: what the agent says it
 * did and why, how much it touched, its busiest files, where a large change
 * landed, and every file a search away. A diff opens in the person's editor
 * — one file can be a thousand lines, so none is drawn here.
 */
export function ChangeView({
  experimentId,
  runId,
  change,
  rationale,
}: {
  experimentId: string
  runId: string
  change: Change
  rationale: string
}) {
  const open = useOpenInEditor(experimentId)
  const [all, setAll] = useState(false)
  const totals = changeTotals(change)
  const files = change.files
  const busiest = useMemo(
    () => [...files].sort((a, b) => b.added + b.removed - (a.added + a.removed)),
    [files],
  )
  const openFile = (path: string) => open({ runId, path })
  return (
    <div className="xp-change-view">
      {change.summary ? <p className="xp-change-summary">{change.summary}</p> : null}
      <p className="xp-change-why">
        <span className="xp-faint">Why · </span>
        {rationale}
      </p>
      {files.length > 0 ? (
        <div className="xp-change-totals">
          <span {...tooltip(`${exact(files.length)} files`)}>
            {plural(files.length, "file")}
          </span>
          <span className="xp-add" {...tooltip(`${exact(totals.added)} lines added`)}>
            +{count(totals.added)}
          </span>
          <span
            className="xp-remove"
            {...tooltip(`${exact(totals.removed)} lines removed`)}
          >
            −{count(totals.removed)}
          </span>
          <Churn added={totals.added} removed={totals.removed} />
        </div>
      ) : null}
      {files.length === 0 ? null : all ? (
        <AllFiles files={files} onOpen={openFile} onDone={() => setAll(false)} />
      ) : (
        <>
          <ul className="xp-files">
            {busiest.slice(0, filesShown).map((file) => (
              <li key={file.path}>
                <FileRow file={file} onOpen={openFile} />
              </li>
            ))}
          </ul>
          {files.length >= foldersFrom ? <Folders files={files} /> : null}
          {files.length > filesShown ? (
            <button type="button" className="xp-link" onClick={() => setAll(true)}>
              All {plural(files.length, "file")}
            </button>
          ) : null}
        </>
      )}
    </div>
  )
}

function OpenGlyph() {
  return (
    <svg
      width="11"
      height="11"
      viewBox="0 0 12 12"
      aria-hidden
      fill="none"
      stroke="currentColor"
      strokeWidth="1.4"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <path d="M4.5 2.5h5v5M9.5 2.5l-7 7" />
    </svg>
  )
}

/** Added and removed as five cells, the way a diff stat reads at a glance. */
function Churn({ added, removed }: { added: number; removed: number }) {
  const total = added + removed
  const green = total === 0 ? 0 : Math.round((added / total) * 5)
  return (
    <span className="xp-churn" aria-hidden>
      {Array.from({ length: 5 }, (_, index) => (
        <span key={index} data-kind={index < green ? "add" : "remove"} />
      ))}
    </span>
  )
}

/** The folder a file is in, to the depth that tells a large change's parts apart. */
function folderOf(path: string, depth = 3): string {
  const parts = path.split("/").slice(0, -1)
  return parts.slice(0, depth).join("/") || "."
}

/**
 * Where a large change landed: its busiest folders, each with how many files
 * and lines, and a bar for its share of the change.
 */
function Folders({ files }: { files: readonly FileChange[] }) {
  const folders = useMemo(() => {
    const byFolder = new Map<string, { files: number; added: number; removed: number }>()
    for (const file of files) {
      const folder = folderOf(file.path)
      const sum = byFolder.get(folder) ?? { files: 0, added: 0, removed: 0 }
      byFolder.set(folder, {
        files: sum.files + 1,
        added: sum.added + file.added,
        removed: sum.removed + file.removed,
      })
    }
    return [...byFolder.entries()]
      .map(([path, sum]) => ({ path, ...sum }))
      .sort((a, b) => b.added + b.removed - (a.added + a.removed))
  }, [files])
  const most = Math.max(...folders.map((each) => each.added + each.removed), 1)
  const rest = folders.length - foldersShown
  return (
    <div className="xp-folders">
      <h5>Where it landed</h5>
      <ul>
        {folders.slice(0, foldersShown).map((folder) => (
          <li
            key={folder.path}
            {...tooltip(`${folder.path}/ · ${exact(folder.files)} files`)}
          >
            <span className="xp-folder-path xp-truncate">{folder.path}/</span>
            <span className="xp-faint">{plural(folder.files, "file")}</span>
            <span className="xp-folder-bar" aria-hidden>
              <span
                style={{ width: `${((folder.added + folder.removed) / most) * 100}%` }}
              >
                <span
                  className="xp-folder-bar-add"
                  style={{
                    width: `${(folder.added / Math.max(folder.added + folder.removed, 1)) * 100}%`,
                  }}
                />
              </span>
            </span>
          </li>
        ))}
      </ul>
      {rest > 0 ? (
        <p className="xp-detail-note">
          and {plural(rest, "more folder", "more folders")}
        </p>
      ) : null}
    </div>
  )
}

/**
 * A file: its status, its name plainly and its folder quietly after it, its
 * lines, and what changed in it where the agent said. It opens in the
 * person's editor. Under its folder's heading it drops the folder.
 */
function FileRow({
  file,
  onOpen,
  inFolder = false,
}: {
  file: FileChange
  onOpen: (path: string) => void
  inFolder?: boolean
}) {
  const split = file.path.lastIndexOf("/")
  const folder = split >= 0 ? file.path.slice(0, split) : ""
  const name = file.path.slice(split + 1)
  const noted = Boolean(file.note) && !inFolder
  return (
    <button
      type="button"
      className="xp-file"
      data-noted={noted || undefined}
      onClick={() => onOpen(file.path)}
      {...tooltip(`Open ${file.path} in your editor`)}
    >
      <span
        className="xp-file-status"
        data-status={file.status}
        aria-label={statusLabels[file.status]}
      >
        {statusLetters[file.status]}
      </span>
      <span className="xp-file-path">
        <span className="xp-file-name">{name}</span>
        {inFolder || !folder ? null : <span className="xp-file-folder">{folder}</span>}
      </span>
      <span className="xp-file-lines">
        {file.added > 0 ? <span className="xp-add">+{count(file.added)}</span> : null}
        {file.removed > 0 ? (
          <span className="xp-remove">−{count(file.removed)}</span>
        ) : null}
        <span className="xp-file-open" aria-hidden>
          <OpenGlyph />
        </span>
      </span>
      {noted ? <span className="xp-file-note">{file.note}</span> : null}
    </button>
  )
}

type Line =
  | { readonly kind: "folder"; readonly path: string; readonly files: number }
  | { readonly kind: "file"; readonly file: FileChange }

/** Every file, found by any part of its path, grouped under its folder. */
function AllFiles({
  files,
  onOpen,
  onDone,
}: {
  files: readonly FileChange[]
  onOpen: (path: string) => void
  onDone: () => void
}) {
  const [query, setQuery] = useState("")
  const lines = useMemo(() => {
    const needle = query.trim().toLowerCase()
    const matching = needle
      ? files.filter((file) => file.path.toLowerCase().includes(needle))
      : files
    const folders = new Map<string, FileChange[]>()
    for (const file of matching) {
      const split = file.path.lastIndexOf("/")
      const folder = split >= 0 ? file.path.slice(0, split) : "."
      const inFolder = folders.get(folder) ?? []
      inFolder.push(file)
      folders.set(folder, inFolder)
    }
    return [...folders.entries()]
      .sort(([a], [b]) => a.localeCompare(b))
      .flatMap(([path, inFolder]): Line[] => [
        { kind: "folder", path, files: inFolder.length },
        ...inFolder
          .sort((a, b) => a.path.localeCompare(b.path))
          .map((file): Line => ({ kind: "file", file })),
      ])
  }, [files, query])
  const shown = lines.filter((line) => line.kind === "file").length
  return (
    <div className="xp-all-files">
      <div className="xp-all-files-head">
        <input
          className="xp-file-search"
          type="search"
          placeholder={`Filter ${plural(files.length, "file")}`}
          aria-label="Filter files"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          autoFocus
        />
        <button type="button" className="xp-link" onClick={onDone}>
          Fewer
        </button>
      </div>
      {lines.length === 0 ? (
        <p className="xp-detail-note">No file matches.</p>
      ) : (
        <VirtualList
          className="xp-files-list"
          items={lines}
          getKey={(line) =>
            line.kind === "folder" ? `folder:${line.path}` : line.file.path
          }
          rowHeight={fileRow}
          height={Math.min(lines.length * fileRow, 10 * fileRow)}
          aria-label="Changed files"
        >
          {(line) =>
            line.kind === "folder" ? (
              <div className="xp-files-folder">
                <span className="xp-truncate">{line.path}/</span>
                <span className="xp-faint">{count(line.files)}</span>
              </div>
            ) : (
              <FileRow file={line.file} onOpen={onOpen} inFolder />
            )
          }
        </VirtualList>
      )}
      {query ? (
        <p className="xp-detail-note">
          {plural(shown, "file")} of {count(files.length)}
        </p>
      ) : null}
    </div>
  )
}
