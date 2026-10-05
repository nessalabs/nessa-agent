---
id: "desktop-workspace-trace-verified-pull-request-contributions"
title: "trace verified pull-request contributions"
kind: "contract"
status: "implemented"
summary: "These are Git-history-verified PRs, not issue numbers inferred from feature comments."
parent: "desktop-workspace"
sources:
  - "src/desktop/dependencies.ts"
  - "src-tauri/src/desktop_window.rs"
diagramLinks: {}
---

# trace verified pull-request contributions

These are Git-history-verified PRs, not issue numbers inferred from feature comments. Merge entries identify the PR; first-parent merge diffs confirm the scope. Squash subjects identify #353 and #374 as their final parenthetical numbers; their first references, #327 and #360, are issue numbers. A broad PR contribution does not mean every later line originated there.

| Verified PR | Git evidence | Contribution relevant to these flows |
| --- | --- | --- |
| [#251](https://github.com/nessalabs/nessa-agent/pull/251) | `132e2b72`, `Merge pull request #251 … desktop-app`; merge diff includes desktop entry, composition, settings/workspace and `desktop_window.rs` | Main desktop surface and workspace frontend |
| [#279](https://github.com/nessalabs/nessa-agent/pull/279) | `961dc78d`, `Merge pull request #279 … 253-split-panes`; merge diff includes split-pane model, adapters, UI and workspace integration | Reusable split-pane ownership and dragging/resizing |
| [#284](https://github.com/nessalabs/nessa-agent/pull/284) | `868bbd57`, `Merge pull request #284 … 283-overview-header` | Overview header work; inspect merge diff for exact row/layout change |
| [#288](https://github.com/nessalabs/nessa-agent/pull/288) | `f10d0c22`, `Merge pull request #288 … list-inset-alignment` | Session-list inset alignment |
| [#353](https://github.com/nessalabs/nessa-agent/pull/353) | `5a877ce8`, `feat(desktop): a pane holds a session or a widget (#327) (#353)` | Session/widget pane identity and host integration |
| [#364](https://github.com/nessalabs/nessa-agent/pull/364) | `c3151c07`, `Merge pull request #364 … 328-widget-hosts`; merge diff includes widget registry, hosts, Escape integration and tests | Inline/pane/window hosts, accessories, rendering/exit regression fixes |
| [#363](https://github.com/nessalabs/nessa-agent/pull/363) | `cd092666`, `Merge pull request #363 … 346-gateway-mcp-client` | MCP connection/tool UI plumbing; detailed transport flow in [extensions UI](../../extensions/ui/README.md) |
| [#374](https://github.com/nessalabs/nessa-agent/pull/374) | `52bc6cbc`, `feat(desktop): widget host size from nessa_ui's shared size observer (#360) (#374)` | Shared size observation for widget host context |

Read-only verification commands used were `git log --merges --format='%h %s'`, scoped `git log`, and `git diff-tree --no-commit-id --name-only -r <merge>^1 <merge>` for #251/#279/#284/#288/#364. No PR publication, branch creation, commits, or source changes were made for this map. The linked ADR numbers 238/253/326 and feature references #327/#328/#360 are decision/issue identifiers; they are not substituted for verified PR identities.

## Further reading

[Source](../../../../../src/desktop/dependencies.ts) · [Related source](../../../../../src-tauri/src/desktop_window.rs)
