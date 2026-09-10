#!/bin/sh
# mat 代役（デバイスイベント重複配達テスト用）。起動 argv を記録し、同一
# event_number（100）のデバイスイベント行を 2 回出してからブロックし続ける
# （matd の再配達を模す）。casad の event_dedupe が 2 回目を捨てるので、
# 一致ルールの casa 発火は 1 回だけになるはず。casad kill 後に残らないよう
# sleep は 60 秒で自然終了させる。
echo "mat $@" >> "${CASAD_TEST_DIR:?}/mat_spawns.log"
printf '%s\n' \
  '{"timestamp":"2026-09-10T00:00:00+09:00","node_id":9119,"endpoint":1,"cluster":"booleanstate","event":"state-change","event_number":100,"data":{"state-value":true},"priming":false}' \
  '{"timestamp":"2026-09-10T00:00:01+09:00","node_id":9119,"endpoint":1,"cluster":"booleanstate","event":"state-change","event_number":100,"data":{"state-value":true},"priming":false}'
sleep 60
exit 0
