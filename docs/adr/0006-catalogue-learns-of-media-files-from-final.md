# The catalogue learns of a Media file only when its transcode is complete

The asset server no longer listens to upload events on `dodoglass/ingest` or to the transcoder's
status messages. It learns of a Media file from RustFS itself: an event rule on the `final` bucket
publishes every new `source.json` (the output marker of ADR 0001) to `dodoglass/final`, delivered at
least once (QoS 1), and the periodic reconcile stays as the safety net. The catalogue therefore holds
only finished output: no Processing or Failed Media files and no 24-hour rule. What is still
transcoding or has failed is visible in the transcoder's status messages and as inputs left in
`ingest`, not in the catalogue. This supersedes the spec's (#1) ingest subscription and status hints.

## Considered Options

- **Keep both subscriptions** (the original design): shows Processing and Failed in the catalogue, at
  the cost of two inputs, a third state machine branch and the 24-hour rule. Not needed: nobody
  manages transcodes through the catalogue.
- **Trigger on the transcoder's `completed` status**: best-effort (`try_publish`, dropped when the
  queue is full), so a lost message would hide a finished video until the next reconcile.

## Consequences

- `final` gets an event rule after all. It used to have none so that the transcoder's output would not
  trigger new transcodes; that cannot happen with a separate topic only the asset server subscribes to.
- Uploads with a path that doesn't fit the layout appear as Unassigned media files only after their
  transcode.
- A re-upload to the same path deletes the old marker before the new transcode starts, so for that
  time the Media file is Missing. This must not change its Label or move the Default media file away.
