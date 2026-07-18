#!/bin/sh
# casa 代役（常駐テスト用）。呼ばれた引数をファイルに追記する。
# 常駐 casad は exit しないため stdout ではなくファイルで観測する。
echo "casa called: $@" >> "${CASAD_TEST_DIR:?}/casa.log"
exit 0
