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

## 特徴

- **英語配列ファースト**: 半角/全角キーなしで完結する IME 切り替え（Ctrl-J, Ctrl-Space 等をネイティブサポート）
- **正統派 SKK**: Shift による変換開始、送り仮名処理、辞書引きなど SKK の基本動作を忠実に実装
- **Rust 製**: メモリ安全性とパフォーマンスを両立。TSF (Text Services Framework) による全 Windows アプリ対応
- **SandS (Space and Shift)**: Space 長押しで Shift として機能。SKK の Shift 多用を小指から親指に移行
- **Emacs キーバインド**: Ctrl+F/B/A/E/N/P/D/K によるカーソル移動・編集
- **CapsLock → Ctrl**: CapsLock を Ctrl として使用。Ctrl+J/G/Space 等が押しやすくなる
- **Thumb Shift**: 無変換/変換/カナキーを Shift として使用
- **設定の柔軟性**: TOML ベースの設定ファイルでカスタマイズ

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
│           ├── text_service.rs # ITfTextInputProcessor / ITfKeyEventSink 実装
│           ├── key_event.rs    # VK → KeyEvent 変換
│           ├── edit_session.rs # テキスト編集セッション
│           ├── candidate_ui.rs # 候補ウィンドウ
│           ├── lang_bar.rs     # 言語バーボタン
│           └── register.rs     # COM 登録/解除
│
├── dict/                   # デフォルト辞書ファイル
├── docs/                   # 設計ドキュメント
│   ├── ARCHITECTURE.md
│   ├── KEYMAP.md
│   └── DEVELOPMENT.md
└── installer/              # Inno Setup スクリプト・winget マニフェスト
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

### SandS (Space and Shift)

Space キーを Shift キーとして兼用する機能です（デフォルト有効）。

| 操作 | 動作 |
|------|------|
| Space 単独タップ | 通常の Space（変換/次候補） |
| Space + 他キー | Shift + そのキー（▽モード開始等） |

SKK では Shift を多用しますが、SandS により小指の負担を親指に移せます。

### Emacs キーバインド

Ctrl を使ったカーソル移動・編集（デフォルト有効）:

| キー | 動作 |
|------|------|
| Ctrl-F / Ctrl-B | カーソル右 / 左 |
| Ctrl-A / Ctrl-E | 行頭 / 行末 |
| Ctrl-N / Ctrl-P | カーソル下 / 上 |
| Ctrl-D | Delete |
| Ctrl-H | Backspace（常に有効） |
| Ctrl-K | 行末まで削除 |

### CapsLock → Ctrl

CapsLock を Ctrl として使用します（デフォルト無効）。有効にすると:

- CapsLock + J → Ctrl-J（IME ON）
- CapsLock + Space → Ctrl-Space（トグル）
- CapsLock + F → Ctrl-F（カーソル右）
- CapsLock + C → Ctrl-C（コピー等、非 IME コンボもアプリに届く）

> **注意**: CapsLock のトグル動作は OS レベルで発生します。PowerToys やレジストリで CapsLock トグルを無効化することを推奨します。

### 設定

`%APPDATA%\Koyubi\config.toml` で設定：

```toml
# SandS (Space and Shift) — デフォルト: true
sands_enabled = true

# Emacs キーバインド (Ctrl+F/B/A/E/N/P/D/K) — デフォルト: true
emacs_bindings_enabled = true

# Thumb Shift（無変換/変換/カナキーを Shift として使用）— デフォルト: false
thumb_shift_enabled = false

# CapsLock → Ctrl — デフォルト: false
caps_ctrl_enabled = false

# 起動時の入力モード: ascii, hiragana, katakana
initial_mode = "ascii"

# SKK 操作キー
toggle_kana = "q"      # カタカナ変換
enter_ascii = "l"      # ASCII モード
enter_zenkaku = "L"    # 全角英数モード
prev_candidate = "x"   # 前候補

# 辞書パス（省略時は自動検出）
# system_dict_paths = ["C:\\path\\to\\SKK-JISYO.L"]
# user_dict_path = "C:\\path\\to\\user-dict.skk"
```

## 実装状況

- [x] TSF COM DLL（登録・有効化・キー入力フック・テキスト挿入）
- [x] ローマ字 → ひらがな変換（ステートマシン、テーブル駆動）
- [x] SKK 変換エンジン（▽モード / ▼モード / 送り仮名処理）
- [x] SKK 辞書管理（SKK-JISYO.L、EUC-JP / UTF-8 対応）
- [x] ユーザー辞書（辞書登録モード）
- [x] 候補ウィンドウ（数字キー選択、ページ切り替え）
- [x] カタカナ変換 / 全角英数モード
- [x] SandS (Space and Shift)
- [x] Emacs キーバインド（Ctrl+F/B/A/E/N/P/D/K/H）
- [x] CapsLock → Ctrl リマッピング
- [x] Thumb Shift（無変換/変換/カナキー）
- [x] TOML 設定ファイル
- [x] 言語バーボタン / Win11 入力インジケーター
- [x] Inno Setup インストーラー
- [ ] Abbrev モード
- [ ] 数値変換 / 接頭辞・接尾辞変換
- [ ] winget-pkgs 登録

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

## 注意事項

- 本プロジェクトは [Claude Code](https://claude.ai/claude-code)（Anthropic の AI コーディングツール）を使って開発されています。
- 現時点では**英語配列キーボード（US 配列）でのみテスト**しています。日本語配列（JIS 配列）キーボードでの動作は未検証です。

## ライセンス

MIT License

## 名前の由来

SKK ユーザーは Shift キーを多用します。
その Shift キーを押し続ける小指 (koyubi) に敬意を込めて。
