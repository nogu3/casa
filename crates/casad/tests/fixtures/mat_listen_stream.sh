#!/bin/sh
# mat 代役（常駐ストリームテスト用）。起動 argv を記録し、matd 再購読直後の
# prime バースト相当の 3 行（priming 1 件 + 実イベント 2 件）を一気に出して
# からブロックする。one-shot 実装（child の終了を待ってから出力を読む）は
# このブロックで永遠にイベントを処理できないため、行単位のストリーム実装
# のみがルールを発火できる。casad kill 後に残らないよう sleep は 60 秒で
# 自然終了させる。
echo "mat $@" >> "${CASAD_TEST_DIR:?}/mat_spawns.log"
printf '%s\n' \
  '{"timestamp":"2026-07-27T00:00:00+09:00","node_id":16,"endpoint":1,"cluster":"occupancysensing","attribute":"occupancy","value":1,"priming":true}' \
  '{"timestamp":"2026-07-27T00:00:01+09:00","node_id":16,"endpoint":1,"cluster":"occupancysensing","attribute":"occupancy","value":0,"priming":false}' \
  '{"timestamp":"2026-07-27T00:00:02+09:00","node_id":16,"endpoint":1,"cluster":"occupancysensing","attribute":"occupancy","value":1,"priming":false,"recovered":true}'
sleep 60
exit 0
