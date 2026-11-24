#!/bin/sh
# One-shot setup of RustFS, run by the `init` service in compose.yml.
# Safe to run any number of times: existing buckets are kept, the event rules are set again.
set -eu

echo "waiting for RustFS at $AWS_ENDPOINT_URL ..."
tries=0
until aws s3api list-buckets > /dev/null 2>&1; do
    tries=$((tries + 1))
    if [ "$tries" -ge 60 ]; then
        echo "RustFS did not answer within 60 s" >&2
        exit 1
    fi
    sleep 1
done

for bucket in ingest final assets; do
    if aws s3api head-bucket --bucket "$bucket" > /dev/null 2>&1; then
        echo "bucket $bucket: exists"
    else
        aws s3api create-bucket --bucket "$bucket" > /dev/null
        echo "bucket $bucket: created"
    fi
done

# The upload layout's top folders (ADR 0003), as empty marker objects so they stay visible in the
# web console even when no upload is waiting. Not videos, so the event rule ignores them.
for folder in movies shows; do
    if aws s3api head-object --bucket ingest --key "$folder/" > /dev/null 2>&1; then
        echo "folder ingest/$folder/: exists"
    else
        aws s3api put-object --bucket ingest --key "$folder/" > /dev/null
        echo "folder ingest/$folder/: created"
    fi
done

# Each call replaces the whole notification config of the bucket with the file's content.
aws s3api put-bucket-notification-configuration --bucket ingest \
    --notification-configuration file:///init/ingest-notification.json
echo "event rule on ingest: set (.mp4 .mov .mkv -> MQTT topic dodoglass/ingest)"

# New output markers only (ADR 0006): the asset server places a Media file once its transcode is
# complete. A separate topic, which the transcoders do not subscribe to, so output never triggers
# a transcode.
aws s3api put-bucket-notification-configuration --bucket final \
    --notification-configuration file:///init/final-notification.json
echo "event rule on final: set (source.json -> MQTT topic dodoglass/final)"
