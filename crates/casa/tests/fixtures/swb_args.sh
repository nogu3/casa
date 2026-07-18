#!/bin/sh
# 受け取った引数を JSON 配列で echo するダミー swb。
# casa が組む引数列の検証用。テストの引数に JSON 特殊文字は含まれない前提。
out=""
for a in "$@"; do
  [ -n "$out" ] && out="$out, "
  out="$out\"$a\""
done
printf '{"args": [%s]}\n' "$out"
