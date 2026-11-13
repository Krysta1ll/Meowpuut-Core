# 資料來源與授權

喵撲自己的程式碼和資料是 MIT（[LICENSE](LICENSE)）。米米莫和莫莫的圖像、App 圖示、標誌和節日素材不是 MIT，保留所有權利，範圍和可以怎麼用見 [ASSETS-LICENSE.md](ASSETS-LICENSE.md)。下面是詞庫和其他資料用到的外部來源。

本專案詞庫系統精簡為繁體、簡體、日語各一份統一詞庫（`mimo-tc`、`mimo-sc`、`mimo-jp`），由離線清洗管線（`scripts/mimo_pipeline.py`、日語為 `scripts/mimo_jp_pipeline.py`）改造開源詞庫並編譯為本專案自訂格式的唯讀二進位檔 `mimo_base.bin`（`MIMO-BASE-1` / `MIMO-BIN-1`）與日語詞表 `mimo_jp.bin`（`MIMO-JP-1`）。

## 1. mimo-tc 繁體中文詞庫（MIT / Apache-2.0）

- 單一整合詞庫：`DataSources/mimo-tc/mimo-tc.json`
- 上游開源來源與授權：
  - **McBopomofoData**（`openvanilla/McBopomofoData`，固定修訂 `df084385e546287fc072dfce486d46f102ef9bd4`，**MIT** 授權）：提供注音／帶調拼音基準詞條與三級多音字優先權重。
  - **OpenCC**（`BYVoid/OpenCC`，**Apache-2.0** 授權）：提供 `STPhrases.txt` 繁簡一對多精準字形對齊與 `TWPhrases.txt`／`HKPhrases.txt` 港台地區詞彙。
  - **jieba**（`fxsjy/jieba` `dict.txt.big`，**MIT** 授權）：提供繁體詞頻統計校準。
- 隨附授權聲明：`Licenses/mimo-tc-MIT.txt`、`Licenses/OpenCC-Apache-2.0.txt`、`Licenses/jieba-MIT.txt` 與 `Licenses/mimo-tc-SOURCE.json`。

## 2. mimo-sc 簡體中文詞庫（MIT）

- 單一整合詞庫：`DataSources/mimo-sc/mimo-sc.json`
- 上游開源來源與授權：**pinyin-pro**（`zh-lx/pinyin-pro`，固定修訂 `0785234b4dc0fa3ea9f244e3d77703b2df6a9db4`，**MIT** 授權）：帶調拼音讀音與基礎詞頻。
- 隨附授權聲明：`Licenses/mimo-sc-MIT.txt` 與 `Licenses/mimo-sc-SOURCE.json`。
## 3. mimo-jp 日語詞表（BSD-3-Clause AND NAIST-2003 AND Apache-2.0 AND Unicode-3.0 AND MIT）

- 快照：`DataSources/mimo-jp/mimo-jp.tsv`（約 46,000 詞，由 `scripts/mimo_jp_pipeline.py build` 產生，不手改）、`kanji-pinyin.tsv`、`build-report.json`；來源與檢查碼記在 `DataSources/mimo-jp/SOURCE.json`。
- 建置讀取的上游來源與授權：
  - **Mozc** `dictionary_oss`（`google/mozc`，固定修訂 `a4541df29db443b49f81cf54665873b68fde9efa`，**BSD-3-Clause** 與 **NAIST-2003**）：讀音、詞性與成本，權重由成本換算。
  - **UniDic** `unidic-cwj` 2025.12（國立國語研究所 NINJAL，`https://clrd.ninjal.ac.jp/unidic_archive/2512/unidic-cwj-202512_full.zip`，三種授權中選用 **BSD-3-Clause**）：詞形、活用類別與外來語判斷。
  - **OpenCC** `data/dictionary/JPShinjitaiCharacters.txt`（`BYVoid/OpenCC`，固定修訂 `fae2a0a911c30764e669a7d8b0591cd8e852934a`，**Apache-2.0**）：日本新字體與舊字形對照。
  - **Unihan** 18.0.0（Unicode，`Unihan_Readings.txt`、`Unihan_Variants.txt`，**Unicode-3.0**）：少數漢字的普通話讀音與異體字。
  - 漢字拼音取自本專案的 `mimo-sc` `entries`（pinyin-pro，**MIT**）與 `mimo-tc`（McBopomofoData，**MIT**）讀音。
- 本專案自行編寫（依本倉庫授權）：`Resources/Lexicon/japanese-reviewed.tsv`（60 個審過的詞，原 mimo-jp 1.x）、`japanese-deny.tsv`（不收的詞）、`japanese-kanji-pinyin.tsv`（多音字與國字的讀音）。
- `japanese-deny.tsv` 的 2,098 列待審詞已逐列判定，但判定是自動審讀，尚未經日語母語者審讀。
- 只在開發時用來排定審讀順序、建置不讀也不隨附任何內容的資料：**JMdict**（EDRDG，**CC BY-SA 4.0**，`http://ftp.edrdg.org/pub/Nihongo/JMdict_e.gz`，2026-11-10 的檔案，sha256 `8e511b59c1efb57d7dc90f58f386058035ca662cb4a23d5693b5e185190c5ede`）與 **SudachiDict** 20261015（**Apache-2.0**，`https://sudachi.s3.ap-northeast-1.amazonaws.com/sudachidict-raw/v1/20261015/`，`small_lex.zip` `981b78cdc532ce058b34c76cd9acd3c9a6c771cc0ce528d7de92424b21172125`、`core_lex.zip` `e9ac06222f0ce401a0fef9b129e61e93d59e3ff395a1b037f498962ee5b8ad8b`、`notcore_lex.zip` `16072d75abe6a048947aacbcecfe1ab65a200965a9f1ddb7ceab366348fa21f2`）。詞類偏移量曾對照 TUBELEX-JA（修訂 `7cb5fb36add76b83a266d1967536e1a1d3faa513`，BSD-3-Clause）量測一次，只留下五個常數。
- 授權：快照、`SOURCE.json` 與編譯出的 `mimo_jp.bin` 為 `BSD-3-Clause AND NAIST-2003 AND Apache-2.0 AND Unicode-3.0 AND MIT`。只在開發時用來排審讀順序的 JMdict（CC BY-SA 4.0）不進建置，也不隨附。
- 隨附授權聲明：`Licenses/mimo-jp-Mozc-BSD.txt`（Mozc `LICENSE`，含 NAIST 聲明）、`Licenses/mimo-jp-Unicode-3.0.txt`、`Licenses/mimo-jp-BSD.txt`、`Licenses/mimo-jp-derived-BSD.txt`、`Licenses/mimo-jp-COPYING.txt`、`Licenses/mimo-jp-SOURCE.json`；OpenCC 與 pinyin-pro、McBopomofoData 的聲明即上文的 `OpenCC-Apache-2.0.txt`、`mimo-sc-MIT.txt`、`mimo-tc-MIT.txt`。

## 4. 表情符號與顏文字（Unicode-3.0 與本專案自行編寫）

- CLDR 快照：`DataSources/cldr-emoji/cldr-emoji.tsv`（中文）與 `cldr-emoji-ja.tsv`（日語），由 `scripts/build-emoji-associations.py --cldr` 產生；來源與檢查碼記在 `DataSources/cldr-emoji/SOURCE.json`。上游：**CLDR** 49（`unicode-org/cldr`，標籤 `release-49`，`common/annotations/zh.xml`、`zh_Hant.xml`、`ja.xml`，**Unicode-3.0**）。
- 本專案自行編寫：`Resources/Lexicon/emoji.tsv`（日常用詞對應的表情符號）與 `Resources/Lexicon/kaomoji.tsv`（顏文字與觸發詞）。鍵盤隨附的 `associations.json` 由這兩份和 CLDR 快照建成。
- 隨附授權聲明：`Licenses/CLDR-Unicode-3.0.txt` 與 `Licenses/cldr-emoji-SOURCE.json`。

## 5. mimo-pairs 詞對與詞頻計數（計數為本專案所有；來源只需註明出處）

- 計數快照：`DataSources/mimo-pairs/counts.txt.xz`，由 `scripts/mimo_pairs.py` 從下列文字切詞計數；建置時打包為 `mimo_pairs.bin`（`MIMO-PAIR-1`），單詞次數也用來校準詞的權重。來源版本與 SHA-256 見 `DataSources/mimo-pairs/SOURCE.json`。
- 來源與授權：
  - **本專案自寫的句子**（`DataSources/mimo-everyday`，第 8 節）：約 20.6 萬句，佔計數文字的三成多。
  - **Tatoeba** 中文句子（2026-11-11 匯出，**CC BY 2.0 FR**）：署名為 Tatoeba 與其貢獻者。
  - **Mozilla Common Voice sentence-collector** zh-TW、zh-CN（commit `bc5d2c4`，**CC0 1.0**）。
  - **CrossWOZ**（`thu-coai/CrossWOZ`，commit `df82c9f`，**Apache-2.0**）：訂房、餐廳、景點、交通的任務對話。
  - **KdConv**（`thu-coai/KdConv`，commit `653db76`，**Apache-2.0**）：電影、音樂、旅遊的對話。
  - **MASSIVE** 1.1 的 zh-TW 與 zh-CN（Amazon，**CC BY 4.0**）：語音助理指令。
- 不用的來源：維基百科與其姊妹計畫（CC BY-SA）、任何相同方式分享／非商業／禁止改作的文字、新聞與政府文字、網頁爬取而發佈者不擁有內容的語料。
- 內容篩選：含 `Resources/Lexicon/blocked-terms.txt` 所列詞語（政治與公共衝突、貶低某一群人的說法、辱罵、色情、毒品、自傷）的句子整句不計入，短語也不收含這些詞的。這些詞仍在詞庫裡，使用者自己要打仍然打得出來，只是不會出現在短語和聯想詞裡。
- 測試與保留句不計入：評測集的 Tatoeba 句子編號、保留集句子，以及含有其中任一 5 字以上文字的子句。
- 隨附授權聲明：`Licenses/mimo-pairs-SOURCE.json`、`Licenses/mimo-pairs-Tatoeba-CC-BY-2.0-FR.html`、`Licenses/mimo-pairs-CommonVoice-CC0-1.0.txt`、`Licenses/mimo-pairs-Apache-2.0.txt`、`Licenses/mimo-pairs-CC-BY-4.0.txt`。

## 5a. mimo-latin 中文裡的英文詞

- 詞表：`DataSources/mimo-latin/latin.tsv`，由 `scripts/mimo_latin.py` 數出緊鄰漢字的英文詞與最常見的大小寫。只數人寫成文字的句子：本專案自寫的句子（主要是「中文夾英文」的 `mixed-2.txt.xz`）、Tatoeba 與 Common Voice；三份對話資料不數（語音指令全是小寫，另兩份的英文多半是旅館和歌曲名）。另外併入手寫的常用英文名稱與縮寫（`Resources/Lexicon/latin.tsv`，本專案自行編寫），拼法以它為準。建置時加入 `mimo_base.bin` 的拼音詞表。見 `DataSources/mimo-latin/SOURCE.json`。
- 隨附授權聲明：`Licenses/mimo-latin-SOURCE.json`。

## 5b. mimo-phrases 常單獨說的短語

- 清單：`DataSources/mimo-phrases/phrases.tsv`（約 430 個），由 `scripts/mimo_phrases.py` 從第 5 節的口語文字找出常單獨成句的詞組（辛苦了、知道了、真的嗎）。來源與授權同第 5 節，見 `DataSources/mimo-phrases/SOURCE.json`。
- 隨附授權聲明：`Licenses/mimo-phrases-SOURCE.json`。

## 6. mimo-names 作品、角色、地名等名稱（CC0）

- 清單：`DataSources/mimo-names/names.tsv`（約 5.8 萬個），由 `scripts/mimo_names.py` 向 Wikidata 查詢各類別有中文名稱的項目：動畫、漫畫、輕小說、遊戲與其角色、影視、書籍、疾病與藥品、食物、資訊科技、企業與品牌、車款、地名與車站、天文、樂器、運動、學科、職業等。不取真人，也不取政治、戰爭、宗教的類別。查詢的類別、門檻與回應情形記在 `DataSources/mimo-names/SOURCE.json`。
- 授權：**Wikidata** 的資料以 **CC0 1.0** 釋出。
- 隨附聲明：`Licenses/mimo-names-SOURCE.json`。

## 7. 各行業用詞（本專案自行編寫）

- `Resources/Lexicon/terms.tsv`：約 2 萬個詞，70 個領域（醫療、法律、會計、軟體、製造、動畫製作、餐飲、物流、科學等），台灣用語與大陸用語分列。2026-11-12 由語言模型為本專案寫成，不含第三方資料；尚未逐條經各領域的人審讀。

## 8. mimo-everyday 本專案自寫的句子（本專案所有）

- `DataSources/mimo-everyday`：`everyday.txt.xz`（2026-11-11，96,627 句日常聊天）、`chat-2.txt.xz`（45,606 句聊天、情境用語與問答）、`written-2.txt.xz`（54,676 句敘述、說明、故事與例句）、`mixed-2.txt.xz`（9,436 句中文夾英文）。後三份寫於 2026-11-12。
- 全部由語言模型為本專案寫成，每位寫手拿到主題、語體與內容限制（不寫政治、不針對任何群體、不寫真人與作品品牌名稱），沒有看過評測集與保留集。台灣用語（繁體）與大陸用語（簡體）都有。主題、行數與檢查碼見該目錄的 `SOURCE.json`。
## 引擎與整體授權說明

- 解碼核心（Rust，`#![forbid(unsafe_code)]`）、唯讀二進位格式 `mimo_base.bin`（`MIMO-BASE-1` / `MIMO-BIN-1`）與選詞記錄（`mimo_user_wal.log` + `mimo_user.db`）由本專案寫成，執行時不依賴第三方引擎。
- 本專案的程式依 **MIT** 授權發佈（`LICENSE`）。詞庫裡的第三方資料都是只需保留聲明或註明出處的授權（MIT、BSD、NAIST、Apache-2.0、Unicode、CC0、CC BY）。上游各來源的版權與授權聲明完整保留。
