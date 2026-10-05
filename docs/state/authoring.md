---
id: "authoring"
title: "Authoring and maintaining statecharts"
kind: "guide"
status: "reference"
summary: "Authoring and maintaining statecharts"
parent: "nessa"
sources:
  - "CODING_STANDARDS.md"
diagramLinks: {}
---

# Authoring and maintaining statecharts

## Statechart principles

[Harel's 1987 paper](https://www.state-machine.com/doc/Harel87.pdf) uses nested
states to refine behavior, concurrent regions to describe activity that coexists,
and events to coordinate it. Shared outer transitions can replace repeated
inner transitions. History distinguishes a default entry from a return to a
previous configuration; it does not establish storage durability.

In Nessa, apply these ideas to the actual owner rather than inventing one giant
application state. Keep attachment, admission, execution, provider cleanup,
commit acknowledgement and transport connectivity separately identifiable.
A component diagram organizes services; it does not imply that only one service
exists at a time. Mermaid is our rendering notation, not a complete implementation
of the paper's semantics.

## Repository document contract

The website lives outside this repository. It reads these files through a
configured repository root. Markdown remains reviewable and useful without the
website. A document has YAML frontmatter followed by normal Markdown:

```yaml
---
id: sdk-provider
title: Provider attachment
kind: statechart
status: implemented
summary: Provider attachment and its cleanup ownership
parent: sdk
sources:
  - crates/nessa-sdk/src/application/agent_execution/agents/lifecycle.rs
diagramLinks: {}
---
```

IDs are stable lowercase
kebab-case values. `parent` is another document ID or null for a system root.
Kinds are `system`, `service`, `feature`, `flow`, `statechart`, `operation`,
`contract`, and `guide`. Status is `implemented`, `mixed`, `fixture`, `proposed`,
or `reference`; source evidence and verification exclusions explain that label.
The loader rejects duplicate IDs, missing parents, cycles and unresolved diagram
links. Each `sources` entry is a repository-relative path. `diagramLinks` maps
Mermaid node IDs to document IDs for drill-down; ordinary Markdown links also
work. Folder paths organize the files, while parent metadata owns the browsing
hierarchy. Do not add another navigation manifest.

## Diagram explanations

Keep state and guard explanations in the owning page's Markdown. The optional
`diagramDetails` frontmatter list connects a diagram element to one of those
sections. It does not repeat the explanation:

```yaml
diagramDetails:
  - diagram: 0
    node: Admission
    section: Call admission
  - diagram: 0
    edge:
      from: Admission
      to: Refused
      label: Origin, scope, visibility or bounds rejected
    section: Admission refusal rules
```

Use `diagramDetails` with `stateDiagram-v2` or a component `flowchart`/`graph`.
Component maps still use `diagramLinks` for navigation. Sequence diagrams explain
order in Details; their messages do not receive inspector bindings. `diagram` is the zero-based order of the fenced
Mermaid block in that page.
An entry selects either a `node` or an `edge`. The node IDs, edge endpoints and
edge label match the chart source; `section` is the exact plain heading text
read by the loader, unique within the page. Headings inside a quote or list do
not define an inspector section. The section includes its nested headings
and ends at the next heading of the same or higher level. Distinct labels can
identify different transitions between the same two states.

For a component map, select an ordinary node or an edge with a plain label.
Subgraph groups and edges to those groups cannot carry inspector details. Two
edges with the same endpoints and label are ambiguous. Distinct Mermaid element
identities must also be unique. Health reports those cases instead of guessing
which element should open an explanation.

Use plain transition labels written as-is. Labels containing Mermaid entity
codes such as `#quot;`, HTML markup, Markdown strings or a literal `|` cannot receive detail bindings; the
browser reports the unsupported label in Settings → Health. A Mermaid block
can contain at most 100,000 UTF-16 code units as written, including frontmatter,
directives and comments. The browser and loader apply this same bound. Split a
larger model into diagrams owned by the relevant services or lifecycles.

A detail section may be the reader's first encounter with this part of Nessa.
Start with two short paragraphs in plain English: what is happening, what
allows or prevents the next step, and what happens afterward. Explain chart
notation in words where it is used. For example, `[time >= issuance]` means
“the requested time is no earlier than the credential's creation.”

Use a short sequence when it helps show who passes work to whom and in what
order. Include the important refusal or uncertain outcome, but leave the full
call path in source. Do not add a sequence to every page or turn every function
into a diagram node. States belong in statecharts; parts that coexist belong in
component maps.

Keep two or three useful source or evidence links in Further reading. The full
source list stays in frontmatter. Do not move walls of function names and test
names into Technical notes. Compact and Standard provide short explanations;
Full displays the authored section, including its useful examples. The complete
page and its sequences are available in Details.

Use short sentences and ordinary words. Avoid em dashes, filler and slogans.
Preserve important conditions and uncertainty in the opening explanation.
Several selectors can point to one section when they explain the same decision.
A missing explanation is a documentation gap, rather than permission to infer
behavior from a label.

In the browser, a `+` marks an available explanation or a deeper document.
`diagramLinks` continues to own navigation between documents. A detail section
opens in the right inspector; a node with both kinds of depth can offer the
related document from that inspector. The complete page remains available in
Details. Diagram detail preferences change presentation: Full displays the
authored explanation, and does not launch an agent or change repository files.
If an explanation contains a Mermaid block, the inspector links to that
block in Details rather than rendering a second interactive copy.
The compact view must preserve access to the transition's meaning even when
its label is visually hidden.

## Reading statechart notation

| Notation | Meaning to establish on the owning page |
| --- | --- |
| State box | The named configuration of one entity or operation, with its owner and identity. |
| Nested state | A refinement of the enclosing state; explain shared transitions and entry behavior. |
| Concurrent regions | Configurations that can coexist; name synchronization and cross-region guards. |
| `event [guard] / action` | The trigger, condition required for this transition, and resulting action. A rejected guard needs a defined alternative outcome. |
| Initial or final marker | Entry to or completion of the modeled operation. Completion alone does not prove cleanup or erasure. |
| `H` / `H*` in Harel's notation | Shallow history restores the previous immediate substate; deep history restores the previous nested configuration. State what is remembered, by whom, for how long, and the default when no history exists. |

The history notation explains the paper; it does not assert that a Mermaid
renderer supports those symbols or that Nessa implements such a state. Use an
explicitly named restoration operation when that better describes the actual
code. A stored record, a browser tab and a running provider session have
different lifetimes and cannot substitute for one another.

## What to put on a page

Start with what the page follows and what the reader can expect. Explain the
important states, triggers, pass/fail conditions and uncertainty. Link the source
for exact types and detailed checks, rather than retyping them. Put diagrams near the behavior they explain. A pure value
contract or a component inventory does not need an invented lifecycle.
Keep one complete user-goal index per feature.
Use `event [guard] / action` on arrows when all three are meaningful. A final
marker ends the modeled operation; it does not assert a resource was deleted.

For independent facts, use a small chart per owner or concurrent regions with
`--` inside a composite state. Explain synchronization and cross-region guards.
Do not draw a successful receipt as proof of provider completion, or an IO error
as proof that publication never happened. Keep audit acknowledgement distinct
from physical effect.

For history, state exactly what survives: in-memory tabs, browser references,
committed records, checkpoints, or provider context. Reconnect does not imply
command replay; restored records do not imply re-execution. Do not add an `H`
node that promises unsupported resume semantics.

## Workflow as the code changes

1. Locate the existing owner and page using the service hierarchy and source links.
2. Re-read that owner's code and relevant regression cases before changing its chart.
3. Follow the repository's [design-order and review requirements](../../CODING_STANDARDS.md#gates) in their canonical document. Tables in existing design records remain evidence; this atlas does not retype those requirements.
4. Change the owning Markdown, then validate metadata, links, and Mermaid rendering with the standalone browser's checks.
5. Update gap/risk dispositions when evidence changes. Keep proposals separate from implemented behavior; retain historical reproduction context.

Generated image previews are optional output, never a second editable chart.
The browser renders SVG directly from fenced Mermaid in Markdown. This atlas is
for documentation; it does not dispatch tools, derive production guards from
English, or replace Rust/TypeScript domain code. Executable state definitions
would be a separate explicit design decision with an owner and regression proof.
