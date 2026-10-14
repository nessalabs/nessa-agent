/**
 * A Drive grant's room for hosts, and the request a draft becomes (row D9 of
 * `docs/design/read-grants.md` › "Dialog state and order").
 */
import { describe, expect, it } from "vitest"
import {
  draftRestarted,
  draftSettled,
  draftFromGrant,
  emptyDraft,
  hostFits,
  patternCount,
  patternsFrom,
  requestFrom,
  type ShareGrant,
} from "./sharing"

const limits = { patterns: 16, hosts: 16, hostBytes: 96 }
const none: readonly ShareGrant[] = []

describe("a Drive grant's hosts", () => {
  it("fit by UTF-8 bytes, not characters", () => {
    expect(hostFits(limits, "a".repeat(96))).toBe(true)
    expect(hostFits(limits, "a".repeat(97))).toBe(false)
    // 32 three-byte characters are 96 bytes; 33 are past it.
    expect(hostFits(limits, "€".repeat(32))).toBe(true)
    expect(hostFits(limits, "€".repeat(33))).toBe(false)
  })

  it("count only a custom policy's patterns", () => {
    expect(patternCount({ kind: "preset", preset: "all", commandHosts: ["a"] })).toBe(0)
    expect(
      patternCount({ kind: "rules", allow: ["a", "b"], deny: ["c"], commandHosts: [] }),
    ).toBe(3)
  })
})

describe("patterns typed", () => {
  it("are split on commas and lines, trimmed, blanks dropped", () => {
    expect(patternsFrom(" Read, Grep\n\nrun ,, ")).toEqual(["Read", "Grep", "run"])
    expect(patternsFrom("   ")).toEqual([])
  })

  it("keep a grant's own pattern with a comma whole wherever its text still stands", () => {
    const kept = ["Bash(git log,status)", "Read"]
    expect(patternsFrom("Bash(git log,status), Read, Grep", kept)).toEqual([
      "Bash(git log,status)",
      "Read",
      "Grep",
    ])
    // Its text changed: it is read as typed.
    expect(patternsFrom("Bash(git log,diff)", kept)).toEqual(["Bash(git log", "diff)"])
  })
})

const drive = (change: Partial<ReturnType<typeof emptyDraft>> = {}) => ({
  ...emptyDraft("phone"),
  role: "drive" as const,
  ...change,
})
const grant = (change: Partial<ShareGrant> = {}): ShareGrant => ({
  credentialId: "phone",
  role: "drive",
  policy: {
    kind: "rules",
    allow: ["Read"],
    deny: ["Write", "Edit"],
    commandHosts: ["devbox"],
  },
  policyRevision: 2,
  grantedAtMs: 1,
  ...change,
})

describe("a draft's request", () => {
  it("needs a device", () => {
    expect(requestFrom(emptyDraft(null), none, limits)).toEqual({
      ok: false,
      reason: "no-target",
    })
  })

  it("carries no policy for Read or Comment", () => {
    expect(
      requestFrom(
        { ...emptyDraft("phone"), role: "comment", hosts: ["x"] },
        none,
        limits,
      ),
    ).toEqual({
      ok: true,
      // 0: built on a list where the device held no grant.
      request: { credentialId: "phone", role: "comment", expectedPolicyRevision: 0 },
    })
  })

  it("carries a preset and every host checked, one not offered now included", () => {
    const draft = drive({ preset: "no_shell", hosts: ["build", "gone", "devbox"] })
    expect(requestFrom(draft, none, limits)).toEqual({
      ok: true,
      request: {
        credentialId: "phone",
        role: "drive",
        policy: {
          kind: "preset",
          preset: "no_shell",
          commandHosts: ["build", "gone", "devbox"],
        },
        expectedPolicyRevision: 0,
      },
    })
  })

  it("D9: needs a pattern for custom rules, and no more in a list than the schema allows", () => {
    const draft = drive({ preset: "custom" })
    expect(requestFrom(draft, none, limits)).toEqual({ ok: false, reason: "no-pattern" })
    expect(requestFrom({ ...draft, deny: "Write" }, none, limits)).toMatchObject({
      ok: true,
      request: {
        policy: { kind: "rules", allow: [], deny: ["Write"], commandHosts: [] },
      },
    })
    const many = (count: number) =>
      Array.from({ length: count }, (_, at) => `p${at}`).join(",")
    expect(requestFrom({ ...draft, allow: many(16) }, none, limits).ok).toBe(true)
    expect(requestFrom({ ...draft, allow: many(17) }, none, limits)).toEqual({
      ok: false,
      reason: "too-many-patterns",
    })
  })

  it("D9: holds the hosts to their own bound, however full the patterns are", () => {
    const hosts = Array.from({ length: limits.hosts }, (_, at) => `h${at}`)
    const draft = drive({ preset: "custom", hosts })
    const patterns = (count: number) =>
      Array.from({ length: count }, (_, at) => `p${at}`).join(",")
    expect(
      requestFrom({ ...draft, allow: patterns(16), deny: patterns(16) }, none, limits).ok,
    ).toBe(true)
    expect(
      requestFrom({ ...draft, allow: "Read", hosts: [...hosts, "extra"] }, none, limits),
    ).toEqual({
      ok: false,
      reason: "too-many-hosts",
    })
  })

  it("D9: is held when the device's grant changed since the draft began", () => {
    const draft = draftFromGrant(grant())
    // Built on the grant: it names the revision it was built on.
    expect(requestFrom(draft, [grant()], limits)).toEqual({
      ok: true,
      request: {
        credentialId: "phone",
        role: "drive",
        policy: grant().policy,
        expectedPolicyRevision: 2,
      },
    })
    for (const grants of [[grant({ policyRevision: 3 })], []])
      expect(requestFrom(draft, grants, limits)).toEqual({
        ok: false,
        reason: "changed",
      })
    // A device that held nothing, and now holds a grant made elsewhere.
    expect(requestFrom(emptyDraft("phone"), [grant()], limits)).toEqual({
      ok: false,
      reason: "changed",
    })
  })
})

describe("a draft from a grant", () => {
  it("starts where the grant stands, its patterns kept exactly", () => {
    const comma = grant({
      policy: { kind: "rules", allow: ["mcp__a,b", "Read"], deny: [], commandHosts: [] },
    })
    const draft = draftFromGrant(comma)
    expect(draft).toMatchObject({ basis: 2, preset: "custom", allow: "mcp__a,b, Read" })
    expect(requestFrom(draft, [comma], limits)).toMatchObject({
      request: { policy: { allow: ["mcp__a,b", "Read"] } },
    })
    expect(
      requestFrom({ ...draft, allow: `${draft.allow}, Grep` }, [comma], limits),
    ).toMatchObject({ request: { policy: { allow: ["mcp__a,b", "Read", "Grep"] } } })
    expect(draftFromGrant(grant({ role: "comment", policy: undefined }))).toEqual({
      ...emptyDraft("phone"),
      basis: 2,
      role: "comment",
    })
  })

  it("D10: builds on the revision the gateway answered its own device's change with, keeping what was set", () => {
    const draft = drive({ preset: "all", hosts: ["devbox"] })
    expect(draftSettled(draft, "tablet", 2)).toBe(draft)
    expect(draftSettled(draft, "phone", 2)).toEqual({ ...draft, basis: 2 })
    expect(draftSettled({ ...draft, basis: 2 }, "phone", null)).toEqual(draft)
    expect(requestFrom(draftSettled(draft, "phone", 2), [grant()], limits)).toMatchObject(
      {
        ok: true,
        request: { expectedPolicyRevision: 2 },
      },
    )
    // A list read later that holds another window's change shows it as changed.
    expect(
      requestFrom(
        draftSettled(draft, "phone", 2),
        [grant({ policyRevision: 5 })],
        limits,
      ),
    ).toEqual({ ok: false, reason: "changed" })
  })

  it("starts again from what its device holds, for Start Again", () => {
    const draft = drive({ preset: "all" })
    expect(draftRestarted(draft, [grant()])).toEqual(draftFromGrant(grant()))
    expect(draftRestarted(draft, [])).toEqual(emptyDraft("phone"))
    const none = emptyDraft(null)
    expect(draftRestarted(none, [grant()])).toBe(none)
  })
})
