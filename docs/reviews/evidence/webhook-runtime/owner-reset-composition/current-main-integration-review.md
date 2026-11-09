Technical review excerpt. Original full report SHA256: `55c093ff5571c119172efe4fa2ef778db73d5d66260d22e4d903f94809b9803a`.

# Issue896 — bounded current-main integration review

Disposition: **no findings in the mechanical integration boundaries**. Blocker: empty. Major: empty. Minor: empty. Nit: empty.

Reviewed frozen author HEAD: `3e7e965d6b5edbcf29ff6926d736d21090268bdb`, `/workspace/nessa-webhook-delivery-review`.
First parent: `25a6e33fe21d9e6d3deb093e310c9707a437021f`.
Second parent/current-main input: `2026c2becebec94ec69f849fa529143ad4625995`.
Upstream chain independently checked: `919079a7b13f3e4e8600cc7584b097641f8992c7` → `9d697f6e3cf1b5ec3b3b38b1b4c71955c5f33327` (#1125 GitHub row) → `2026c2becebec94ec69f849fa529143ad4625995` (#1122 chat headers/widget tabs).
Separately reviewed Rust precision-fix source: `69351af72cbd73179f7f8765424251ecf4a20b99`.

This is an affected integration review, **not a fourth whole-feature round, review-counter reset, or new review of the accepted upstream UI feature**. Earlier whole-feature findings/closure, CI fixture reviews and the separate precision review retain their original sources and limits. No source, configuration, shared artifact or branch was edited, and no Rust suite or browser was launched by this review.

## Exact preservation checks

- Independently checked the merge's exact two parents and the upstream first-parent chain above.
- Compared Git object identities: the **entire crates tree**, root **Cargo.toml** and **Cargo.lock** are byte-identical to69351af7. No Rust behavior or build dependency input was introduced by this integration. This is a source-equivalence check, not a new platform run or independent re-review of the precision correction.
- Extracted the complete `async webhook` step through the next step boundary: **byte-identical25a6e33f versus merged3e7e965d**, SHA256 `e43efe5a3afcc3597cac0e2d94223ca3f2b2ba952801ddefad82dd8b748cf8e5`. The same actor, configuration, current owner API, fixed-Head polling, no-attempt proof and cleanup remain.
- Both full strict-selection files are byte-identical to25a6e33f **and** the independently reviewed942e66bf helper correction:
  - `stress-conversation.mjs`: `ac9a6c959c767582c33bd26643c6239cb41f72d7f58a5c275084934ca2ecd3f2`.
  - `stress-conversation.test.mjs`: `f7055412b7d6cb175b890f241cca01c4550cb731a55b0915187fbce8ca0c8fdf`.
- The upstream change set is exactly129 paths, and the merge's first-parent changed-path set is **that same129-path set**. Of these,125 match upstream blob/deletion state exactly. For the other four, independently reconstructed a clean three-way merge from25a6e33f/919079a7/2026c2bec using temporary files and `git merge-file -p`; its output exactly matches the committed merge:
  - `.github/workflows/local-auth.yml`
  - `docs/codebase-structure.md`
  - `verification/desktop/scripts/conversation-stress.mjs`
  - `verification/desktop/scripts/lib/stress-plan.mjs`
- UI pin is exactly upstream `3118f2d8a15f68825ecef3fe0c337368b3d7f889`. I also checked the separate browser-preparation checkout is at3e7e965d and its actual vendor HEAD equals that pin; this is provenance, not browser qualification.

Machine-readable evidence: `/tmp/nessa-896-current-main-integration-review-checks.json`. The checks use Git objects and scratch temporary merge inputs, without altering the author worktree.

## Shared selector and UI boundary analysis

The inherited changes to `lib/selectors.mjs` concern the new panel/widget selectors, GitHub account controls and safe-area exemptions. The webhook/strict helper's actual selection contract remains:

- one click on `.workspace-list [data-session-row="<exact identity>"]`, with the unchanged15-second budget;
- one joint observation of exactly one `aria-selected="true"` row carrying the intended `data-session-row`, together with exactly one `.workspace-conversation .workspace-dock textarea`;
- nonzero width/height and visible computed visibility; duplicate composers, including hidden duplicates, throw;
- no repeated click, retry/replay, title-only fallback or remembered earlier selection.

I traced these selectors into the inherited source. `session-list.tsx` still emits exact `data-session-row`, derives `aria-selected` from the focused session and activates the selected identity through `useOpenFromRow`/`openSession`. The incoming keyboard navigation and New Session control do not replace that identity path. `Conversation` still emits `.workspace-conversation` and its `.workspace-dock`, and the existing desktop Composer still emits a textarea. Moving widgets into the chat's right panel does not replace that composer hierarchy. The right panel is its separate sibling aside with its own inert/hidden state. The pane grid's existing retired-pane retention was inspected; it was not introduced as a hidden selection workaround in this merge.

The helper therefore retains the source-level selector contract. This does **not** prove that the new rendered layout is unique, visible, focusable or unobstructed at runtime, or that no transient duplicate occurs. Those are exactly the conditions the unchanged strict helper and real browser qualification must still measure. No selection criterion was relaxed to accommodate the upstream layout.

## Workflow and inherited scope

The workflow change relative to25a6e33f is exactly the upstream nine-line GitHub WebKit stress step. The merged workflow retains the pre-existing Windows `RUST_TEST_NOCAPTURE` overlay; that is not a new integration edit or a claim of full byte identity to upstream. The main and first-parent deltas were inspected separately. No new own workflow command, timeout, dispatch, rerun or browser-download route was added. The new isolated GitHub step derives its zeroed knobs from the merged plan, including the webhook knob, rather than silently enabling that feature there.

The new UI/layout/model/widget/docs/workflow behavior was accepted upstream and preserved mechanically. I did not commission or perform a new whole UI feature review, nor modify inherited UI, source ownership or policy to make the webhook check pass. No actual baseline conflict or new ownership decision was identified within these integration boundaries.

## Executed checks and limits

- Independently ran `node --check verification/desktop/scripts/conversation-stress.mjs`: passed.
- Independently ran gateway-stack, stress-plan and strict-selection helper controls: **24/24 passed**, `/tmp/nessa-896-current-main-integration-review-helpers.log`.
- `git diff --check 25a6e33f..3e7e965d`: passed.
- All preservation/object/body/helper/three-way checks above passed. Author tracked source remained unchanged; only the pre-existing untracked tooling node_modules link is present.

The precision repair's295 tests, independent controls, Clippy and signed-out loopback CLI capture are separate source-qualified evidence supplied by its owner/reviewer, **not newly executed by this integration review**. No full Rust suite was repeated because its complete source/Cargo input tree is identical.

Full frontend preparation with the new UI pin is being run separately. I do not claim its result. Old0d CI/platform results describe their old UI/workflow/source input; they are not acceptance of this new merge. Real canonical browser, current default/raised stress/chaos and source-matched remote/platform CI remain independent gates, including the newly inherited GitHub WebKit step. No held rerun/download/dispatch, live provider/account/credential/recipient, public listener, network/security-setting change, publication or merge was performed here.
