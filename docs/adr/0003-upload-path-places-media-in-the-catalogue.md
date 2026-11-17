# The upload path places a Media file in the catalogue

Where a video is uploaded in `ingest` decides what it is, so uploading needs no extra form or API:

```
movies/<Name> (<Year>)/<label>.<ext>
shows/<Name> (<Year>)/Season <n>/S<nn>E<nn>[ anything].<ext>
shows/<Name> (<Year>)/Specials/S00E<nn>[ anything].<ext>
```

The year is required: without it, same-named Movies and Shows (*Dune* 1984 / 2021) cannot be told
apart and metadata matching guesses. An upload that does not fit (no year, other folder, season in the
file name disagreeing with the folder) becomes an Unassigned media file rather than being rejected, so
nothing uploaded is lost. This layout is a contract with whoever uploads; changing it later means
re-placing existing uploads.
