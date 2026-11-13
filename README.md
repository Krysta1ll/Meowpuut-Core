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

程式依 [MIT](LICENSE) 授權。詞庫的來源及授權分別列在 [THIRD_PARTY.md](THIRD_PARTY.md) 和 `DataSources/` 各目錄。早期 C++ 版保存在 `legacy-cpp` 標籤。

[鍵盤設定與支援](docs/IOS_SUPPORT.md) · [iOS 私隱政策](docs/IOS_PRIVACY.md)
