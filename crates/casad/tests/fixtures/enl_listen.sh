#!/bin/sh
# enl バイナリの代役。`listen` サブコマンド時に固定の INF 通知 JSON を出して終了する
# （living_aircon = 192.0.2.10 / EOJ 013001 の電源 EPC 80 が 30=ON になった通知）。
# CASAD_ENL_EDT で EDT を上書きでき、値不一致のテストに使える。
echo "{\"events\":[{\"ip\":\"192.0.2.10\",\"tid\":\"00ab\",\"seoj\":\"013001\",\"deoj\":\"05ff01\",\"esv\":\"Inf\",\"properties\":[{\"epc\":\"80\",\"pdc\":1,\"edt_hex\":\"${CASAD_ENL_EDT:-30}\"}]}]}"
exit 0
