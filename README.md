# 喵撲核心 · Meowpuut Core

喵撲輸入法的 Rust 引擎、C 介面和詞庫。拼音、注音、日語、中日混打、句子分段與候選排序都在這裡。客戶端和下載請到 [Meowpuut](https://github.com/Krysta1ll/Meowpuut)。

```sh
cargo build --locked --release --workspace
```

編譯完整詞庫後再跑測試。詞庫建置需要 Python 3 和 Swift，繁簡轉換沿用 Foundation。

```sh
bash scripts/build-lexicon.sh
cargo test --locked --workspace
python3 scripts/check-chinese-golden.py
python3 scripts/check-japanese-lexicon.py
```

詞庫產物在 `.build/lexicon/`，C 介面在 `crates/meowpuut-ffi/include/meowpuut.h`。客戶端按 Git 提交固定引擎與詞庫，兩者一同更新。

客製化皮膚的驗證、解密與安裝在 `crates/meowpuut-skin`（只驗證，不含簽章與加密的產生端）。C 介面是 `meowpuut-ffi` 的 `skin` 功能（`mps_*`，標頭裡的 `MEOWPUUT_SKIN`），只給 Android 主程式與輸入法：Release 用 `--features skin` 並設 `MEOWPUUT_RELEASE_BUILD=1`，Debug／Lab 才加 `skin-test-keys`（信任 `test-` 測試金鑰）。鍵盤擴充照舊只編 `-p meowpuut-ffi`，不含這些符號。測試向量與格式摘要在 `Tests/skin-vectors/`。

程式依 [MIT](LICENSE) 授權。詞庫的來源及授權分別列在 [THIRD_PARTY.md](THIRD_PARTY.md) 和 `DataSources/` 各目錄。

[鍵盤設定與支援](docs/IOS_SUPPORT.md) · [iOS 私隱政策](docs/IOS_PRIVACY.md)
