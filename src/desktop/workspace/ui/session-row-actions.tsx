/**
 * What a session row carries out past the menu that asked for it: renaming
 * in place, where its title was, and deleting once the person has said so in
 * a confirmation. The window holds both (`usecases/row-editing.ts`), so a row
 * drawn afresh keeps its edit; a row that offers them hands `items` to its
 * `SessionMenuItems`, and `DeleteSessionHost` asks the question once for the
 * whole window, outside every list.
 */
import { useEffect, useRef, type ReactNode } from "react"
import { Button } from "@nessa-ui/react/button"
import { Dialog, DialogClose } from "@nessa-ui/react/dialog"
import { Input } from "@nessa-ui/react/input"
import {
  deleteAsked,
  deleteClosed,
  deleteSession,
  renameDrafted,
  renameEnded,
  renameSession,
  renameStarted,
} from "../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../adapters/store/hooks"
import { selectDeleteAsked, selectRenameDraft } from "../adapters/store/selectors"
import { focusRowPlace, rowPlace, type RowPlace } from "./row-focus"
import "./session-row-actions.css"

export interface SessionRowActions {
  /** On the row itself: whose place its menus note as they open. */
  readonly ref: (node: HTMLElement | null) => void
  /** For the row's `SessionMenuItems`. */
  readonly items: { readonly onRename: () => void; readonly onDelete: () => void }
  /** For each menu the row opens (`onOpenChange`). */
  readonly onMenuOpen: (open: boolean) => void
  /**
   * For each menu the row opens (`onCloseAutoFocus`), and the only say in
   * where focus goes as it closes: on Rename or Delete, to the field or the
   * confirmation; otherwise back where it was as the menu opened, or to the
   * row, or past it if the row went (`row-focus.ts`).
   */
  readonly onCloseAutoFocus: (event: Event) => void
  /** Whether the title is being edited: the row then answers no click or key of its own. */
  readonly renaming: boolean
  /** The row's title, or the field that edits it. */
  readonly title: (title: string) => ReactNode
}

/** As a row's menu opens: the row's place, and where focus was. */
interface Opened {
  readonly place: RowPlace | null
  readonly from: Element | null
}

// Where the delete question was asked from: as it closes, focus goes back as
// from the menu that asked it (`giveBack`).
let deleteFrom: Opened | null = null

/**
 * Focus as a row's menu or question closes: back where it was as the menu
 * opened, if that is still there; else to the row asked from, or past it if
 * it went (`row-focus.ts`).
 */
function giveBack(opened: Opened | null) {
  const from = opened?.from
  if (from instanceof HTMLElement && from.isConnected && from !== from.ownerDocument.body)
    from.focus({ preventScroll: true })
  else focusRowPlace(opened?.place ?? null)
}

export function useSessionRowActions(sessionId: string): SessionRowActions {
  const dispatch = useWorkspaceDispatch()
  const draft = useWorkspaceSelector((state) => selectRenameDraft(state, sessionId))
  // Set by Rename or Delete until its menu has closed: the field and the
  // confirmation's trap take focus themselves, and the menu handing it back
  // to its button would take it from them.
  const handOff = useRef(false)
  const node = useRef<HTMLElement | null>(null)
  const opened = useRef<Opened | null>(null)
  return {
    ref: (next) => {
      node.current = next
    },
    items: {
      onRename: () => {
        handOff.current = true
        dispatch(renameStarted({ sessionId }))
      },
      onDelete: () => {
        handOff.current = true
        deleteFrom = opened.current ?? { place: rowPlace(node.current), from: null }
        dispatch(deleteAsked({ sessionId }))
      },
    },
    onMenuOpen: (open) => {
      if (open)
        opened.current = {
          place: rowPlace(node.current),
          from: node.current?.ownerDocument.activeElement ?? null,
        }
    },
    onCloseAutoFocus: (event) => {
      const asked = opened.current
      opened.current = null
      // Never to the menu's opener, which may be the pointer's button.
      event.preventDefault()
      if (handOff.current) {
        handOff.current = false
        return
      }
      giveBack(asked ?? { place: rowPlace(node.current), from: null })
    },
    renaming: draft !== null,
    title: (title) =>
      draft === null ? (
        title
      ) : (
        <RenameField
          title={title}
          draft={draft}
          onDraft={(next) => dispatch(renameDrafted({ sessionId, draft: next }))}
          onDone={(next) => {
            dispatch(renameEnded({ sessionId }))
            if (next !== null)
              void dispatch(
                renameSession({ sessionId, title: next, initiator: "person" }),
              )
          }}
        />
      ),
  }
}

/**
 * The title, editable where it stood. ↩ or leaving the field keeps what was
 * typed; Escape keeps the title as it was. Nothing in it reaches the row: a
 * key or a click here is the field's, never the row's opening.
 */
function RenameField({
  title,
  draft,
  onDraft,
  onDone,
}: {
  title: string
  /** What is typed so far, held by the window. */
  draft: string
  onDraft: (draft: string) => void
  /** What was typed, or `null` to leave the title as it was. */
  onDone: (next: string | null) => void
}) {
  const input = useRef<HTMLInputElement>(null)
  // The field takes focus as it appears: the menu that asked for it may
  // already be gone, its own hand-off with it. Its words are chosen only
  // while untouched, so a row drawn again mid-edit keeps what is typed.
  const untouched = useRef(draft === title)
  useEffect(() => {
    input.current?.focus()
    if (untouched.current) input.current?.select()
  }, [])
  // One answer per edit: Escape or ↩ ends it, and the blur that follows says nothing more.
  const done = useRef(false)
  const finish = (next: string | null) => {
    if (done.current) return
    done.current = true
    onDone(next)
  }
  return (
    <Input
      ref={input}
      className="workspace-rename"
      aria-label="Session title"
      data-session-rename=""
      value={draft}
      spellCheck={false}
      onChange={(event) => onDraft(event.target.value)}
      onBlur={() => finish(draft)}
      onClick={(event) => event.stopPropagation()}
      onDoubleClick={(event) => event.stopPropagation()}
      onKeyDown={(event) => {
        event.stopPropagation()
        if (event.key !== "Enter" && event.key !== "Escape") return
        // An IME's Enter takes its candidate and its Escape drops it, not the
        // title: Safari says so only by keyCode 229.
        if (event.nativeEvent.isComposing || event.keyCode === 229) return
        event.preventDefault()
        finish(event.key === "Enter" ? draft : null)
        // Ended by a key, the row it is in takes focus back, as Finder's does;
        // the blur that comes with it is past the one answer.
        event.currentTarget.parentElement
          ?.closest<HTMLElement>("[tabindex]")
          ?.focus({ preventScroll: true })
      }}
    />
  )
}

/**
 * The delete question, while one is asked: once for the window and outside
 * every list, so nothing pressed in it reaches a row or a list's keys, and
 * it stays while its row is drawn again elsewhere. A deleted session's
 * conversation, uploads and summary are gone for good.
 */
export function DeleteSessionHost() {
  const dispatch = useWorkspaceDispatch()
  const sessionId = useWorkspaceSelector(selectDeleteAsked)
  if (sessionId === null) return null
  const close = () => dispatch(deleteClosed())
  return (
    <Dialog
      open
      onOpenChange={(next) => {
        if (!next) close()
      }}
      // It was asked from a menu that is gone, whose opener may be a
      // pointer's button the keys cannot use: focus goes to the row it was
      // asked from instead, or past it once deleted, so the list keeps its keys.
      onCloseAutoFocus={(event) => {
        event.preventDefault()
        const asked = deleteFrom
        deleteFrom = null
        giveBack(asked)
      }}
      size="sm"
      title="Delete this session?"
      description="It can’t be brought back."
      showCloseButton={false}
      data-delete-session=""
      footer={
        <>
          <DialogClose asChild>
            <Button variant="outline">Cancel</Button>
          </DialogClose>
          <Button
            variant="destructive"
            data-delete-action="confirm"
            onClick={() => {
              close()
              void dispatch(deleteSession({ sessionId, initiator: "person" }))
            }}
          >
            Delete
          </Button>
        </>
      }
    />
  )
}
