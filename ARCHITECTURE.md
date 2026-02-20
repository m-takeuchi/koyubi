# Koyubi アーキテクチャ設計

## 設計原則

1. **エンジンとプラットフォーム層の分離**: SKK 変換エンジン (`koyubi-engine`) はプラットフォーム非依存とし、Linux 上で単体テスト可能にする
2. **状態マシン駆動**: 入力状態をすべて明示的な状態遷移で管理する
3. **安全第一**: IME がクラッシュすると OS 巻き込みの可能性があるため、unwrap/panic を極力排除し Result ベースのエラー処理を徹底する

## koyubi-engine

プラットフォーム非依存の SKK 変換エンジン。`#![no_std]` は目指さないが、Windows 依存は一切持たない。

### 入力状態 (InputMode)

```rust
enum InputMode {
    /// ASCII 直接入力（IME OFF 相当）
    Ascii,
    /// ひらがな入力（IME ON）
    Hiragana,
    /// カタカナ入力
    Katakana,
    /// 半角カタカナ入力
    HankakuKatakana,
    /// 全角英数
    ZenkakuAscii,
    /// Abbrev モード
    Abbrev,
}
```

### 変換状態 (CompositionState)

```rust
enum CompositionState {
    /// 通常入力（確定済みテキストを直接入力）
    Direct,
    /// ▽モード: 変換対象の読みを入力中
    /// 例: ▽かんじ
    PreComposition {
        /// 読みのバッファ（ひらがな）
        reading: String,
        /// ローマ字の未確定部分
        pending_roman: String,
    },
    /// ▼モード: 変換候補を選択中
    /// 例: ▼漢字
    Conversion {
        /// 元の読み
        reading: String,
        /// 送り仮名（あれば）
        okuri: Option<String>,
        /// 候補リスト
        candidates: Vec<Candidate>,
        /// 現在選択中のインデックス
        selected: usize,
    },
    /// 辞書登録モード: 変換候補がない場合にユーザーが直接入力
    Registration {
        /// 登録する読み
        reading: String,
        /// 送り仮名（あれば）
        okuri: Option<String>,
        /// 入力中の単語
        word: String,
    },
}
```

### ローマ字変換

ステートマシンによるローマ字→かな変換。

```
入力: 'k' → pending="k" (未確定)
入力: 'a' → output="か", pending="" (確定)
入力: 'k' → pending="k"
入力: 'k' → output="っ", pending="k" (促音)
入力: 'a' → output="か", pending=""
```

テーブル駆動で実装し、カスタムルール（AZIK 等）にも将来対応可能な設計にする。

```rust
struct RomajiTable {
    /// ローマ字 → (出力かな, 残りのローマ字) のマッピング
    /// 例: "ka" → ("か", ""), "kk" → ("っ", "k")
    entries: HashMap<String, (String, String)>,
}
```

### 辞書

SKK 辞書フォーマットに準拠。

```
;; SKK-JISYO 形式
;; 送りなしエントリ
かんじ /漢字/感じ/幹事/
;; 送りありエントリ
おおk /大/多/
```

辞書は起動時にメモリに読み込み、HashMap で高速検索する。

```rust
struct Dictionary {
    /// 送りなしエントリ: 読み → 候補リスト
    okuri_nashi: HashMap<String, Vec<DictEntry>>,
    /// 送りありエントリ: 読み → 候補リスト
    okuri_ari: HashMap<String, Vec<DictEntry>>,
}

struct DictEntry {
    word: String,
    annotation: Option<String>,
}
```

### エンジン API

TSF 層から呼び出すメインインターフェース。

```rust
struct SkkEngine {
    input_mode: InputMode,
    composition: CompositionState,
    romaji_table: RomajiTable,
    dictionaries: Vec<Dictionary>,
    user_dict: UserDictionary,
    config: Config,
}

/// キー入力に対するエンジンの応答
enum EngineResponse {
    /// キーを消費し、確定文字列を出力
    Commit(String),
    /// キーを消費し、コンポジション（未確定文字列）を更新
    UpdateComposition {
        display: String,    // 表示テキスト（例: "▽かんじ"）
        candidates: Option<Vec<String>>,  // 候補リスト（▼モード時）
    },
    /// キーを消費したが、表示の変更なし
    Consumed,
    /// キーを消費しない（アプリにそのまま渡す）
    PassThrough,
}

impl SkkEngine {
    /// キーイベントを処理する
    fn process_key(&mut self, key: KeyEvent) -> EngineResponse;

    /// 現在の入力モードを取得
    fn current_mode(&self) -> InputMode;

    /// 現在のコンポジション表示文字列を取得
    fn composition_text(&self) -> Option<&str>;
}
```

## koyubi-tsf

Windows TSF COM DLL。`windows-rs` クレートを使用。

### COM インターフェース実装

TSF IME として最低限必要なインターフェース：

```
ITfTextInputProcessorEx  ← メインのテキストサービス
├── ITfThreadMgrEventSink    ← フォーカス変更の通知
├── ITfKeyEventSink          ← キー入力のフック
├── ITfCompositionSink       ← コンポジションのライフサイクル
├── ITfDisplayAttributeProvider ← 未確定文字列の装飾
└── ITfLangBarItemButton     ← 言語バーのアイコン/状態表示
```

### DLL エクスポート関数

```rust
#[no_mangle]
extern "system" fn DllGetClassObject(
    rclsid: *const GUID,
    riid: *const GUID,
    ppv: *mut *mut c_void,
) -> HRESULT;

#[no_mangle]
extern "system" fn DllCanUnloadNow() -> HRESULT;

#[no_mangle]
extern "system" fn DllRegisterServer() -> HRESULT;

#[no_mangle]
extern "system" fn DllUnregisterServer() -> HRESULT;
```

### キーイベントの流れ

```
1. ユーザーがキーを押す
2. Windows が ITfKeyEventSink::OnTestKeyDown() を呼ぶ
   → このキーを処理するか？ (true/false を返す)
3. true の場合、ITfKeyEventSink::OnKeyDown() が呼ばれる
   → koyubi-engine の process_key() にキーイベントを渡す
   → EngineResponse に応じて:
      - Commit: ITfContext にテキストを確定挿入
      - UpdateComposition: コンポジションを更新表示
      - PassThrough: 何もしない（アプリに処理を委譲）
```

### 候補ウィンドウ

Win32 API で独自ウィンドウを作成。TSF の ITfCandidateListUIElement は制約が多いため、
多くの IME 実装と同様に独自ウィンドウで候補を表示する。

## データフロー全体図

```
[キー入力]
    │
    ▼
[koyubi-tsf: OnTestKeyDown]
    │ キーを処理するか判定
    ▼
[koyubi-tsf: OnKeyDown]
    │ KeyEvent に変換
    ▼
[koyubi-engine: process_key]
    │ 状態遷移 + 辞書検索
    ▼
[EngineResponse]
    │
    ├── Commit("漢字")
    │     → ITfContext に確定文字列を挿入
    │
    ├── UpdateComposition("▽かんじ")
    │     → コンポジション文字列を更新
    │     → 候補があれば候補ウィンドウ表示
    │
    └── PassThrough
          → キーをアプリに渡す
```

## 英語配列キーボード固有の考慮事項

### 半角/全角キーの不在

日本語配列では VK_KANJI (半角/全角) で IME を切り替えるが、
英語配列にはこのキーが存在しない。

Koyubi では以下のキーで IME ON/OFF を実現する：
- Ctrl-Space: トグル
- Ctrl-J: IME ON
- Ctrl-;: IME OFF

これらは OnTestKeyDown の段階で判定し、IME OFF 状態でも Ctrl-J を
キャプチャして IME ON にできるようにする。

### Shift キーの扱い

SKK の根幹。英語配列では Shift の位置が重要：
- 左 Shift: 一般的に左手側の文字の大文字化に使用
- 右 Shift: 右手側の文字の大文字化に使用

HHKB では Shift キーが比較的押しやすい位置にあるが、
長時間の SKK 使用では小指の負担が大きい。

将来的な機能として：
- SandS (Space and Shift): スペースキーを Shift としても使用
- Sticky Shift: Shift を押して離してから次のキーが大文字になる

### キーコードの注意点

英語配列と日本語配列で仮想キーコード (VK) が異なるキーがある。
特に記号キー（;, :, [, ] 等）は配列によって VK が変わるため、
スキャンコードベースの判定も考慮する。
```

## テスト戦略

### koyubi-engine のテスト（Linux 上で実行可能）

```rust
#[test]
fn test_romaji_to_hiragana() {
    let mut engine = SkkEngine::new(test_config());
    // "ka" → "か"
    assert_eq!(engine.process_key(key('k')), EngineResponse::UpdateComposition { .. });
    assert_eq!(engine.process_key(key('a')), EngineResponse::Commit("か".into()));
}

#[test]
fn test_henkan_basic() {
    let mut engine = SkkEngine::new_with_dict(test_dict());
    // Shift+K で▽モード開始、"anji" で "▽かんじ"
    engine.process_key(shift_key('K'));
    engine.process_key(key('a'));
    engine.process_key(key('n'));
    engine.process_key(key('j'));
    engine.process_key(key('i'));
    // Space で変換
    let response = engine.process_key(key(' '));
    assert_matches!(response, EngineResponse::UpdateComposition { display, .. } if display == "▼漢字");
}

#[test]
fn test_okuri_ari() {
    let mut engine = SkkEngine::new_with_dict(test_dict());
    // "OoKii" → "▽おおk" → "大きい" (送りあり変換)
    engine.process_key(shift_key('O'));  // ▽モード開始 + 'お'
    engine.process_key(key('o'));         // 'おお'
    engine.process_key(shift_key('K'));   // 送り仮名開始
    engine.process_key(key('i'));         // 'き' → 辞書引き "おおk"
    engine.process_key(key('i'));         // → "大きい" 確定
}
```

### koyubi-tsf のテスト（Windows 上で実行）

- 手動テスト: regsvr32 で登録 → メモ帳/ブラウザで入力テスト
- 統合テスト: Windows VM 上で UI オートメーション
```
