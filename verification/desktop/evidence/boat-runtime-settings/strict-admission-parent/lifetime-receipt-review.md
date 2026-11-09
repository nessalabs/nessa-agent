# Finite lifetime receipt review

Source: `ee77f0d9c1ccdfb89e9d1465a87336ef8aeea964` on parent `398d5b084310711453f3ad6bf2a899b5b9e2ea17`.

The first scoped review found that a finite lifetime request was not reconciled with the provider GET receipt before workspace SDK authority. The correction requires a positive provider `ttlSeconds` no longer than the selected finite limit on create and archived resume. An active VM is not compared with a newly edited setting. Typed refusal retains the actual VM target and survives assignment-history reopen.

The second review found that zero was accepted; the source and hostile HTTP test were corrected. A final fresh scoped review found no remaining actionable issue. The reviewer inspected the create, archived resume, active reopen, durable refusal and cleanup paths. The result is scoped to this source change, not a claim that CI or the provider workflow passed.

Local checks: workspace suite 54 passed; strict all-target server Clippy passed; intentional removal of the finite cap failed the hostile receipt test (exit 101), and the restored test passed. Final provider evidence is recorded separately.
