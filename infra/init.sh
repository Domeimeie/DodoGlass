#!/bin/sh
# One-shot setup of RustFS, run by the `init` service in compose.yml.
# Safe to run any number of times: existing buckets are kept, the event rule is set again.
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

for bucket in ingest final; do
    if aws s3api head-bucket --bucket "$bucket" > /dev/null 2>&1; then
        echo "bucket $bucket: exists"
    else
        aws s3api create-bucket --bucket "$bucket" > /dev/null
        echo "bucket $bucket: created"
    fi
done

# Replaces the whole notification config of the bucket with the file's content.
aws s3api put-bucket-notification-configuration --bucket ingest \
    --notification-configuration file:///init/ingest-notification.json
echo "event rule on ingest: set (.mp4 .mov .mkv -> MQTT topic dodoglass/ingest)"
