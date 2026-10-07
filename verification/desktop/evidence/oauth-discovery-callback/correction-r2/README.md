# PR645 correction and issue647 verification evidence

Final candidate: `09e2c8238199f03e04082ba1032495de5c94a5f8`; base: `e03e94c50ae025f9922a2f1d201ac0f63309ebcf`.

Rust checks were executed on `f55c460ee1fb09bd6ceec5e1844c45ae6c049eff`. All Rust source, tests and ADR392 are byte-identical at the final candidate. Browser and restored Node verification cover the assembled final candidate. `handoff.json` records exact proof provenance, deliberate mutations and failure dispositions.

The original Windows failure at prior b8 remains a failed run; its anonymous wait did not identify which stage timed out. The correction observes successful completion of the actual listener supervisor while token exchange is gated, with a deliberate delayed-receiver-drop mutation failing that assertion. The original f55 browser run remains failed and is preserved separately. Issue647 corrects its source-backed late-message classification race; the final both-engine browser run passes. Initial Node assertion-shape development failure is also retained and is not a pass.

Independent R2 review and corrected-head supported-platform CI are pending when this evidence is first published.
