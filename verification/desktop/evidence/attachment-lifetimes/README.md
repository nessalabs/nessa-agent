# Terminal attachment refusal cleanup — issue 816

Runtime classification: `c1100ce71`; browser fixture/runner: `8df0b57efbfb6b59250c31203eb5f2ab6609b025`.
The later #863 parent merge changes no attachment App, query, store, or fixture/runner code.

`attachments-races.mjs` passed 26/26 in Chromium and WebKit, with zero page errors.
Its AL24 subset passed 4/4: preserved-upload and reupload refusal, in both engines.
Each case retained two native 40 MiB originals, while the fake gateway stored 68 B
processed images. After the first refused draft was retried and accepted, then a
second image was accepted, the 64 MiB preview cap released the first resource and
kept the second. The old blob URL disappeared from the DOM. The reupload case uses
the existing “No longer kept” reference fallback for the old gateway image.

The asserted source revert removed `failed` from the settled receipt classification.
The Chromium regression then failed on retained resources: actual `[true, true]`,
expected `[false, true]`. The source was restored byte-for-byte and all four positive
cases passed again. Raw local records: `/tmp/816-al24-attachments-races-full.log`,
`/tmp/816-al24-source-revert.stdout`, `/tmp/816-al24-source-revert.stderr`.

The screenshots show the resulting App history, including the local preview that
remains and the honest reference fallback. These checks measure retention and
rendered results; they make no performance claim.
