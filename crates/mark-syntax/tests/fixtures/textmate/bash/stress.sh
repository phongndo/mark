#!/usr/bin/env bash
set -euo pipefail

# Bash stress fixture with non-ASCII text: café λ🚀.
name="${1:-café}"
json_payload="$(cat <<'JSON'
{
  "message": "hello λ🚀",
  "regex": "^/api/[[:alpha:]]+$"
}
JSON
)"

cat <<EOF
user=${name}
now=$(date +%s)
payload=$(printf '%s' "$json_payload" | sed 's/"/\\"/g')
EOF

result=$((42 / 2))
echo "result=${result} upper=$(printf '%s' "$name" | tr '[:lower:]' '[:upper:]')"
