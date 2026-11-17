# The transcoder's output marker decides when a Media file is Ready

The asset server treats a Media file as Ready only when `final/<id>/source.json` exists, and
reconciles against the `final` bucket at startup and periodically. Status messages on
`dodoglass/status` are only a hint to react quickly: they are published best-effort (`try_publish`,
dropped when the queue is full) and are missed while the asset server is down, so relying on them
alone would leave finished videos stuck as "processing".
