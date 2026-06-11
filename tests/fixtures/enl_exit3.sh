#!/bin/sh
# enl のタイムアウト（exit code 3）を模倣するダミー。
echo '{"error": {"kind": "timeout", "detail": "device did not respond"}}' >&2
exit 3
