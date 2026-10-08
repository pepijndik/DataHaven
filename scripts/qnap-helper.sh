#!/bin/sh
set -eu

command_name="${1:-}"
root="${2:-}"

if [ "$command_name" != "manifest" ] || [ -z "$root" ]; then
  printf '%s\n' "usage: qnap-helper.sh manifest /share/Backups/LightroomStation"
  exit 2
fi

if [ ! -d "$root" ]; then
  printf '%s\n' "root does not exist: $root" >&2
  exit 1
fi

hash_tool=""

if command -v sha256sum >/dev/null 2>&1; then
  hash_tool="sha256sum"
elif command -v shasum >/dev/null 2>&1; then
  hash_tool="shasum -a 256"
fi

cd "$root"

find . -type f -print | while IFS= read -r file_path; do
  size="$(wc -c < "$file_path" | tr -d ' ')"
  modified="$(date -r "$file_path" +%s 2>/dev/null || stat -c %Y "$file_path")"

  if [ -n "$hash_tool" ]; then
    hash="$($hash_tool "$file_path" | awk '{print $1}')"
  else
    hash=""
  fi

  escaped_path="$(printf '%s' "$file_path" | sed 's/\\/\\\\/g; s/"/\\"/g')"
  printf '{"path":"%s","size":%s,"modified":%s,"sha256":"%s"}\n' "$escaped_path" "$size" "$modified" "$hash"
done
