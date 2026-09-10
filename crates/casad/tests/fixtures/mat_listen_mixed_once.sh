#!/bin/sh
# mat 代役（--listen-once-mat の属性行・イベント行混在バッチテスト用）。
# 属性行 1 件（motion = node 1234 の occupancy=0）とデバイスイベント行 1 件
# （door = node 9119 の booleanstate state-change、state-value=true）を
# 1 バッチで出して終了する。
printf '%s\n' \
  '{"timestamp":"2026-09-10T00:00:00+09:00","node_id":1234,"endpoint":1,"cluster":"occupancysensing","attribute":"occupancy","value":0,"priming":false}' \
  '{"timestamp":"2026-09-10T00:00:01+09:00","node_id":9119,"endpoint":1,"cluster":"booleanstate","event":"state-change","event_number":100,"data":{"state-value":true},"priming":false}'
exit 0
