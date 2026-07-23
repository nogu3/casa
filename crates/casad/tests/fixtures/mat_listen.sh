#!/bin/sh
# mat バイナリの代役。`listen` サブコマンド時に固定の occupancy イベント 1 行を出して終了する
# （study_motion = node 16 / endpoint 1 の occupancy が 0=不在 になったイベント）。
# CASAD_MAT_VALUE で value を、CASAD_MAT_PRIMING で priming を上書きできる。
echo "{\"timestamp\":\"2026-07-23T00:00:00+09:00\",\"node_id\":16,\"endpoint\":1,\"cluster\":\"occupancysensing\",\"attribute\":\"occupancy\",\"value\":${CASAD_MAT_VALUE:-0},\"priming\":${CASAD_MAT_PRIMING:-false}}"
exit 0
