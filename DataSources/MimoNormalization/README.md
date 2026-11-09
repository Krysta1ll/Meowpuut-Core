# mimo 固定繁簡轉換資料

四個平台編譯 mimo 時使用相同的轉換快照；一般建置只需要 Python，不需要 ICU、Foundation 或網路。快照以 `t2s`／`s2t`、原文和結果為一筆 TSV，gzip 保存。程式會核對 `SOURCE.json` 的壓縮檔、解壓資料與人工修訂 SHA-256；格式、校驗值錯誤或來源中出現未收錄的轉換鍵時會停止建置，不會改用系統轉換器。

快照依固定上游詞庫的實際轉換需求產生，並不是完整的繁簡轉換詞典。產生時使用 ICU 78.2 的 `Traditional-Simplified` 正向與反向，資料來自 ICU 所附的 CLDR `Hans_Hant.txt`；該檔保留 Apple Inc.、Unicode Inc. 及其他貢獻者的版權聲明。`ICU-LICENSE.txt` 為上游 Unicode License V3 原文，原始規則、版本、URL 和 SHA-256 見 `SOURCE.json`。`reviewed-overrides.tsv` 另列喵撲審核過的 11 筆字形，不修改原始 ICU 規則。

## 更新來源時

1. 在更新後的來源上建置 ICU **78.2**，確認上游來源與授權；普通使用者不需要此步。以 pkg-config 可用的環境為例：

   ```sh
   c++ -std=c++17 -O2 scripts/FreezeConversions.cpp $(pkg-config --cflags --libs icu-i18n) -o /tmp/meowpuut-freeze-78
   /tmp/meowpuut-freeze-78 --version
   ```

   輸出必須是 `ICU 78.2`。請勿以系統其他 ICU 版本或其他方向的轉換替代。
2. 以不存在的檔名產生候選快照。工具不會覆蓋已審核資料，也不需網路：

   ```sh
   python3 scripts/update-normalization-snapshot.py --converter /tmp/meowpuut-freeze-78 --output /tmp/mimo-next.tsv.gz
   python3 scripts/compare-normalization.py DataSources/MimoNormalization/conversions-icu78.2.tsv.gz /tmp/mimo-next.tsv.gz > /tmp/mimo-conversion-changes.tsv
   ```

3. 逐項檢查增減和變更，重新審核可能需要修改的 `reviewed-overrides.tsv`，保留 ICU 的原始版權及授權。更新快照後同步修改 `SOURCE.json` 中檔名、壓縮及解壓資料 SHA-256 與生成器版本資料。不同 zlib 版本的壓縮結果可能不同，請以解壓資料 SHA-256 判斷內容；不可只改雜湊而跳過字形審核。
   如果上游移除某個人工覆寫的鍵，或新規則已與覆寫一致，工具會保留候選快照供比較、列出需審核的覆寫並以非零狀態結束。確認後更新覆寫與 `SOURCE.json` 的覆寫校驗值，再以新檔名重新產生候選。
4. 執行 `python3 scripts/test-frozen-normalization.py --integration`，再跑詞庫、單字與長句回歸；確認拼音、注音、繁簡及所有平台的詞庫內容。

一般詞庫修訂仍放在 `Resources/Lexicon/`。固定轉換並不能解決所有一簡對多繁的語義歧義，請以詞條覆寫和回歸案例處理。
