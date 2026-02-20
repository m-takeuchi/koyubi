# Koyubi キーマップ設計

## 設計思想

英語配列キーボード（特に HHKB）で SKK を使うユーザーにとって最も自然な
キーバインドをデフォルトとする。半角/全角キーの存在を前提としない。

## デフォルトキーマップ

### IME 制御

| キー | 動作 | 備考 |
|------|------|------|
| Ctrl-Space | IME ON/OFF トグル | 最も汎用的 |
| Ctrl-J | IME ON → ひらがなモード | Emacs SKK 互換 |
| Ctrl-; | IME OFF → ASCII モード | 押しやすい位置 |

### ひらがなモード (IME ON)

| キー | 動作 |
|------|------|
| [a-z] | ローマ字入力 → ひらがな |
| Shift + [A-Z] | ▽モード開始（変換ポイント設定） |
| Space | 変換（▽→▼）/ 次候補 |
| Enter | 確定（コンポジションがあれば） |
| Ctrl-J | 確定 |
| Ctrl-G | キャンセル（▽/▼→直接入力に戻る） |
| Escape | キャンセル（Ctrl-G と同じ） |
| q | ▽モード中: カタカナに変換して確定 |
| l | ASCII モードに切り替え |
| L (Shift-l) | 全角英数モードに切り替え |
| / | Abbrev モード開始 |
| x | ▼モード中: 前候補に戻る |
| Backspace | 直前の文字を削除 |
| Ctrl-H | Backspace と同じ（Emacs 互換） |

### ▽モード（変換読み入力中）

| キー | 動作 |
|------|------|
| [a-z] | 読みに追加 |
| Shift + [A-Z] | 送り仮名の開始点を設定 |
| Space | 辞書引きを実行 → ▼モードへ |
| Ctrl-J | 読みをそのままひらがなで確定 |
| Ctrl-G / Escape | ▽モードをキャンセル |
| q | カタカナに変換して確定 |
| Backspace | 読みの末尾を削除 |

### ▼モード（変換候補選択中）

| キー | 動作 |
|------|------|
| Space | 次の候補へ |
| x | 前の候補へ |
| Ctrl-J / Enter | 現在の候補で確定 |
| Ctrl-G / Escape | 変換をキャンセル → ▽モードに戻る |
| [a-z] | 現在の候補で確定し、次の入力を開始 |
| Shift + [A-Z] | 現在の候補で確定し、▽モード開始 |

## 英語配列固有の考慮事項

### 記号キーの仮想キーコード

英語配列と日本語配列で仮想キーコードが異なるキーがある。
Koyubi は英語配列を前提とするため、以下のマッピングを使用する。

| 物理キー (US) | VK コード | 日本語配列での物理キー |
|---------------|-----------|----------------------|
| ; | VK_OEM_1 | + |
| : (Shift-;) | VK_OEM_1 + Shift | * |
| [ | VK_OEM_4 | @ |
| ] | VK_OEM_6 | [ |

**重要**: `VK_OEM_*` の値はキーボードレイアウトに依存する。
スキャンコードによる判定を併用し、レイアウトによらず正しいキーを認識する。

### HHKB 固有のキー配置

HHKB Professional 2/Classic/HYBRID の特徴：
- Ctrl が A の左（一般的なキーボードの CapsLock 位置）
- Fn キーで矢印キーを入力
- Backspace が Delete の位置（右上）

これにより：
- Ctrl-J, Ctrl-G 等が非常に押しやすい → SKK との相性が良い
- Ctrl-; (IME OFF) も左手 Ctrl + 右手 ; で自然に押せる
- Backspace (BS) の VK は HHKB の設定による（DIP スイッチ）

### SandS (Space and Shift) 対応 [将来]

Space キーを Shift としても使えるモード。
小指の負担を軽減する。

- Space を押してすぐ離す → Space
- Space を押しながら他のキーを押す → Shift + そのキー

実装上の課題：
- KeyDown 時点では Space か Shift か判定できない
- KeyUp まで待つとタイムラグが発生する
- タイムアウト方式（一定時間内に他のキーが来たら Shift）が現実的

### Sticky Shift 対応 [将来]

Shift を同時押しではなく、順次押しで使えるモード。

- Shift を押して離す → 次の1文字が大文字（▽モード開始）
- 小指の負担を大幅に軽減

## 設定ファイルでのカスタマイズ

`%APPDATA%\Koyubi\config.toml`:

```toml
[keymap.ime_control]
# IME 切り替え
toggle = ["Ctrl-Space"]
on = ["Ctrl-J"]
off = ["Ctrl-Semicolon"]

[keymap.composition]
# 変換操作
start_henkan = "Space"
confirm = ["Ctrl-J", "Enter"]
cancel = ["Ctrl-G", "Escape"]
backspace = ["Backspace", "Ctrl-H"]

# 前候補/次候補
next_candidate = "Space"
prev_candidate = "x"

[keymap.mode_switch]
# モード切り替え
katakana = "q"
ascii = "l"
zenkaku_ascii = "L"
abbrev = "/"

[keymap.advanced]
# SandS (将来)
sands_enabled = false
sands_timeout_ms = 200

# Sticky Shift (将来)
sticky_shift_enabled = false
```

## キーイベント処理の優先順位

```
1. IME 制御キー（Ctrl-Space, Ctrl-J, Ctrl-;）
   → IME ON/OFF 状態を変更
   → 常にキャプチャ（IME OFF でも）

2. モード切り替えキー（l, L, /）
   → ▽/▼モードでなければモード切り替え
   → ▽/▼モード中は通常の入力として処理

3. 変換操作キー（Space, Enter, Ctrl-G, Backspace）
   → 現在の CompositionState に応じた処理

4. Shift + [A-Z]
   → ▽モード開始または送り仮名開始

5. 通常文字 [a-z]
   → ローマ字→かな変換パイプラインへ

6. その他
   → PassThrough（アプリに渡す）
```
