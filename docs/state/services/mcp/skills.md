---
id: "mcp-skills"
title: "Skills over MCP (SEP-2640)"
kind: "statechart"
status: "implemented"
summary: "MCP servers can publish Agent Skills alongside their tools, and
  clients verify every skill file against its retained manifest."
parent: "mcp"
sourceRevision: "aee2b45f28c3be8f0dc30c41dadf2ce39b3dc09c"
sources:
  - "crates/nessa-sdk/src/domain/mcp_skills/mod.rs"
  - "crates/nessa-sdk/src/infrastructure/mcp/wire.rs"
  - "crates/nessa-sdk/src/infrastructure/mcp/servers.rs"
  - "crates/nessa-sdk/src/infrastructure/mcp/stand_in.rs"
  - "crates/nessa-conversation/src/mcp_servers/infrastructure/built_in.rs"
  - "crates/nessa-gateway/src/cli/infrastructure/mcp.rs"
diagramLinks: {}
model:
  spec: tla/services/mcp/Skills.tla
  config: tla/services/mcp/Skills.cfg
modelCheck:
  sourceRevision: "aee2b45f28c3be8f0dc30c41dadf2ce39b3dc09c"
  checker: tlc-1.7.4
  date: 2026-11-25T05:44:51.191Z
  config: tla/services/mcp/Skills.cfg
  modelDigest: 9d4497981fbe20b0cbe879f994518a5d2d815db7deda6e70be4ed4425e10fa5c
  states: 35
  held:
    - TypeOK
    - NoRetainedEntryWithoutCapability
    - NoDirReadWithoutCapability
    - LoadedMatchesRetainedAndApproved
    - NeverDeliveredUnverified
  broke: []
  unchecked: []
---
# Skills over MCP (SEP-2640)

An MCP server can ship Agent Skills (`SKILL.md` and optional supporting files) over the same connection it uses for tools by declaring `capabilities.resources` and `capabilities.extensions["io.modelcontextprotocol/skills"]`.

Discovery is lazy and manifest-bound. Calling `skills/list` or `skills/get` returns each skill's URI, verbatim YAML frontmatter, and a complete `{ uri, digest, size }` manifest (or `"dynamic"`). No skill file is fetched ahead of need. Before any file read via `resources/read` is accepted, the client checks that the URI belongs to the retained manifest, matches the declared byte size and SHA-256 digest, and (for `SKILL.md`) has YAML frontmatter identical field-by-field to the retained entry.

If the server updates a skill after discovery, reads against the previous manifest fail verification and deliver nothing. Recovering requires refreshing the entry with `skills/get` or `skills/list`, which invalidates any prior manifest-bound approval and loaded state until the new manifest is approved. Optional `resources/directory/read` is gated on `directoryRead: true` and never extends a retained manifest.

```mermaid
stateDiagram-v2
    [*] --> Negotiated: initialize accepted [resources and io.modelcontextprotocol/skills]
    [*] --> Unsupported: initialize accepted [skills extension absent]
    Negotiated --> Retained: skills/list or skills/get [valid entry within 512 files and 16 MiB]
    Retained --> Approved: Approve manifest [bound to server and resource digests]
    Approved --> Loaded: resources/read [URI in manifest, size, SHA-256, and frontmatter verified]
    Approved --> Approved: resources/read [unlisted URI, digest, size, or frontmatter mismatch / reject]
    Loaded --> Loaded: resources/read supporting file [URI in manifest, size and SHA-256 verified]
    Loaded --> Retained: skills/list or skills/get [manifest revision changed / clear approval and loaded state]
    Approved --> Retained: skills/list or skills/get [manifest revision changed / clear approval]
    Retained --> Retained: resources/directory/read [directoryRead true / manifest unchanged]
```

## Further reading

[SEP-2640](https://modelcontextprotocol.io/seps/2640-skills-extension) · [MCP sessions](sessions.md)

## Model coverage

Direct model: Capability gating (`io.modelcontextprotocol/skills` and `directoryRead`), manifest-bound entry retention, approval invalidation on manifest refresh, verified file delivery, and non-extension of retained manifests during directory reads.

The finite model checks safety only across two server revisions. Multi-page pagination cursors, per-file byte limits, YAML parsing details, and Cedar policy storage are verified by Rust domain and integration tests rather than this model. Terminal states may stutter; no liveness or implementation proof is claimed.
