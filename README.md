# mimo · 喵撲輸入核心與詞庫

mimo 是喵撲使用的中文輸入核心。它用拼音或大千注音尋找繁體、簡體候選，支援長句分段、候選選取和剩餘讀音。核心使用 C++17，透過 C 介面供不同平台呼叫。

這個倉庫包含核心、詞庫來源和建置工具。iOS 鍵盤、App 與視覺素材不在此倉庫；各平台的候選介面、個人記憶與表情選詞需要由客戶端接入。

## 建置詞庫

需要 Python 3 和 SQLite。繁簡轉換使用倉庫內固定的 ICU 資料，無須連網。

```sh
python3 scripts/compile-emoji-associations.py
python3 scripts/compile-native-lexicon.py build/lexicon
```

生成目錄包含 `native-pinyin.tsv`、`native-zhuyin.tsv`、`language-evidence.sqlite`、表情對應表及來源報告。呼叫端負責載入資料並維持 lexicon 在 session 存活期間有效。

## 建置核心

```sh
cmake -S Core -B build/core
cmake --build build/core --config Release
```

介面與記憶體所有權見 [MeowpuutCore.h](Core/MeowpuutCore.h)。單一 session 對應一個輸入上下文；選取前段後請讀取剩餘輸入並交付已提交文字。拼音與注音使用各自的詞庫，不共用按鍵規則。

## 詞庫與表情

mimo 1.0 合併 McBopomofo、Pinyin Pro 與本地修訂；簡體與繁體保留各自的頻率權重。日語詞條包含 UniDic 的精選資料。表情對應來自 Unicode CLDR 48 與 Emoji 17.0，另保留喵撲手寫的常用詞和顏文字。完整來源、固定版本與授權見 [THIRD_PARTY.md](THIRD_PARTY.md)。

候選排序仍會出錯，長句有時需要分段選詞。表情關鍵詞的詞庫覆蓋不代表每個讀音和輸入情境都已驗證。

## 授權

核心與本專案修訂採 [MIT License](LICENSE)。上游詞庫與轉換資料分別依隨附的 MIT、BSD 或 Unicode 授權分發；再分發時請一併保留相關聲明。
