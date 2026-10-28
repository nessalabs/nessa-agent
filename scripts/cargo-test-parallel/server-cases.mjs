// Windows execution ownership; test bodies and libtest concurrency are unchanged.
export const cataloguePrefix =
  "product::socket::tests::subscriptions::paired_lists::stress::paired_catalogue_seeded_mock_agent_stress_"
export const catalogueCases = ["seed_1", "seed_7", "seed_41", "under_load"].map(
  (suffix) => `${cataloguePrefix}${suffix}`,
)
export const conversationFilter = "conversation::"

/** Parse libtest's complete --list output, including ignored tests as cases. */
export function testRegistry(output) {
  const lines = output.trim().split(/\r?\n/)
  const summary = /^(\d+) tests?, (\d+) benchmarks?$/.exec(lines.pop() ?? "")
  if (!summary || Number(summary[2]) !== 0)
    throw new Error("invalid libtest registry summary")
  const names = lines
    .filter((line) => line !== "")
    .map((line) => {
      const match = /^(.+): test$/.exec(line)
      if (!match) throw new Error("invalid libtest registry entry")
      return match[1]
    })
  if (names.length !== Number(summary[1]) || new Set(names).size !== names.length)
    throw new Error("incomplete or duplicate libtest registry")
  return names.sort()
}

export function serverCasePlan(registry) {
  const seeded = registry.filter((name) => name.includes(cataloguePrefix))
  if (JSON.stringify(seeded) !== JSON.stringify([...catalogueCases].sort()))
    throw new Error("expected exactly four seeded paired-catalogue cases")
  const ordinary = registry.filter((name) => !name.includes(cataloguePrefix))
  if (ordinary.length === 0) throw new Error("server library has no ordinary cases")
  const conversation = ordinary.filter((name) => name.includes(conversationFilter))
  const other = ordinary.filter((name) => !name.includes(conversationFilter))
  if (conversation.length === 0 || other.length === 0)
    throw new Error("server library must have both conversation and other cases")
  return {
    "server-conversation": {
      names: conversation,
      tasks: [{ label: "conversation", args: [conversationFilter], names: conversation }],
    },
    "server-other": {
      names: other,
      tasks: [
        {
          label: "other",
          args: ["--skip", conversationFilter, "--skip", cataloguePrefix],
          names: other,
        },
      ],
    },
    remaining: {
      names: seeded,
      tasks: [{ label: "seeded catalogue", args: [cataloguePrefix], names: seeded }],
    },
  }
}

export function validateFilteredRegistry(output, expected) {
  if (JSON.stringify(testRegistry(output)) !== JSON.stringify(expected))
    throw new Error("libtest filter does not match assigned server cases")
}
