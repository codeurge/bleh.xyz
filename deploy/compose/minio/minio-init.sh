#!/bin/sh
# Creates the local archive buckets in MinIO (synthetic mode only): a public
# bucket with anonymous read, standing in for the R2 bucket behind
# data.bleh.xyz, and a private bucket for unfiltered raw.
set -eu
endpoint=http://minio:9000
s3() { aws --endpoint-url "$endpoint" "$@"; }

i=0
until s3 s3api list-buckets >/dev/null 2>&1; do
  i=$((i + 1))
  [ "$i" -gt 60 ] && { echo "minio-init: MinIO not reachable" >&2; exit 1; }
  sleep 2
done

for b in "$PUBLIC_BUCKET" "$PRIVATE_BUCKET"; do
  s3 s3api head-bucket --bucket "$b" >/dev/null 2>&1 || s3 s3api create-bucket --bucket "$b"
done

s3 s3api put-bucket-policy --bucket "$PUBLIC_BUCKET" --policy "$(cat <<EOF
{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":["*"]},
"Action":["s3:GetObject","s3:ListBucket"],"Resource":["arn:aws:s3:::$PUBLIC_BUCKET","arn:aws:s3:::$PUBLIC_BUCKET/*"]}]}
EOF
)"
echo "minio-init: buckets ready: $PUBLIC_BUCKET (public read), $PRIVATE_BUCKET (private)"
