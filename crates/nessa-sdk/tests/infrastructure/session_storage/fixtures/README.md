# Storage input fixtures

`retired-opening.bin` is a fixed **handcrafted** retired-shape opening frame,
not bytes captured from a historical producer. The public record test creates
its stream with the actual current producer, substitutes this row through
SQLite, checks repeated typed corruption without mutation, then restores and
accepts the original current data. Exact historical-emission provenance remains
OPEN. No compatibility reader or fixture encoder is added.
