#!/bin/sh
# casa 代役（常駐テスト用）。呼ばれた引数をファイルに追記する。
# 常駐 casad は exit しないため stdout ではなくファイルで観測する。
# 実機アクションの遅延を模し 2 秒 sleep してから記録する。同期実装だとこの
# 2 秒の間 enl の再 spawn がブロックされるため、非同期実装との判別に使う。
sleep 2
echo "casa called: $@" >> "${CASAD_TEST_DIR:?}/casa.log"
exit 0
