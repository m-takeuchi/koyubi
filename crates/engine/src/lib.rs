pub mod composer;
pub mod config;
pub mod dict;
pub mod romaji;

/// IME の入力モード
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputMode {
    /// ASCII 直接入力（IME OFF）
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

impl Default for InputMode {
    fn default() -> Self {
        Self::Ascii
    }
}

/// キーイベント
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyEvent {
    pub key: Key,
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

/// キーの種類
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Space,
    Backspace,
    Escape,
    Tab,
}

/// 変換候補
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub word: String,
    pub annotation: Option<String>,
}

/// 変換状態
#[derive(Debug, Clone, PartialEq)]
pub enum CompositionState {
    /// 通常入力（確定済みテキストを直接入力）
    Direct,
    /// ▽モード: 変換対象の読みを入力中
    PreComposition {
        reading: String,
        pending_roman: String,
    },
    /// ▼モード: 変換候補を選択中
    Conversion {
        reading: String,
        okuri: Option<String>,
        candidates: Vec<Candidate>,
        selected: usize,
    },
    /// 辞書登録モード
    Registration {
        reading: String,
        okuri: Option<String>,
        word: String,
        pending_roman: String,
    },
}

impl Default for CompositionState {
    fn default() -> Self {
        Self::Direct
    }
}

/// 候補ウィンドウに表示する候補情報
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateDisplay {
    pub word: String,
    pub annotation: Option<String>,
}

/// 候補ウィンドウ表示用の状態
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateInfo {
    pub candidates: Vec<CandidateDisplay>,
    pub selected: usize,
}

/// キー入力に対するエンジンの応答
#[derive(Debug, Clone, PartialEq)]
pub enum EngineResponse {
    /// 確定文字列を出力
    Commit(String),
    /// コンポジション（未確定文字列）を更新
    UpdateComposition {
        display: String,
        candidates: Option<Vec<String>>,
    },
    /// キーを消費したが表示変更なし
    Consumed,
    /// キーを消費しない（アプリにそのまま渡す）
    PassThrough,
}
