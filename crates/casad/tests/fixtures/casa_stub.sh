#!/bin/sh
# casa バイナリの代役。casad が渡した引数を stdout に記録し、
# CASA_FAKE_EXIT（既定 0）の exit code で終了する。
echo "casa called: $@"
exit "${CASA_FAKE_EXIT:-0}"
