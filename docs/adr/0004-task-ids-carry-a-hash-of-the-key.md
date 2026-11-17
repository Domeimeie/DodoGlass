# Task ids carry a hash of the full upload key

A task id is a readable prefix plus the first 8 hex digits of SHA-256 over the full upload key,
normalised to Unicode NFC: `movies/The Matrix (1999)/matrix.mkv` → `The_Matrix__1999__matrix-3f9a2c1b`.
The previous id (the key with every other character replaced by `_` and the extension dropped) made
`matrix.mkv` and `matrix.mp4`, or `Amélie` and `Am lie`, the same video, so one upload silently
replaced another's output. The readable part keeps `final/` browsable. The function lives in a crate
shared by the transcoder and the asset server, because both must derive the same id from a key. The
change moved every `final/` path; existing outputs were deleted and re-uploaded, not migrated.
