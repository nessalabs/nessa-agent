# Turn outcomes evidence

The two `terminals-*` Chromium/columns screenshots preserve the original
terminal-state captures introduced by #841 at
`3a1346cdc447b134a8c2bfa3530b31037cfd00e7`. They are renamed byte-for-byte from
current main, rather than replaced with questionnaire crops.

The two `cleared-*` Chromium/columns screenshots are the compact question-card
captures taken on #863 source `d8ad963aff4bfaeb6eb0288c1ce85fe184b01357`.
They do not claim to be captures of the later main integration.

At current #863 runtime source `43049bc1fdde2d49bcbb6c79f657df0a766ef5ec`,
the full turn-outcomes replay passed **12/12** in Chromium/WebKit across
columns/sidebar, including terminal text, question display, answer/skip and
later-frame removal. Current native captures are retained locally under
`/tmp/nessa-863-main850-shots`; the four branch captures above remain explicitly
attributed historical visual evidence.

The script names each file by check, surface, engine and layout. A source-negative
probe restoring the previous names passed the four browser scenarios but produced
only four files, failing the eight-distinct-output assertion; exact source
restoration produced eight nonempty files again. That probe was run at `d0ea3036`
before main #850, and is not attributed to the later integration.
