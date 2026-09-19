import { describe, expect, it } from "vitest"

import {
  afterUpdate,
  noUpdate,
  releaseNotes,
  updateNotice,
  updateTab,
  type UpdateEvent,
  type UpdateState,
} from "./update-surface"
import type { Release } from "../../host"

const published: Release = { from: "0.1.0", version: "0.1.1", notes: null }

/** The panel, after everything that has happened to it so far. */
function panel(...events: readonly UpdateEvent[]): UpdateState {
  return events.reduce(afterUpdate, noUpdate)
}

describe("an update that has been found", () => {
  it("shows nothing at all until a check finds something", () => {
    expect(updateNotice(noUpdate)).toBeNull()
    expect(updateTab(noUpdate)).toBeNull()
  })

  it("puts a notice up naming the version", () => {
    const notice = updateNotice(panel({ kind: "announced", release: published }))

    expect(notice?.version).toBe("0.1.1")
    // The two controls carry no text, so their names are the only thing a
    // screen reader has to tell them apart by.
    expect(notice?.installLabel).toBe("Install update 0.1.1")
    expect(notice?.dismissLabel).toBe("Dismiss update 0.1.1")
  })

  it("has no tab until the install is taken", () => {
    expect(updateTab(panel({ kind: "announced", release: published }))).toBeNull()
  })
})

describe("dismissing the notice", () => {
  it("leaves no trace", () => {
    const after = panel({ kind: "announced", release: published }, { kind: "dismiss" })

    expect(updateNotice(after)).toBeNull()
    expect(updateTab(after)).toBeNull()
  })

  it("keeps that version from noticing again this launch", () => {
    const after = panel(
      { kind: "announced", release: published },
      { kind: "dismiss" },
      { kind: "announced", release: published },
    )

    expect(updateNotice(after)).toBeNull()
  })

  it("does not silence a newer version", () => {
    // The whole point of scoping the dismissal to a version: turning down
    // 0.1.1 is not turning down everything that comes after it.
    const after = panel(
      { kind: "announced", release: published },
      { kind: "dismiss" },
      { kind: "announced", release: { from: "0.1.0", version: "0.2.0", notes: null } },
    )

    expect(updateNotice(after)?.version).toBe("0.2.0")
  })

  it("cannot be taken twice, or after the download has started", () => {
    const downloading = panel(
      { kind: "announced", release: published },
      { kind: "install" },
      { kind: "dismiss" },
    )

    expect(updateTab(downloading)?.progress.kind).toBe("downloading")
    expect(afterUpdate(noUpdate, { kind: "dismiss" })).toEqual(noUpdate)
  })
})

describe("taking the install", () => {
  it("takes the notice down and opens the tab on the two versions", () => {
    const after = panel({ kind: "announced", release: published }, { kind: "install" })

    expect(updateNotice(after)).toBeNull()
    const tab = updateTab(after)
    expect(tab?.from).toBe("0.1.0")
    expect(tab?.to).toBe("0.1.1")
  })

  it("starts the bar unmeasured and moves it as bytes are declared", () => {
    const started = panel({ kind: "announced", release: published }, { kind: "install" })
    expect(updateTab(started)?.progress).toEqual({
      kind: "downloading",
      label: "Downloading",
      percent: null,
    })

    const measured = afterUpdate(started, {
      kind: "progress",
      downloaded: 620,
      total: 1000,
    })
    expect(updateTab(measured)?.progress).toEqual({
      kind: "downloading",
      label: "Downloading",
      percent: 62,
    })
  })

  it("keeps an unmeasured download unmeasured rather than inventing a percentage", () => {
    const after = panel(
      { kind: "announced", release: published },
      { kind: "install" },
      { kind: "progress", downloaded: 900_000, total: null },
    )

    expect(
      updateTab(after)?.progress.kind === "downloading" && updateTab(after)?.progress,
    ).toMatchObject({ percent: null })
  })

  it("never draws more than a full bar", () => {
    const after = panel(
      { kind: "announced", release: published },
      { kind: "install" },
      { kind: "progress", downloaded: 1_100, total: 1_000 },
    )

    expect(updateTab(after)?.progress).toMatchObject({ percent: 100 })
  })

  /**
   * Closing a running download hides it; it cannot stop it, because the host
   * has neither a cancel nor a timeout. Refusing the close instead left a tab
   * reading "Downloading the update" for the rest of the launch whenever a
   * download stalled — a gateway that accepts the connection and then answers
   * nothing produces no bytes, no error and no end.
   */
  it("closes while running, and the install carries on without it", () => {
    const running = panel({ kind: "announced", release: published }, { kind: "install" })
    expect(running.stage).toBe("downloading")
    expect(updateTab(running)?.closeable).toBe(true)

    const hidden = afterUpdate(running, { kind: "closed" })

    expect(hidden.stage).toBe("installing")
    expect(updateTab(hidden)).toBeNull()
    // Not a dismissal: the version was not turned down, it is being installed.
    expect(hidden.dismissed).toEqual([])
    expect(hidden.release).not.toBeNull()
  })

  /** The answer is still owed to whoever asked for the install. */
  it("brings the tab back when a download it stopped watching fails", () => {
    const hidden = panel(
      { kind: "announced", release: published },
      { kind: "install" },
      { kind: "progress", downloaded: 400, total: 1_000 },
      { kind: "closed" },
    )
    expect(hidden.stage).toBe("installing")

    const failed = afterUpdate(hidden, { kind: "failed" })

    expect(failed.stage).toBe("failed")
    expect(updateTab(failed)?.progress.kind).toBe("failed")
    // And closing it now is the turning-down the closed tab was not.
    const gone = afterUpdate(failed, { kind: "closed" })
    expect(updateTab(gone)).toBeNull()
    expect(gone.dismissed).toEqual([published.version])
  })

  /** A hidden download still counts, so a later failure is not a blank one. */
  it("keeps counting bytes while nobody is watching", () => {
    const hidden = panel(
      { kind: "announced", release: published },
      { kind: "install" },
      { kind: "closed" },
      { kind: "progress", downloaded: 700, total: 1_000 },
    )

    expect(hidden.downloaded).toBe(700)
  })
})

describe("a download that was refused", () => {
  it("says so plainly and offers the way back in", () => {
    const after = panel(
      { kind: "announced", release: published },
      { kind: "install" },
      { kind: "failed" },
    )

    const progress = updateTab(after)?.progress
    expect(progress?.kind).toBe("failed")
    expect(progress).toMatchObject({
      statement: "The download did not finish. Nessa is still on the version it had.",
      retryLabel: "Retry update 0.1.1",
    })
    // Nothing technical, and nothing that leaves somebody wondering which
    // version they are on.
    expect(progress?.kind === "failed" && progress.statement).not.toMatch(/error|http/i)
  })

  it("takes the retry and downloads again from nothing", () => {
    const retried = panel(
      { kind: "announced", release: published },
      { kind: "install" },
      { kind: "progress", downloaded: 500, total: 1_000 },
      { kind: "failed" },
      { kind: "install" },
    )

    expect(updateTab(retried)?.progress).toEqual({
      kind: "downloading",
      label: "Downloading",
      percent: null,
    })
  })

  it("is the one update tab that can be closed, and closing it is a dismissal", () => {
    const failed = panel(
      { kind: "announced", release: published },
      { kind: "install" },
      { kind: "failed" },
    )
    expect(updateTab(failed)?.closeable).toBe(true)

    const closed = afterUpdate(failed, { kind: "closed" })
    expect(updateTab(closed)).toBeNull()
    // Closing it is an answer about this version, so it does not come straight
    // back as a notice.
    expect(
      updateNotice(afterUpdate(closed, { kind: "announced", release: published })),
    ).toBeNull()
  })

  it("can be refused before a single byte arrives", () => {
    // What a simulated offer does: there is no release behind it, so the
    // refusal comes back instead of any progress at all.
    const after = panel(
      { kind: "announced", release: published },
      { kind: "install" },
      { kind: "failed" },
    )

    expect(updateTab(after)?.progress.kind).toBe("failed")
  })

  it("is not reported when nothing was ever announced", () => {
    expect(afterUpdate(noUpdate, { kind: "failed" })).toEqual(noUpdate)
  })
})

describe("the headline, which is the tab's loudest line", () => {
  const taken = panel({ kind: "announced", release: published }, { kind: "install" })

  it("says what a download in flight is doing, measured or not", () => {
    expect(updateTab(taken)?.headline).toBe("Downloading the update")
    expect(
      updateTab(afterUpdate(taken, { kind: "progress", downloaded: 620, total: 1_000 }))
        ?.headline,
    ).toBe("Downloading the update")
  })

  it("says the restart is coming once the last byte is in", () => {
    const full = afterUpdate(taken, {
      kind: "progress",
      downloaded: 1_000,
      total: 1_000,
    })

    expect(updateTab(full)?.headline).toBe("Restarting to finish the update")
  })

  it("stops claiming a download the moment one is refused", () => {
    // The defect this exists for: the tab headed "Downloading the update" while
    // the sentence under it said the download had not finished.
    const failed = afterUpdate(taken, { kind: "failed" })
    const tab = updateTab(failed)

    expect(tab?.headline).toBe("The update did not install")
    expect(tab?.headline).not.toMatch(/downloading/i)
    expect(tab?.progress.kind).toBe("failed")
  })

  it("goes back to the download when the retry is taken", () => {
    const retried = panel(
      { kind: "announced", release: published },
      { kind: "install" },
      { kind: "failed" },
      { kind: "install" },
    )

    expect(updateTab(retried)?.headline).toBe("Downloading the update")
  })

  it("keeps a bar that is not quite full from announcing the restart", () => {
    const nearly = afterUpdate(taken, {
      kind: "progress",
      downloaded: 999,
      total: 1_000,
    })

    expect(updateTab(nearly)?.headline).toBe("Downloading the update")
  })
})

describe("what the release published", () => {
  it("holds up when it published nothing, which is what ships first", () => {
    const notes = releaseNotes(null)

    expect(notes.title).toBeNull()
    expect(notes.points).toEqual([])
    expect(notes.empty).toBe("This release published no notes.")
  })

  it("treats notes that are only blank space as none", () => {
    expect(releaseNotes("  \n \n ").empty).toBe("This release published no notes.")
  })

  it("reads the first line as the release's title and the rest as the points", () => {
    const notes = releaseNotes(
      "Setup stops running every launch\n" +
        "- First-run setup is remembered.\n" +
        "* Settings are never replaced when the file cannot be read.\n",
    )

    expect(notes.title).toBe("Setup stops running every launch")
    expect(notes.points).toEqual([
      "First-run setup is remembered.",
      "Settings are never replaced when the file cannot be read.",
    ])
    expect(notes.empty).toBeNull()
  })

  it("says nothing about emptiness when there is a single line", () => {
    const notes = releaseNotes("A quiet release")

    expect(notes).toEqual({ title: "A quiet release", points: [], empty: null })
  })

  it("never lets the release's own title stand in for the state", () => {
    // The title is whatever the release called itself; the headline is what the
    // download is doing. Keeping them apart is the whole of the fix — a tab
    // that says "Downloading" because of a *string in the manifest* would say
    // it over a failure too.
    const after = panel(
      {
        kind: "announced",
        release: { from: "0.1.0", version: "0.1.1", notes: "Faster startup" },
      },
      { kind: "install" },
      { kind: "failed" },
    )

    const tab = updateTab(after)
    expect(tab?.notes.title).toBe("Faster startup")
    expect(tab?.headline).toBe("The update did not install")
  })

  it("carries the notes into the tab", () => {
    const after = panel(
      {
        kind: "announced",
        release: { from: "0.1.0", version: "0.1.1", notes: "Faster\n- Less waiting" },
      },
      { kind: "install" },
    )

    expect(updateTab(after)?.notes.points).toEqual(["Less waiting"])
  })
})
