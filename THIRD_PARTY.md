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
  - **本專案自寫的句子**（`DataSources/mimo-everyday`，第 8 節）：約 82.2 萬句（822,480 句），包含口語聊天、詞語搭配與各領域例句。
  - **Tatoeba** 中文句子（2026-11-19 匯出，**CC BY 2.0 FR**）：署名為 Tatoeba 與其貢獻者。
  - **Mozilla Common Voice sentence-collector** zh-TW、zh-CN（commit `bc5d2c4`，**CC0 1.0**）。
  - **CrossWOZ**（`thu-coai/CrossWOZ`，commit `df82c9f`，**Apache-2.0**）：訂房、餐廳、景點、交通的任務對話。
  - **KdConv**（`thu-coai/KdConv`，commit `653db76`，**Apache-2.0**）：電影、音樂、旅遊的對話。
  - **MASSIVE** 1.1 的 zh-TW 與 zh-CN（Amazon，**CC BY 4.0**）：語音助理指令。
- 不用的來源：維基百科與其姊妹計畫（CC BY-SA）、任何相同方式分享／非商業／禁止改作的文字、新聞與官方文件、網頁爬取而發佈者不擁有內容的語料。
- 內容篩選：含 `Resources/Lexicon/blocked-terms.txt` 所列爭議性或冒犯性用語的句子整句不計入，短語也不收含這些詞的。含這些詞的詞，鍵盤不主動提供：不出現在聯想詞、簡拼、補完、打錯字和模糊音修正、emoji 聯想裡，同音時排在日常詞之後；完整拼出時仍打得出來（`scripts/check-no-volunteer.py` 檢查）。
- 測試與保留句不計入：評測集的 Tatoeba 句子編號、保留集句子，以及含有其中任一 5 字以上文字的子句。
- 隨附授權聲明：`Licenses/mimo-pairs-SOURCE.json`、`Licenses/mimo-pairs-Tatoeba-CC-BY-2.0-FR.html`、`Licenses/mimo-pairs-CommonVoice-CC0-1.0.txt`、`Licenses/mimo-pairs-Apache-2.0.txt`、`Licenses/mimo-pairs-CC-BY-4.0.txt`。

## 5a. mimo-latin 中文裡的英文詞

- 詞表：`DataSources/mimo-latin/latin.tsv`，由 `scripts/mimo_latin.py` 數出緊鄰漢字的英文詞與最常見的大小寫。只數人寫成文字的句子：本專案自寫的句子（主要是「中文夾英文」的 `mixed-2.txt.xz`）、Tatoeba 與 Common Voice；三份對話資料不數（語音指令全是小寫，另兩份的英文多半是旅館和歌曲名）。另外併入手寫的常用英文名稱與縮寫（`Resources/Lexicon/latin.tsv`，本專案自行編寫），拼法以它為準。建置時加入 `mimo_base.bin` 的拼音詞表。見 `DataSources/mimo-latin/SOURCE.json`。
- 隨附授權聲明：`Licenses/mimo-latin-SOURCE.json`。

## 5b. mimo-phrases 常單獨說的短語

- 清單：`DataSources/mimo-phrases/phrases.tsv`（約 430 個），由 `scripts/mimo_phrases.py` 從第 5 節的口語文字找出常單獨成句的詞組（辛苦了、知道了、真的嗎）。來源與授權同第 5 節，見 `DataSources/mimo-phrases/SOURCE.json`。
- 隨附授權聲明：`Licenses/mimo-phrases-SOURCE.json`。

## 5c. mimo-english 英文詞庫

- 詞表與詞對：`DataSources/mimo-english/english.tsv.xz`、`pairs.tsv.xz`，由 `scripts/mimo_english.py` 做出：詞（拼法、鍵、加權次數、旗標）限於 **SCOWL/ESDB**（Kevin Atkinson，`en-wl/wordlist` v2 分支，commit `1e5b7d3a72f47a71da5d28686c1dd4b397178485`，美式拼法、等級 70 以下，只讀 `data/scowl-pre.txt`、`data/exclude`、`data/variants`，不執行該專案的程式）加上 mimo-latin 與 `Resources/Lexicon/latin.tsv` 的詞；次數與詞對數自 **Tatoeba** 英文句子（`https://downloads.tatoeba.org/exports/per_language/eng/eng_sentences_detailed.tsv.bz2`，2026-11-14 的每週匯出，sha256 `f3a08a0af8deef3f50777cf0ccd7666f39e619f70fadae4327b7c29119468313`，預設授權 **CC BY 2.0 FR**，部分句子 CC0；授權依 `https://tatoeba.org/en/terms_of_use` 第 6.2 節核對）與 **Mozilla Common Voice** 英文 `server/data/en/sentence-collector.txt`（commit `bc5d2c4cc31f2fd853f7ef0026581fe50c67779b`，sha256 `31ac8e200449ebd7aee4a8c6bd16d4dafd076cf11f7c3efe1035f0283712d136`，**CC0 1.0**，同 commit 的 `server/data/en/LICENSE` 為 CC0 1.0 Universal）數出，口語句子 ×10。含 `Resources/Lexicon/blocked-terms.txt` 的 [en] 節（本專案自寫）所列詞或 SCOWL 標為 offensive-1、offensive-2、vulgar-1 之詞的句子不計入；Tatoeba 中的人名次數設上限。不用 Common Voice 的 `wiki.en.txt`（句子取自維基百科，CC BY-SA）與 SCOWL 等級 80 以上。見 `DataSources/mimo-english/SOURCE.json`。
- SCOWL 的版權聲明（必須隨附）：

  > Copyright 2000-2026 by Kevin Atkinson. Permission to use, copy, modify, distribute, and sell any part of the English Speller Database (ESDB, previously known as SCOWLv2), or word lists created from it, is hereby granted without fee, provided that the above copyright notice appears in all copies and that both the above copyright notice and this notice appear in supporting documentation. Kevin Atkinson makes no representations about the suitability of this database for any purpose. It is provided "as is" without express or implied warranty. ESDB is derived from many sources, most of which are in the Public Domain. Data from the Corpus of Contemporary American English (COCA) was also used. The primary source of words for ESDB comes from 12dicts and ENABLE2K. Both are in the Public Domain, but Alan Beale deserves special credit as he is the author of 12dicts and a major contributor to ENABLE2K.

  Copyright 檔全文（含 UKACD 的 J Ross Beresford 聲明：若隨附於軟體，聲明須「prominently displayed」並逐字附上；WordNet 1.6 的 Princeton 聲明；澳洲英語的 Benjamin Titze 聲明）在 `DataSources/mimo-english/SCOWL-Copyright.txt`。本專案只用美式、等級 70 以下的詞，Copyright 檔寫明 UKACD 聲明適用於大於 80 的詞表，澳洲聲明適用於 `D`／`AU` 拼法；為保險起見，App 的「授權」頁仍須顯示 SCOWL 聲明（UKACD 的顯著標示）。
- 署名：SCOWL/ESDB © Kevin Atkinson and contributors；Tatoeba (https://tatoeba.org) and its contributors；Mozilla Common Voice sentence-collector contributors。
- 隨附授權聲明：`Licenses/mimo-english-SOURCE.json`、`Licenses/mimo-english-SCOWL-Copyright.txt`、`Licenses/mimo-english-CommonVoice-en-CC0-1.0.txt`；Tatoeba 的 CC BY 2.0 FR 沿用 `Licenses/mimo-pairs-Tatoeba-CC-BY-2.0-FR.html`。

## 6. mimo-names 作品、角色、地名等名稱（CC0）

- 清單：`DataSources/mimo-names/names.tsv`（約 5.8 萬個），由 `scripts/mimo_names.py` 向 Wikidata 查詢各類別有中文名稱的項目：動畫、漫畫、輕小說、遊戲與其角色、影視、書籍、疾病與藥品、食物、資訊科技、企業與品牌、車款、地名與車站、天文、樂器、運動、學科、職業等。不取真人，也不取爭議性主題的類別。查詢的類別、門檻與回應情形記在 `DataSources/mimo-names/SOURCE.json`。
- 授權：**Wikidata** 的資料以 **CC0 1.0** 釋出。
- 隨附聲明：`Licenses/mimo-names-SOURCE.json`。

## 7. 各行業用詞（本專案自行編寫）

- `Resources/Lexicon/terms.tsv`：約 2.78 萬個詞（含 AI 工程、臨床西醫、法律合規、金融經濟、網絡熱梗、ACG 遊戲、時事科技與各行業術語），台灣用語與大陸用語分列。2026-11-12 及 2026-11-19 由語言模型為本專案寫成，不含第三方資料；尚未逐條經各領域的人審讀。

## 8. mimo-everyday 本專案自寫的句子（本專案所有）

- `DataSources/mimo-everyday`：`everyday.txt.xz`（2026-11-11，96,627 句日常聊天）、`chat-2.txt.xz`（45,606 句聊天、情境用語與問答）、`written-2.txt.xz`（54,676 句敘述、說明、故事與例句）、`mixed-2.txt.xz`（9,436 句中文夾英文；以上 3 份寫於 2026-11-12），`modern-chat-3.txt.xz`（46,645 句現代社群與熱梗對話）、`modern-domains-3.txt.xz`（191,486 句現代專業領域與錨點詞造句），以及 `colloc-chat-4.txt.xz`（306,601 句高頻口語與詞語搭配對話）、`colloc-domains-4.txt.xz`（71,411 句專業詞彙與同音消歧搭配造句；以上 4 份寫於 2026-11-19）。
- 全部由語言模型為本專案寫成，每位寫手拿到主題、語體與內容限制（不寫爭議性內容、不針對任何群體、不寫真人與作品品牌名稱），沒有看過評測集與保留集。台灣用語（繁體）與大陸用語（簡體）都有。主題、行數與檢查碼見該目錄的 `SOURCE.json`。
## 9. 客製化皮膚 `crates/meowpuut-skin` 的 Rust 相依（BSD-3-Clause、MIT、Apache-2.0 等）

只有 `meowpuut-skin`（以及開了 `skin` 功能的 `meowpuut-ffi`，只給 Android 主程式與輸入法）用到外部 crate；引擎 crate 的相依圖不變，鍵盤擴充不編進這些。版本以 `=` 鎖定，實際解析結果記在 `Cargo.lock`。`png`／`flate2`／`miniz_oxide` 的版本決定安裝時重新編碼的 PNG 位元組（簽章蓋到這些位元組），升級時要重新產生 `Tests/skin-vectors`。

| crate | 版本 | 授權 | 用途 |
| --- | --- | --- | --- |
| ed25519-dalek | 3.0.0 | BSD-3-Clause | Ed25519 驗章（`verify_strict`） |
| x25519-dalek | 3.0.0 | BSD-3-Clause | X25519 索取金鑰 |
| curve25519-dalek | 5.0.0 | BSD-3-Clause | 上兩者的曲線運算 |
| subtle | 2.6.1 | BSD-3-Clause | 常數時間比較（經 dalek 與 AEAD） |
| chacha20poly1305 | 0.11.0 | Apache-2.0 OR MIT | XChaCha20-Poly1305 |
| chacha20 0.10.2、poly1305 0.9.1、aead 0.6.1、cipher 0.5.2、universal-hash 0.6.1、inout 0.2.6 | | MIT OR Apache-2.0 | AEAD 組件 |
| hkdf 0.13.0、hmac 0.13.0、sha2 0.11.1、digest 0.11.3、block-buffer 0.12.1、crypto-common 0.2.2 | | MIT OR Apache-2.0 | HKDF-SHA256、SHA-256 |
| ed25519 3.0.0、signature 3.0.0 | | Apache-2.0 OR MIT | 簽章型別 |
| hybrid-array 0.4.15、typenum 1.20.1、cpufeatures 0.3.1、ctutils 0.4.8、cmov 0.5.4 | | MIT OR Apache-2.0 | 密碼學 crate 的基礎型別 |
| zeroize | 1.9.1 | Apache-2.0 OR MIT | 用完清除金鑰 |
| getrandom 0.4.3、rand_core 0.10.1、libc 0.2.191、cfg-if 1.0.5 | | MIT OR Apache-2.0 | 系統亂數（索取金鑰、暫存資料夾名稱） |
| serde_json 1.0.154、serde_core 1.0.229、itoa 1.0.19 | | MIT OR Apache-2.0 | JSON |
| zmij | 1.0.23 | MIT | serde_json 的浮點數輸出 |
| memchr | 2.8.3 | Unlicense OR MIT | serde_json |
| png 0.18.1、flate2 1.1.11、fdeflate 0.3.7、crc32fast 1.5.5、bitflags 2.13.2 | | MIT OR Apache-2.0 | PNG 解碼與重新編碼 |
| miniz_oxide 0.8.9／0.9.1 | | MIT OR Zlib OR Apache-2.0 | deflate |
| adler2 | 2.0.1 | 0BSD OR MIT OR Apache-2.0 | deflate 檢查碼 |
| simd-adler32 | 0.3.10 | MIT | PNG 檢查碼 |

多重授權的 crate 一律選 MIT。App 的授權頁（Android「關於」）要列出上表並附下列 BSD-3-Clause 聲明：BSD-3-Clause 第 2 條要求二進位散布時重現版權聲明、條件與免責聲明。

```text
ed25519-dalek:    Copyright (c) 2017-2019 isis agora lovecruft. All rights reserved.
x25519-dalek:     Copyright (c) 2017-2021 isis agora lovecruft. All rights reserved.
                  Copyright (c) 2019-2021 DebugSteven. All rights reserved.
curve25519-dalek: Copyright (c) 2016-2021 isis agora lovecruft. All rights reserved.
                  Copyright (c) 2016-2021 Henry de Valence. All rights reserved.
subtle:           Copyright (c) 2016-2017 Isis Agora Lovecruft, Henry de Valence. All rights reserved.
                  Copyright (c) 2016-2024 Isis Agora Lovecruft. All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are
met:

1. Redistributions of source code must retain the above copyright
notice, this list of conditions and the following disclaimer.

2. Redistributions in binary form must reproduce the above copyright
notice, this list of conditions and the following disclaimer in the
documentation and/or other materials provided with the distribution.

3. Neither the name of the copyright holder nor the names of its
contributors may be used to endorse or promote products derived from
this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS
IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED
TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A
PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT
HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED
TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR
PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF
LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING
NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```

測試用金鑰與測試向量（`Tests/skin-vectors/`）由本專案產生，依本倉庫授權；其中的圖與聲音是測試程式畫出、合成的幾何圖形與正弦音，不是角色素材。

## 引擎與整體授權說明

- 解碼核心（Rust，`#![forbid(unsafe_code)]`）、唯讀二進位格式 `mimo_base.bin`（`MIMO-BASE-1` / `MIMO-BIN-1`）與選詞記錄（`mimo_user_wal.log` + `mimo_user.db`）由本專案寫成，執行時不依賴第三方引擎。
- 本專案的程式依 **MIT** 授權發佈（`LICENSE`）。詞庫裡的第三方資料都是只需保留聲明或註明出處的授權（MIT、BSD、NAIST、Apache-2.0、Unicode、CC0、CC BY）。上游各來源的版權與授權聲明完整保留。
