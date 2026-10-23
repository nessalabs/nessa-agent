# Original-issuance correction review, round 1

This is the fresh review of candidate `ee0d8eff159d96eeb70161f1c3ad774ae16da558`, not the previously blocked report. [Report](report.md) records one Minor finding M1; no Blocker/Major/Nit. The review is not clean. The exact failed recovery expectation and independent state-preservation controls are retained. Scripts and argument files are local reproduction evidence; paths describe the isolated review environment. No historical blocked report was accessed.

M1 disposition: fix the matching webhook receipt at the actual issuance owner before command-based transition replay. The owner uses the same original-fact decoder as AccessReader and returns typed OriginRecoveryRequired for an original AdminIssue. Registration forwards that result while retaining intent, without installing or rewriting the original history. Product receipt replay remains unchanged. The canonical regression now covers five accepted historical shapes and repeated registration, with byte preservation and retained-intent checks. Fresh independent round 2 was required; this disposition alone did not close M1. The subsequent clean report is linked below.

Copied text logs remove trailing whitespace and terminal blank lines for repository whitespace checks. Original execution logs remain unchanged under `/tmp`; statuses, assertions and diagnostics are preserved. The review report itself is unchanged.

Fresh [round 2](../review-2/report.md) is clean at `8346c4ae9b69a6127041e4e07d2156143d0b2732`, source/test `1c1355b537037e7416486856211b74c605804fa7`: all four severity buckets empty and M1 independently verified fixed. Full #896 integration and qualification remain pending.
