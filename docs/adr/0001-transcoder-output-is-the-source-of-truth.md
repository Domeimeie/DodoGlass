# The transcoder's output marker decides when a Media file is Ready

The asset server treats a Media file as Ready only when `final/<id>/source.json` exists, and
reconciles against the `final` bucket at startup and periodically. Status messages on
`dodoglass/status` are only a hint to react quickly: they are published best-effort (`try_publish`,
dropped when the queue is full) and are missed while the asset server is down, so relying on them
alone would leave finished videos stuck as "processing".

**Note (ADR 0006):** the trigger is now the marker's own event: RustFS publishes every new
`source.json` in `final` on `dodoglass/final`, and the asset server places the Media file from it
as Ready. The status messages are no longer read, and the periodic reconcile stays as the safety
net. The marker still decides Ready.
