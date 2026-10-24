#!/usr/bin/env bash
# Start LES with an isolated database before running this smoke test.
set -euo pipefail
export AWS_ACCESS_KEY_ID=test AWS_SECRET_ACCESS_KEY=test AWS_DEFAULT_REGION=us-east-1
export AWS_EC2_METADATA_DISABLED=true AWS_PAGER=''
endpoint="${LES_ENDPOINT_URL:-http://127.0.0.1:9325}"
aws --endpoint-url "$endpoint" ses verify-email-identity --email-address sender@example.com
aws --endpoint-url "$endpoint" ses send-email \
  --from sender@example.com \
  --destination '{"ToAddresses":["recipient@example.net"]}' \
  --message '{"Subject":{"Data":"Query test","Charset":"UTF-8"},"Body":{"Text":{"Data":"Hello","Charset":"UTF-8"}}}'
aws --endpoint-url "$endpoint" sesv2 send-email \
  --from-email-address sender@example.com \
  --destination '{"ToAddresses":["recipient@example.net"]}' \
  --content '{"Simple":{"Subject":{"Data":"REST test"},"Body":{"Text":{"Data":"Hello"}}}}'
aws --endpoint-url "$endpoint" sesv2 get-email-identity --email-identity sender@example.com
aws --endpoint-url "$endpoint" ses list-identities
