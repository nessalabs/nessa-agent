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
import "./session-row-actions.css"

export interface SessionRowActions {
  /** For the row's `SessionMenuItems`. */
  readonly items: { readonly onRename: () => void; readonly onDelete: () => void }
  /**
   * For each menu the row opens (`onCloseAutoFocus`): a menu closing on
   * Rename or Delete leaves focus to the field or the confirmation rather
   * than handing it back to the row.
   */
  readonly onCloseAutoFocus: (event: Event) => void
  /** Whether the title is being edited: the row then answers no click or key of its own. */
  readonly renaming: boolean
  /** The row's title, or the field that edits it. */
  readonly title: (title: string) => ReactNode
}

export function useSessionRowActions(sessionId: string): SessionRowActions {
  const dispatch = useWorkspaceDispatch()
  const draft = useWorkspaceSelector((state) => selectRenameDraft(state, sessionId))
  // Set by Rename or Delete until its menu has closed: the field and the
  // confirmation's trap take focus themselves, and the menu handing it back
  // to its button would take it from them.
  const handOff = useRef(false)
  return {
    items: {
      onRename: () => {
        handOff.current = true
        dispatch(renameStarted({ sessionId }))
      },
      onDelete: () => {
        handOff.current = true
        dispatch(deleteAsked({ sessionId }))
      },
    },
    onCloseAutoFocus: (event) => {
      if (!handOff.current) return
      handOff.current = false
      event.preventDefault()
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
