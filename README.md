# Koyubi - 小指 🤙

**英語配列キーボードユーザーのための Windows SKK 実装**

> SKKユーザーの小指に捧ぐ

## コンセプト

Koyubi は、HHKB をはじめとする英語配列キーボードで SKK を快適に使うことに特化した Windows 向け日本語入力システム（IME）です。

Windows 上の既存 SKK 実装（CorvusSKK, SKK日本語入力FEP 等）は優れたソフトウェアですが、英語配列キーボードでの利用において以下のような痒い点が残ります：

- 半角/全角キーに依存した IME ON/OFF の切り替え
- 英語配列に最適化されたキーバインド設定の不足
- Ctrl-J, Ctrl-G 等の Emacs 由来キーバインドの不完全な対応

Koyubi はこれらの課題を解決し、英語配列 + SKK という組み合わせにおける最高の入力体験を目指します。

## 特徴（計画）

- **英語配列ファースト**: 半角/全角キーなしで完結する IME 切り替え（Ctrl-J, Ctrl-Space 等をネイティブサポート）
- **正統派 SKK**: Shift による変換開始、送り仮名処理、辞書引きなど SKK の基本動作を忠実に実装
- **Rust 製**: メモリ安全性とパフォーマンスを両立。TSF (Text Services Framework) による全 Windows アプリ対応
- **winget 対応**: `winget install Koyubi.SKK` で簡単インストール
- **設定の柔軟性**: TOML ベースの設定ファイルでキーバインドを自由にカスタマイズ

## アーキテクチャ

```
┌─────────────────────────────────────┐
│  Windows アプリケーション            │
└──────────┬──────────────────────────┘
           │ TSF (Text Services Framework)
┌──────────▼──────────────────────────┐
│  koyubi-tsf (COM DLL)               │
│  ├── ITfTextInputProcessor          │
│  ├── ITfKeyEventSink                │
│  ├── 候補ウィンドウ                  │
│  └── 入力モードインジケーター         │
└──────────┬──────────────────────────┘
           │ Rust 関数呼び出し
┌──────────▼──────────────────────────┐
│  koyubi-engine (ライブラリ)          │
│  ├── ローマ字 → かな変換             │
│  ├── SKK 辞書管理                    │
│  │   ├── システム辞書 (SKK-JISYO.L)  │
│  │   └── ユーザー辞書                │
│  ├── 変換エンジン                    │
│  │   ├── ▽モード（未変換）           │
│  │   ├── ▼モード（変換候補選択）      │
│  │   └── 送り仮名処理                │
│  └── 設定管理 (TOML)                │
└─────────────────────────────────────┘
```

### クレート構成

```
koyubi/
├── Cargo.toml              # ワークスペース定義
├── crates/
│   ├── engine/             # koyubi-engine: SKK 変換エンジン（プラットフォーム非依存）
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── romaji.rs       # ローマ字 → かな変換テーブル・ステートマシン
│   │       ├── dict.rs         # SKK 辞書の読み込み・検索
│   │       ├── candidate.rs    # 変換候補管理
│   │       ├── composer.rs     # 入力状態管理（▽/▼モード遷移）
│   │       ├── okuri.rs        # 送り仮名処理
│   │       └── config.rs       # 設定ファイル管理
│   │
│   └── tsf/                # koyubi-tsf: Windows TSF 統合（COM DLL）
│       ├── Cargo.toml
│       └── src/
│           ├── lib.rs          # DLL エントリポイント (DllGetClassObject 等)
│           ├── text_service.rs # ITfTextInputProcessor 実装
│           ├── key_handler.rs  # ITfKeyEventSink 実装
│           ├── edit_session.rs # テキスト編集セッション
│           ├── candidate_ui.rs # 候補ウィンドウ
│           └── register.rs     # COM 登録/解除
│
├── dict/                   # デフォルト辞書ファイル
├── installer/              # Inno Setup スクリプト
├── docs/                   # 設計ドキュメント
│   ├── ARCHITECTURE.md
│   ├── KEYMAP.md
│   └── DEVELOPMENT.md
└── .github/
    └── workflows/
        └── release.yml     # CI/CD
```

## SKK の基本動作

```
入力例: 「ここで履物を脱いでください」

キー入力: kokodeHakimono woNuIdekudasai
                ^               ^^
                Shift で変換開始  送り仮名

状態遷移:
  [直接入力] → k → ko → kok → koko → kokod → kokode
  [Shift] → H → ▽は → ▽はk → ▽はki → ▽はkim → ...
  [Space] → ▼履物 → 確定
  ...
```

## 英語配列キーボード向けの設計

### IME ON/OFF（デフォルト設定）

| キー | 動作 |
|------|------|
| Ctrl-J | IME ON（かなモードへ） |
| Ctrl-; | IME OFF（ASCII モードへ）|
| Ctrl-Space | IME ON/OFF トグル |

※ 半角/全角キーがなくても完全に動作する

### SKK 操作キー

| キー | 動作 |
|------|------|
| Shift + [a-z] | ▽モード開始（変換ポイント設定） |
| Space | 変換（▽→▼）/ 次候補 |
| Ctrl-J | 確定 |
| Ctrl-G | キャンセル |
| Enter | 確定 |
| q | カタカナ変換 |
| l | ASCII モード |
| L | 全角英数モード |
| / | Abbrev モード |
| x | 前候補 |

### カスタマイズ

`%APPDATA%\Koyubi\config.toml` で設定：

```toml
[keymap]
# IME ON/OFF
toggle_ime = ["Ctrl-Space"]
ime_on = ["Ctrl-J"]
ime_off = ["Ctrl-Semicolon"]

# SKK 操作
start_henkan = "Space"
confirm = ["Ctrl-J", "Enter"]
cancel = ["Ctrl-G", "Escape"]
katakana = "q"
abbrev = "/"

[dictionary]
# システム辞書
system = ["dict/SKK-JISYO.L"]
# ユーザー辞書
user = "$APPDATA/Koyubi/user-dict.txt"
# 辞書エンコーディング
encoding = "euc-jp"  # or "utf-8"

[behavior]
# 確定時にIMEをOFFにしない（SKKの一般的な動作）
keep_ime_on_after_confirm = true
# 変換候補が1つだけの場合は自動確定
auto_confirm_single = false
# 候補ウィンドウの表示候補数
candidate_page_size = 7
```

## 開発ロードマップ

### Phase 1: 基盤（TSF スケルトン）
- [ ] Rust で最小限の TSF DLL を作成
- [ ] COM DLL 登録/解除
- [ ] キー入力のフック確認（メモ帳で動作確認）
- [ ] 確定文字列のアプリへの挿入

### Phase 2: ローマ字入力
- [ ] ローマ字 → ひらがな変換ステートマシン
- [ ] コンポジション文字列（未確定文字）の表示
- [ ] IME ON/OFF の切り替え（Ctrl-J / Ctrl-Space）
- [ ] ASCII モード / かなモード切り替え

### Phase 3: SKK 変換コア
- [ ] Shift 押下で ▽モード開始
- [ ] SKK-JISYO.L の読み込みと辞書検索
- [ ] Space で辞書引き（▼モード）
- [ ] 送り仮名の処理
- [ ] 変換候補の順次表示
- [ ] ユーザー辞書の読み書き

### Phase 4: 実用レベルへ
- [ ] 候補ウィンドウの表示
- [ ] カタカナ変換 (q)
- [ ] Abbrev モード (/)
- [ ] 全角英数モード (L)
- [ ] 数値変換
- [ ] 接頭辞・接尾辞変換
- [ ] TOML 設定ファイル対応

### Phase 5: 配布
- [ ] GitHub Actions CI/CD
- [ ] Inno Setup インストーラー
- [ ] 32bit DLL 対応
- [ ] GitHub Releases 公開
- [ ] winget-pkgs へ PR → `winget install Koyubi.SKK`

## ビルド

### 前提条件

- Rust (stable)
- Windows 10/11（テスト環境）

### Windows 上でのビルド

```bash
cargo build --release
```

### Linux からのクロスコンパイル

```bash
cargo install cargo-xwin
cargo xwin build --target x86_64-pc-windows-msvc --release
```

### インストール（開発用）

```powershell
# 管理者権限の PowerShell で
regsvr32.exe "target\release\koyubi_tsf.dll"
```

### アンインストール（開発用）

```powershell
regsvr32.exe /u "target\release\koyubi_tsf.dll"
```

## 参考プロジェクト

- [ime-rs](https://github.com/saschanaz/ime-rs) - Microsoft IME サンプルの Rust 移植
- [azooKey-Windows](https://github.com/fkunn1326/azooKey-Windows) - Rust TSF IME の実装例
- [windows-chewing-tsf](https://github.com/chewing/windows-chewing-tsf) - Rust TSF IME (注音入力)
- [cskk](https://github.com/naokiri/cskk) - Rust 製 SKK ライブラリ
- [CorvusSKK](https://github.com/nathancorvussolis/corvusskk) - Windows 向け SKK 実装 (C)

## ライセンス

MIT License

## 名前の由来

SKK ユーザーは Shift キーを多用します。
その Shift キーを押し続ける小指 (koyubi) に敬意を込めて。
