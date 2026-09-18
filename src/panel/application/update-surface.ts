/**
 * What an available update puts on the panel, and when it stops putting it
 * there.
 *
 * The update belongs to the panel rather than to a conversation: it is a fact
 * about the application somebody is running, it survives every tab they open or
 * close, and its two surfaces — a notice above the composer and a tab beside
 * the conversation tabs — are both panel chrome. Nothing about it is a
 * conversation's business.
 *
 * ```text
 *   host announces ──▶ noticed ──install──▶ downloading ──▶ restart
 *                         │                      │
 *                      dismiss                 failed ──retry──┘
 *                         │                      │
 *                       quiet ◀────────────── closed
 * ```
 *
 * A function rather than branches inside the surface, for the reason
 * `readiness-check.ts` and `setup-recovery.ts` are: the decision is the part
 * worth testing, and there is nothing here a React effect could be driven
 * through — the events all come from a host process that owns a release
 * endpoint.
 *
 * Dismissal is scoped to a version and to a launch. `dismissed` is ordinary
 * state in the panel's own memory, so it is gone when the app restarts and gone
 * when the page reloads; nothing about it is written down. A newer version is
 * not the version that was dismissed, so it notices again.
 */

import type { Release } from "../../host"

/**
 * The update tab's id in the tab strip. Not a conversation id: conversations
 * are identified by the ids the gateway hands out, and nothing generates this
 * one.
 */
export const UPDATE_TAB_ID = "nessa:update"

/** What the panel knows about an update, at one moment. */
export interface UpdateState {
  /** The release being shown, or `null` when there is nothing to show. */
  release: Release | null
  /** Versions dismissed during this launch, which are never offered again. */
  dismissed: readonly string[]
  /** Which of the two surfaces, if either, the update is on. */
  stage: "quiet" | "noticed" | "downloading" | "failed"
  /** Bytes received so far, while downloading. */
  downloaded: number
  /** Bytes the server declared, or `null` when it declared none. */
  total: number | null
}

/** No check has answered yet, which is also every launch that finds nothing. */
export const noUpdate: UpdateState = {
  release: null,
  dismissed: [],
  stage: "quiet",
  downloaded: 0,
  total: null,
}

/** Everything that can move an update along. */
export type UpdateEvent =
  /** A check found a newer published release. */
  | { kind: "announced"; release: Release }
  /** The install control on the notice. */
  | { kind: "install" }
  /** The dismiss control on the notice. */
  | { kind: "dismiss" }
  /** The host reporting how far the download has got. */
  | { kind: "progress"; downloaded: number; total: number | null }
  /** The host reporting that the install did not happen. */
  | { kind: "failed" }
  /** The update tab being closed, which only a failed one can be. */
  | { kind: "closed" }

/** Whether this version has already been turned down this launch. */
function dismissed(state: UpdateState, version: string): boolean {
  return state.dismissed.includes(version)
}

/**
 * The update's whole life, in one rule.
 *
 * An announcement is ignored while a download is running and ignored for a
 * version already dismissed — both are the same principle, that an update only
 * ever interrupts somebody once.
 */
export function afterUpdate(state: UpdateState, event: UpdateEvent): UpdateState {
  switch (event.kind) {
    case "announced": {
      // A download in flight is already this update's whole presence on
      // screen; a dismissal was an answer to this exact version.
      if (state.stage !== "quiet" && state.stage !== "noticed") return state
      if (dismissed(state, event.release.version)) return state
      return {
        ...state,
        release: event.release,
        stage: "noticed",
        downloaded: 0,
        total: null,
      }
    }
    case "install": {
      // Taken from the notice, or from the retry a failed download offers.
      if (state.release === null) return state
      if (state.stage !== "noticed" && state.stage !== "failed") return state
      return { ...state, stage: "downloading", downloaded: 0, total: null }
    }
    case "dismiss": {
      if (state.stage !== "noticed" || state.release === null) return state
      return {
        ...state,
        release: null,
        dismissed: [...state.dismissed, state.release.version],
        stage: "quiet",
      }
    }
    case "progress": {
      if (state.stage !== "downloading") return state
      return { ...state, downloaded: event.downloaded, total: event.total }
    }
    case "failed": {
      // A refusal can arrive before a single byte does — there may be no
      // release behind the offer at all — so this is reachable from the notice
      // as well as from a download in flight.
      if (state.release === null) return state
      return { ...state, stage: "failed" }
    }
    case "closed": {
      // Only a failed tab can be closed, and closing it is the same answer as
      // dismissing the notice was: not this version, not this launch.
      if (state.stage !== "failed" || state.release === null) return state
      return {
        ...state,
        release: null,
        dismissed: [...state.dismissed, state.release.version],
        stage: "quiet",
      }
    }
    default:
      return state
  }
}

/** The notice above the composer: two words, a version, and two controls. */
export interface UpdateNotice {
  /** The version, under the heading. */
  version: string
  /** The install control's accessible name. It carries no visible text. */
  installLabel: string
  /** The dismiss control's accessible name, for the same reason. */
  dismissLabel: string
}

/**
 * The notice, or `null` when the panel has nothing to say.
 *
 * Both labels name the version. An icon-only control whose name is "Install"
 * tells somebody using a screen reader less than the notice tells everybody
 * else, and these two controls do opposite things.
 */
export function updateNotice(state: UpdateState): UpdateNotice | null {
  if (state.stage !== "noticed" || state.release === null) return null
  const version = state.release.version
  return {
    version,
    installLabel: `Install update ${version}`,
    dismissLabel: `Dismiss update ${version}`,
  }
}

/** What the release published, as the tab lays it out. */
export interface ReleaseNotes {
  /** The line under the versions. */
  headline: string
  /** The rest of the notes, one per line published. */
  points: readonly string[]
  /** Said instead of the notes when the release published none. */
  empty: string | null
}

/**
 * The notes, split into a headline and the points under it.
 *
 * The manifest publishes one `notes` string. Its first line is the headline and
 * the remaining lines are the points, which is how release notes are written
 * anyway; leading list markers are dropped because the tab draws its own.
 *
 * Nothing fills that field yet, so the empty case is the one that ships first
 * and it is a written sentence rather than a blank space.
 */
export function releaseNotes(notes: string | null): ReleaseNotes {
  const lines = (notes ?? "")
    .split("\n")
    .map((line) => line.replace(/^\s*[-*•]\s+/, "").trim())
    .filter((line) => line.length > 0)
  const [headline, ...points] = lines
  if (headline === undefined) {
    return {
      headline: "Downloading the update",
      points: [],
      empty: "This release published no notes.",
    }
  }
  return { headline, points, empty: null }
}

/** The download's own line, under the notes. */
export type UpdateProgress =
  | {
      kind: "downloading"
      /** The word beside the percentage. */
      label: string
      /** Whole percent, or `null` when the server declared no length. */
      percent: number | null
    }
  | {
      kind: "failed"
      /** What happened, in a sentence, with nothing technical in it. */
      statement: string
      /** The way out: the same install, asked for again. */
      retryLabel: string
    }

/** The update tab, which is the download and whatever the release said. */
export interface UpdateTab {
  /** The version this build is on. */
  from: string
  /** The version it is going to. */
  to: string
  notes: ReleaseNotes
  progress: UpdateProgress
  /**
   * Whether the tab can be closed. A download in flight cannot be stopped, so
   * offering to close it would be a control that does not do what it says; a
   * failed one has nothing left to run and closing it is the way out.
   */
  closeable: boolean
}

/** The tab, or `null` when no install has been asked for. */
export function updateTab(state: UpdateState): UpdateTab | null {
  if (state.release === null) return null
  if (state.stage !== "downloading" && state.stage !== "failed") return null
  return {
    from: state.release.from,
    to: state.release.version,
    notes: releaseNotes(state.release.notes),
    progress:
      state.stage === "failed"
        ? {
            kind: "failed",
            statement:
              "The download did not finish. Nessa is still on the version it had.",
            retryLabel: `Retry update ${state.release.version}`,
          }
        : {
            kind: "downloading",
            label: "Downloading",
            percent: downloadedPercent(state),
          },
    closeable: state.stage === "failed",
  }
}

/**
 * How much of the download has arrived, as a whole percent.
 *
 * `null` when the server declared no length: a bar drawn from a number nobody
 * has is a bar that lies, and the tab draws an unmeasured one instead.
 */
function downloadedPercent(state: UpdateState): number | null {
  if (state.total === null || state.total <= 0) return null
  return Math.min(100, Math.floor((state.downloaded / state.total) * 100))
}
