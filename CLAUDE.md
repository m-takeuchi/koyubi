# Koyubi - Claude Code コンテキスト

## プロジェクト概要

**Koyubi（小指）** は、英語配列キーボード（特にHHKB）ユーザーのためのWindows向けSKK日本語入力システム。Rustで実装する。

- リポジトリ: `~/koyubi`
- winget ID（将来）: `Koyubi.SKK`
- ライセンス: MIT

## アーキテクチャ

2つのクレートで構成するワークスペース:

### koyubi-engine（プラットフォーム非依存）
SKK変換エンジン。Linux上で単体テスト可能。Windows依存なし。

担当機能:
- ローマ字→かな変換（ステートマシン、テーブル駆動）
- SKK辞書管理（SKK-JISYO.L等、EUC-JP対応）
- 変換エンジン（▽モード/▼モード/送り仮名処理）
- 入力状態管理（Ascii/Hiragana/Katakana/Abbrev等）
- 設定管理（TOML）

### koyubi-tsf（Windows専用）
Windows TSF (Text Services Framework) COM DLL。`windows-rs`クレートを使用。

担当機能:
- COM DLLエクスポート（DllGetClassObject等）
- ITfTextInputProcessor実装
- ITfKeyEventSink実装（キー入力フック）
- 候補ウィンドウ表示
- 言語バー連携

## ディレクトリ構成

```
koyubi/
├── CLAUDE.md               # このファイル
├── Cargo.toml              # ワークスペース定義
├── crates/
│   ├── engine/             # koyubi-engine
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── romaji.rs       # ローマ字→かな変換
│   │       ├── dict.rs         # SKK辞書の読み込み・検索
│   │       ├── candidate.rs    # 変換候補管理
│   │       ├── composer.rs     # 入力状態管理（▽/▼モード遷移）
│   │       ├── okuri.rs        # 送り仮名処理
│   │       └── config.rs       # 設定ファイル管理
│   └── tsf/                # koyubi-tsf
│       ├── Cargo.toml
│       └── src/
│           ├── lib.rs          # DLLエントリポイント
│           ├── text_service.rs # ITfTextInputProcessor
│           ├── key_handler.rs  # ITfKeyEventSink
│           ├── edit_session.rs # テキスト編集セッション
│           ├── candidate_ui.rs # 候補ウィンドウ
│           └── register.rs     # COM登録/解除
├── dict/                   # デフォルト辞書ファイル
├── installer/              # Inno Setupスクリプト
├── docs/
│   ├── ARCHITECTURE.md     # 詳細設計
│   ├── KEYMAP.md           # キーマップ設計
│   └── DEVELOPMENT.md      # 開発ガイド
└── .github/workflows/      # CI/CD
```

## コア型定義

### 入力モード

```rust
enum InputMode {
    Ascii,              // IME OFF
    Hiragana,           // ひらがな入力
    Katakana,           // カタカナ入力
    HankakuKatakana,    // 半角カタカナ
    ZenkakuAscii,       // 全角英数
    Abbrev,             // Abbrevモード
}
```

### 変換状態

```rust
enum CompositionState {
    Direct,             // 通常入力
    PreComposition {    // ▽モード
        reading: String,
        pending_roman: String,
    },
    Conversion {        // ▼モード
        reading: String,
        okuri: Option<String>,
        candidates: Vec<Candidate>,
        selected: usize,
    },
    Registration {      // 辞書登録モード
        reading: String,
        okuri: Option<String>,
        word: String,
    },
}
```

### エンジンAPI

```rust
enum EngineResponse {
    Commit(String),                          // 確定文字列を出力
    UpdateComposition {                      // コンポジション更新
        display: String,
        candidates: Option<Vec<String>>,
    },
    Consumed,                                // キー消費、表示変更なし
    PassThrough,                             // キーをアプリに渡す
}

impl SkkEngine {
    fn process_key(&mut self, key: KeyEvent) -> EngineResponse;
}
```

## デフォルトキーマップ（英語配列前提）

### IME制御
- Ctrl-Space: IME ON/OFFトグル
- Ctrl-J: IME ON（ひらがなモード）
- Ctrl-;: IME OFF（ASCIIモード）

### SKK操作
- Shift+[A-Z]: ▽モード開始
- Space: 変換/次候補
- Ctrl-J / Enter: 確定
- Ctrl-G / Escape: キャンセル
- q: カタカナ変換
- l: ASCIIモード
- L: 全角英数モード
- /: Abbrevモード
- x: 前候補

## 開発環境

- 開発マシン: Arch Linux on Distrobox on Ubuntu (GMKtec Ryzen 9 PRO 6950H, 32GB)
- テスト環境: QEMU/KVM上のWindows 11 Enterprise評価版
- IDE: Claude Code
- ビルド（Linux）: `cargo xwin build --target x86_64-pc-windows-msvc`
- ビルド（engine テスト）: `cargo test -p koyubi-engine`

## ビルドコマンド

```bash
# engineのテスト（Linux上で実行可能）
cargo test -p koyubi-engine

# Windows DLLのクロスコンパイル
cargo xwin build -p koyubi-tsf --target x86_64-pc-windows-msvc --release

# engineのみビルド
cargo build -p koyubi-engine
```

## 主要な依存クレート

- `windows-rs`: TSF COMインターフェース（tsf crateのみ）
- `encoding_rs`: EUC-JP辞書の読み込み（engine）
- `serde` + `toml`: 設定ファイル管理（engine）

## コーディング規約

- `unwrap()` 禁止（テストコード除く）。`Result`/`Option`を適切に処理
- TSF層でのパニックは絶対禁止（OSフリーズの原因）
- `unsafe`はTSF COM実装に限定、最小限に
- koyubi-engineには`unsafe`を持ち込まない

## 参考プロジェクト

- [ime-rs](https://github.com/saschanaz/ime-rs) - MS IMEサンプルのRust移植（TSF実装の最重要リファレンス）
- [azooKey-Windows](https://github.com/fkunn1326/azooKey-Windows) - Rust TSF IME実装例
- [windows-chewing-tsf](https://github.com/chewing/windows-chewing-tsf) - Rust TSF IME (注音入力)
- [cskk](https://github.com/naokiri/cskk) - Rust製SKKライブラリ（GPLv3注意）
- [CorvusSKK](https://github.com/nathancorvussolis/corvusskk) - Windows向けSKK実装 (C)

## 現在のフェーズ

**Phase 1: 基盤構築**

最初のタスク:
1. Cargo.tomlワークスペースのセットアップ
2. koyubi-engineのスケルトン作成
3. ローマ字→かな変換の実装とテスト
4. SKK辞書の読み込み・検索の実装

engineから先に作り、Linux上でテストを充実させてからTSF層に進む。
