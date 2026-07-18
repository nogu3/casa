#!/bin/sh
# enl 代役（常駐テスト用）。初回起動のみ INF 通知を 1 件出し、
# 2 回目以降（casad の再 spawn）は長時間ブロックする。casad kill 後も
# 残らないよう sleep は 60 秒で自然終了させる。
marker="${CASAD_TEST_DIR:?}/emitted"
if [ -e "$marker" ]; then
  sleep 60
  exit 0
fi
touch "$marker"
echo "{\"events\":[{\"ip\":\"192.0.2.10\",\"tid\":\"00ab\",\"seoj\":\"013001\",\"deoj\":\"05ff01\",\"esv\":\"Inf\",\"properties\":[{\"epc\":\"80\",\"pdc\":1,\"edt_hex\":\"30\"}]}]}"
exit 0
