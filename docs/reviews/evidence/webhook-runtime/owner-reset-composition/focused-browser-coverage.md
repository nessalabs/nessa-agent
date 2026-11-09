# Focused webhook browser coverage

Coverage source `bd760f6610abcf9186cc8bf4c57641f427d5942a` adds one supplemental
step to the existing prepared conversation-stress job. Integration
`108ce259cdc7b2628a19329181886de21cd88d3c` preserves main
`aef11ea3b3d0c96f8c96fd9509f5657c58693780`, including its running-tool trace
changes. The complete workflow remains byte-identical to the reviewed addition.

The new step reuses the existing pinned browsers and gateway build. It runs seed1
with WebKit/webhooks1, then Chromium and WebKit/webhooks3 per engine. Other knobs
are explicitly zero only in these focused invocations. In particular linking and
pool counts are zero, so their native-listener enable paths are not selected;
each fresh fixture starts native-disabled. These focused runs do not replace or
claim to be the existing complete default or a full raised cohort. Full-default,
menu/GitHub WebKit and chaos steps, permissions, conditions, aggregate, evidence
retention and all existing budgets remain unchanged. The new step has a10-minute
bound inside the unchanged60-minute job.

[Independent affected review](focused-ci-review.md) found no findings:24 controls,
actual CLI/plan dry checks and all-workflow actionlint1.7.12 passed. The
[integration closure](focused-ci-integration-closure.md) independently proves
that the complete product tree differs from CI's actual checkout only by the
same11 workflow additions. These source checks do not execute the focused cases.
The CI-only addition changes no domain state or transition, so existing state
diagrams and the69351af domain source checkpoint remain applicable.

## Executed predecessor evidence

Published head4914f5e7's run40257115915 checked out synthetic merge
`aa316c59dd4d69b48685b922e9fc0a055e97961a`, merging that head into main
`aef11ea3b3d0c96f8c96fd9509f5657c58693780`. This was verified from the actual
job checkout log and fetched Git object, rather than inferring main from PR metadata.

The successful [stress log](ci-491-browser-success.txt) records the actual
Chromium webhook witness: fixed Head46,46 queued and0 attempted, followed by
step036 passing and full default50/50 checks held. WebKit menu and GitHub focused
invocations each held4/4 total checks, and chaos held2/2 runs. Those WebKit cases
do not execute webhook capture. The new webhook WebKit1 and both-engine3 cases
remain unexecuted until a new published head runs them.

The integrated complete frontend check passed5306/5306 tests across409 files and
built in15.74s; see the [source-pinned summary](main-aef-frontend-summary.txt).
The run's frontend job independently passed the same5306 tests ataa316, with
a23.13s build. [Qualification receipt](focused-ci-qualification.json) distinguishes
source/dry-plan checks from actual focused execution. No successful browser result
is inferred for an unexecuted cohort, and positive TLS recipient evidence remains
the separate synthetic loopback product suite.
