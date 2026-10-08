/**
 * What the workspace adds to the split panes' drag (`useSplitPanesDrag`):
 * what only the workspace knows of its page. A session carried from a list
 * is drawn as the pane it will open in — its heading and latest words, and
 * the composer it will have (`carriedSession`); the side columns drawn,
 * docked or revealed from the edge, are never targets (`sideColumns`); and a
 * copy leaves out the window's drag regions and the focused pane's mark.
 */
import type { SplitPanesDragOptions } from "../../../split-panes"
import type { DesktopStore } from "../../../store"
import { paneItemOf } from "../../model/pane-item"
import { focusedPaneAttribute } from "./focus"

/** How many of a session's latest messages its copy shows: a screen's worth. */
const latestShown = 12

/**
 * A session with no pane yet, as the window holds it: its heading and latest
 * words, and the composer it will have — the focused pane's, as it stands.
 * Built detached, from the store and the row pressed.
 */
export function carriedSession(store: DesktopStore): SplitPanesDragOptions["copyOf"] {
  return (key, { pressed, picture, focusedPane }) => {
    // A row carries its session's pane item; nothing else is carried in.
    const item = paneItemOf(key)
    const sessionId = item?.kind === "session" ? item.sessionId : ""
    const card = document.createElement("article")
    card.className = "workspace-pane"
    const header = document.createElement("header")
    header.className = "workspace-pane-header"
    const tile = pressed.querySelector(".workspace-agent-tile")
    if (tile) header.append(picture(tile))
    const body = document.createElement("div")
    // A conversation's body, so its heading and dock take a conversation's shape.
    body.className = "workspace-pane-body workspace-conversation"
    const transcript = document.createElement("div")
    // Its latest words at the foot, as a conversation opens: set by the copy's
    // own layout, not a scroll (`chrome.css`).
    transcript.className = "workspace-transcript workspace-carried-latest"
    const content = document.createElement("div")
    content.className = "workspace-transcript-inner"
    const heading = document.createElement("div")
    heading.className = "workspace-heading"
    const title = document.createElement("h2")
    const state = store.getState().workspace
    const summary = Object.hasOwn(state.sessions, sessionId)
      ? state.sessions[sessionId]
      : undefined
    title.textContent = summary?.title ?? pressed.textContent ?? ""
    heading.append(title)
    content.append(heading)
    const held = Object.hasOwn(state.transcripts, sessionId)
      ? state.transcripts[sessionId]
      : undefined
    // A screen's worth: the latest few, which is all the copy shows.
    const messages = (held?.messages ?? []).slice(-latestShown)
    if (messages.length === 0 && summary?.preview) {
      const line = document.createElement("p")
      line.className = "workspace-message"
      line.textContent = summary.preview
      content.append(line)
    }
    for (const message of messages) {
      const row = document.createElement("div")
      row.className = "workspace-message"
      row.dataset.role = message.role
      const text = message.parts
        .map((part) =>
          part.kind === "text" ? part.text : part.kind === "code" ? part.code : "",
        )
        .filter(Boolean)
        .join("\n\n")
      const block = document.createElement(message.role === "user" ? "div" : "p")
      if (message.role === "user") block.className = "workspace-bubble"
      block.textContent = text
      row.append(block)
      content.append(row)
    }
    transcript.append(content)
    body.append(transcript)
    // The composer it will have: the focused pane's, as it stands — a
    // conversation's dock, or a new session's home's, docked or not.
    const dock = focusedPane?.querySelector(".workspace-dock")
    if (dock) {
      const composer = picture(dock)
      composer.querySelectorAll("textarea").forEach((field) => (field.value = ""))
      body.append(composer)
    }
    card.append(header, body)
    return card
  }
}

/**
 * The side columns drawn under the workspace's root as the press begins —
 * the sidebar docked or revealed from the edge over the panes, the session
 * list docked — which are never targets, whatever is under them.
 */
export function sideColumns(root: HTMLElement): readonly Element[] {
  const { sidebar, list } = root.dataset
  return [
    sidebar === "open" || "peek" in root.dataset
      ? root.querySelector(".workspace-sidebar")
      : null,
    list === "open" ? root.querySelector(".workspace-list") : null,
  ].flatMap((column) => (column ? [column] : []))
}

/** Everything the workspace adds to a drag, built once for its store. */
export function workspaceDragOptions(store: DesktopStore): SplitPanesDragOptions {
  return {
    copyOf: carriedSession(store),
    covered: sideColumns,
    // A copy is no place to move the window from, and no pane of its own.
    stripped: ["data-tauri-drag-region", focusedPaneAttribute],
    // The conversation is not painted on the copy, and neither is the header
    // picture: its night scene is tens of thousands of characters, and shaping
    // them is the lift's long layout (`split-panes-drag.test.tsx`).
    dropped: [".workspace-pane-body", "[data-sliver]"],
    // Nor on the preview: a transform of its own would be a layer that is
    // never seen (`split-panes-drag.test.tsx`).
    unscaled: [".workspace-transcript"],
  }
}
