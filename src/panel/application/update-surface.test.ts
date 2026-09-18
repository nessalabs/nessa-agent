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

  it("cannot be closed while it is running", () => {
    const running = panel({ kind: "announced", release: published }, { kind: "install" })

    expect(running.stage).toBe("downloading")
    expect(updateTab(running)?.closeable).toBe(false)
    // And asking anyway changes nothing: the download cannot be stopped.
    expect(updateTab(afterUpdate(running, { kind: "closed" }))?.progress.kind).toBe(
      "downloading",
    )
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

describe("what the release published", () => {
  it("holds up when it published nothing, which is what ships first", () => {
    const notes = releaseNotes(null)

    expect(notes.headline).toBe("Downloading the update")
    expect(notes.points).toEqual([])
    expect(notes.empty).toBe("This release published no notes.")
  })

  it("treats notes that are only blank space as none", () => {
    expect(releaseNotes("  \n \n ").empty).toBe("This release published no notes.")
  })

  it("reads the first line as the headline and the rest as the points", () => {
    const notes = releaseNotes(
      "Setup stops running every launch\n" +
        "- First-run setup is remembered.\n" +
        "* Settings are never replaced when the file cannot be read.\n",
    )

    expect(notes.headline).toBe("Setup stops running every launch")
    expect(notes.points).toEqual([
      "First-run setup is remembered.",
      "Settings are never replaced when the file cannot be read.",
    ])
    expect(notes.empty).toBeNull()
  })

  it("says nothing about emptiness when there is a single line", () => {
    const notes = releaseNotes("A quiet release")

    expect(notes).toEqual({ headline: "A quiet release", points: [], empty: null })
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
