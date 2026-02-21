//! SKK 入力状態管理（コンポーザー）
//!
//! SkkEngine がキーイベントを受け取り、入力モードと変換状態に応じて
//! ローマ字変換・辞書検索・候補選択を行う。

use crate::dict::Dictionary;
use crate::romaji::{RomajiConverter, RomajiTable};
use crate::{Candidate, CompositionState, EngineResponse, InputMode, Key, KeyEvent};

/// SKK 変換エンジン
pub struct SkkEngine {
    input_mode: InputMode,
    composition: CompositionState,
    romaji: RomajiConverter,
    romaji_table: RomajiTable,
    dictionaries: Vec<Dictionary>,
    /// 送り仮名の子音プレフィックス（送りあり変換時に使用）
    okuri_prefix: Option<String>,
}

impl SkkEngine {
    /// 新しいエンジンを生成（ASCII モード、辞書なし）
    pub fn new() -> Self {
        Self {
            input_mode: InputMode::Ascii,
            composition: CompositionState::Direct,
            romaji: RomajiConverter::new(),
            romaji_table: RomajiTable::default_table(),
            dictionaries: Vec::new(),
            okuri_prefix: None,
        }
    }

    /// 辞書を追加
    pub fn add_dictionary(&mut self, dict: Dictionary) {
        self.dictionaries.push(dict);
    }

    /// 現在の入力モードを取得
    pub fn current_mode(&self) -> InputMode {
        self.input_mode
    }

    /// 現在のコンポジション表示文字列を取得
    ///
    /// TSF 層は process_key() の後にこのメソッドを呼び、
    /// コンポジション表示を更新する。
    pub fn composition_text(&self) -> Option<String> {
        match &self.composition {
            CompositionState::Direct => {
                let pending = self.romaji.pending();
                if pending.is_empty() {
                    None
                } else {
                    Some(pending.to_string())
                }
            }
            CompositionState::PreComposition { reading, .. } => {
                let pending = self.romaji.pending();
                Some(format!("▽{}{}", reading, pending))
            }
            CompositionState::Conversion {
                candidates,
                selected,
                okuri,
                ..
            } => candidates.get(*selected).map(|c| {
                let okuri_str = okuri.as_deref().unwrap_or("");
                format!("▼{}{}", c.word, okuri_str)
            }),
            CompositionState::Registration { .. } => Some("[辞書登録]".to_string()),
        }
    }

    // =========================================================
    // メインディスパッチ
    // =========================================================

    /// キーイベントを処理する
    pub fn process_key(&mut self, key: KeyEvent) -> EngineResponse {
        // IME 制御キーは常に最優先
        if let Some(response) = self.handle_ime_control(&key) {
            return response;
        }

        match self.input_mode {
            InputMode::Ascii => EngineResponse::PassThrough,
            InputMode::Hiragana | InputMode::Katakana => self.handle_hiragana(key),
            // TODO: 他のモードの実装
            _ => EngineResponse::PassThrough,
        }
    }

    fn handle_hiragana(&mut self, key: KeyEvent) -> EngineResponse {
        match self.composition.clone() {
            CompositionState::Direct => self.handle_direct(key),
            CompositionState::PreComposition { .. } => self.handle_pre_composition(key),
            CompositionState::Conversion { .. } => self.handle_conversion(key),
            CompositionState::Registration { .. } => EngineResponse::PassThrough,
        }
    }

    // =========================================================
    // IME 制御
    // =========================================================

    fn handle_ime_control(&mut self, key: &KeyEvent) -> Option<EngineResponse> {
        if !key.ctrl {
            return None;
        }

        match &key.key {
            // Ctrl-Space: トグル
            Key::Space => {
                if self.input_mode == InputMode::Ascii {
                    self.input_mode = InputMode::Hiragana;
                } else {
                    self.reset_state();
                    self.input_mode = InputMode::Ascii;
                }
                Some(EngineResponse::Consumed)
            }
            // Ctrl-J: IME ON（ひらがな）
            Key::Char('j') => {
                if self.input_mode == InputMode::Ascii {
                    self.input_mode = InputMode::Hiragana;
                    Some(EngineResponse::Consumed)
                } else {
                    None // ひらがなモードでは Ctrl-J は確定キーとして使う
                }
            }
            // Ctrl-;: IME OFF
            Key::Char(';') => {
                self.reset_state();
                self.input_mode = InputMode::Ascii;
                Some(EngineResponse::Consumed)
            }
            _ => None,
        }
    }

    // =========================================================
    // Direct モード（通常入力）
    // =========================================================

    fn handle_direct(&mut self, key: KeyEvent) -> EngineResponse {
        // Ctrl 系
        if key.ctrl {
            return match &key.key {
                Key::Char('j') => {
                    let flushed = self.romaji.flush();
                    if flushed.is_empty() {
                        EngineResponse::Consumed
                    } else {
                        EngineResponse::Commit(self.kana_output(flushed))
                    }
                }
                Key::Char('g') => EngineResponse::Consumed,
                _ => EngineResponse::PassThrough,
            };
        }

        match &key.key {
            // l: ASCII モード
            Key::Char('l') if !key.shift => {
                let flushed = self.romaji.flush();
                let flushed = self.kana_output(flushed);
                self.input_mode = InputMode::Ascii;
                if flushed.is_empty() {
                    EngineResponse::Consumed
                } else {
                    EngineResponse::Commit(flushed)
                }
            }

            // L (Shift-l): 全角英数モード（▽モード開始より優先）
            Key::Char('L') if key.shift => {
                let flushed = self.romaji.flush();
                let flushed = self.kana_output(flushed);
                self.input_mode = InputMode::ZenkakuAscii;
                if flushed.is_empty() {
                    EngineResponse::Consumed
                } else {
                    EngineResponse::Commit(flushed)
                }
            }

            // q: ひらがな/カタカナモード切替
            Key::Char('q') if !key.shift && !key.ctrl => {
                let flushed = self.romaji.flush();
                let flushed = self.kana_output(flushed);
                self.input_mode = if self.input_mode == InputMode::Hiragana {
                    InputMode::Katakana
                } else {
                    InputMode::Hiragana
                };
                if flushed.is_empty() {
                    EngineResponse::Consumed
                } else {
                    EngineResponse::Commit(flushed)
                }
            }

            // Shift + 英字: ▽モード開始
            Key::Char(ch) if key.shift && ch.is_ascii_uppercase() => {
                let flushed = self.romaji.flush();
                let flushed = self.kana_output(flushed);
                let lower = ch.to_ascii_lowercase();
                self.composition = CompositionState::PreComposition {
                    reading: String::new(),
                    pending_roman: String::new(),
                };
                let output = self.romaji.feed(lower, &self.romaji_table);
                self.apply_romaji_to_precomp(&output.committed, &output.pending);

                if flushed.is_empty() {
                    self.make_composition_response()
                } else {
                    EngineResponse::Commit(flushed)
                }
            }

            // 英小文字: ローマ字入力
            Key::Char(ch) if ch.is_ascii_lowercase() => {
                let output = self.romaji.feed(*ch, &self.romaji_table);
                if output.committed.is_empty() {
                    EngineResponse::UpdateComposition {
                        display: output.pending,
                        candidates: None,
                    }
                } else {
                    EngineResponse::Commit(self.kana_output(output.committed))
                }
            }

            // Backspace / Ctrl-H
            Key::Backspace => {
                if self.romaji.backspace() {
                    let pending = self.romaji.pending();
                    if pending.is_empty() {
                        EngineResponse::UpdateComposition {
                            display: String::new(),
                            candidates: None,
                        }
                    } else {
                        EngineResponse::UpdateComposition {
                            display: pending.to_string(),
                            candidates: None,
                        }
                    }
                } else {
                    EngineResponse::PassThrough
                }
            }

            // Enter
            Key::Enter => {
                let flushed = self.romaji.flush();
                if flushed.is_empty() {
                    EngineResponse::PassThrough
                } else {
                    EngineResponse::Commit(self.kana_output(flushed))
                }
            }

            _ => EngineResponse::PassThrough,
        }
    }

    // =========================================================
    // PreComposition モード（▽モード）
    // =========================================================

    fn handle_pre_composition(&mut self, key: KeyEvent) -> EngineResponse {
        let is_backspace =
            key.key == Key::Backspace || (key.ctrl && key.key == Key::Char('h'));

        if is_backspace {
            return self.precomp_backspace();
        }

        // Ctrl-J: 読みをひらがなのまま確定
        if key.ctrl && key.key == Key::Char('j') {
            return self.confirm_as_hiragana();
        }

        // Enter: 読みをひらがなのまま確定
        if key.key == Key::Enter {
            return self.confirm_as_hiragana();
        }

        // Ctrl-G / Escape: キャンセル
        if (key.ctrl && key.key == Key::Char('g')) || key.key == Key::Escape {
            return self.cancel_composition();
        }

        // Space: 変換開始
        if key.key == Key::Space {
            return self.start_conversion();
        }

        // q: カタカナ変換
        if !key.ctrl && !key.shift && key.key == Key::Char('q') {
            return self.convert_to_katakana();
        }

        // Shift + 英字: 送り仮名開始
        if key.shift {
            if let Key::Char(ch) = key.key {
                if ch.is_ascii_uppercase() {
                    return self.start_okuri(ch.to_ascii_lowercase());
                }
            }
        }

        // 英小文字: 読みに追加
        if let Key::Char(ch) = key.key {
            if ch.is_ascii_lowercase() && !key.ctrl {
                return self.feed_precomp_char(ch);
            }
        }

        EngineResponse::PassThrough
    }

    fn feed_precomp_char(&mut self, ch: char) -> EngineResponse {
        let output = self.romaji.feed(ch, &self.romaji_table);
        let in_okuri = self.okuri_prefix.is_some();

        // 送りモード中にかなが確定 → 辞書引き
        if in_okuri && !output.committed.is_empty() {
            return self.trigger_okuri_conversion(output.committed);
        }

        self.apply_romaji_to_precomp(&output.committed, &output.pending);
        self.make_composition_response()
    }

    fn start_okuri(&mut self, ch: char) -> EngineResponse {
        // pending のローマ字を読みに確定
        let flushed = self.romaji.flush();
        if let CompositionState::PreComposition {
            reading,
            pending_roman,
        } = &mut self.composition
        {
            reading.push_str(&flushed);
            *pending_roman = String::new();
        }

        self.okuri_prefix = Some(ch.to_string());
        let output = self.romaji.feed(ch, &self.romaji_table);

        // 母音の場合は即座にかな確定 → 辞書引き
        if !output.committed.is_empty() {
            return self.trigger_okuri_conversion(output.committed);
        }

        self.update_pending_roman(&output.pending);
        self.make_composition_response()
    }

    fn trigger_okuri_conversion(&mut self, okuri_kana: String) -> EngineResponse {
        let reading = match &self.composition {
            CompositionState::PreComposition { reading, .. } => reading.clone(),
            _ => return EngineResponse::Consumed,
        };
        let prefix = self.okuri_prefix.take().unwrap_or_default();
        let lookup_key = format!("{}{}", reading, prefix);
        self.romaji.clear();

        let candidates = self.lookup_all_dicts(&lookup_key);
        if candidates.is_empty() {
            // 候補なし → ▽モードに戻る
            self.composition = CompositionState::PreComposition {
                reading,
                pending_roman: String::new(),
            };
            EngineResponse::Consumed
        } else {
            self.composition = CompositionState::Conversion {
                reading,
                okuri: Some(okuri_kana),
                candidates,
                selected: 0,
            };
            self.make_composition_response()
        }
    }

    fn start_conversion(&mut self) -> EngineResponse {
        let flushed = self.romaji.flush();

        let reading = if let CompositionState::PreComposition { reading, .. } =
            &mut self.composition
        {
            reading.push_str(&flushed);
            reading.clone()
        } else {
            return EngineResponse::Consumed;
        };

        if reading.is_empty() {
            return EngineResponse::Consumed;
        }

        let candidates = self.lookup_all_dicts(&reading);
        if candidates.is_empty() {
            // 候補なし: ▽モードのまま
            EngineResponse::Consumed
        } else {
            self.composition = CompositionState::Conversion {
                reading,
                okuri: None,
                candidates,
                selected: 0,
            };
            self.make_composition_response()
        }
    }

    fn confirm_as_hiragana(&mut self) -> EngineResponse {
        let flushed = self.romaji.flush();
        let text = match &self.composition {
            CompositionState::PreComposition { reading, .. } => {
                format!("{}{}", reading, flushed)
            }
            _ => flushed,
        };
        self.composition = CompositionState::Direct;
        self.okuri_prefix = None;

        if text.is_empty() {
            EngineResponse::Consumed
        } else {
            EngineResponse::Commit(text)
        }
    }

    fn convert_to_katakana(&mut self) -> EngineResponse {
        let flushed = self.romaji.flush();
        let text = match &self.composition {
            CompositionState::PreComposition { reading, .. } => {
                hiragana_to_katakana(&format!("{}{}", reading, flushed))
            }
            _ => hiragana_to_katakana(&flushed),
        };
        self.composition = CompositionState::Direct;
        self.okuri_prefix = None;

        if text.is_empty() {
            EngineResponse::Consumed
        } else {
            EngineResponse::Commit(text)
        }
    }

    fn precomp_backspace(&mut self) -> EngineResponse {
        if self.romaji.backspace() {
            let pending = self.romaji.pending().to_string();
            self.update_pending_roman(&pending);
            return self.make_composition_response();
        }

        if let CompositionState::PreComposition { reading, .. } = &mut self.composition {
            if reading.pop().is_some() && !reading.is_empty() {
                return self.make_composition_response();
            }
        }

        // 読みが空 → キャンセル
        self.cancel_composition()
    }

    // =========================================================
    // Conversion モード（▼モード）
    // =========================================================

    fn handle_conversion(&mut self, key: KeyEvent) -> EngineResponse {
        // Ctrl-J: 確定
        if key.ctrl && key.key == Key::Char('j') {
            return self.confirm_candidate();
        }

        // Ctrl-G / Escape: キャンセル → ▽モードに戻る
        if (key.ctrl && key.key == Key::Char('g')) || key.key == Key::Escape {
            return self.cancel_to_precomp();
        }

        match &key.key {
            // Space: 次候補
            Key::Space => self.next_candidate(),

            // x: 前候補
            Key::Char('x') if !key.ctrl && !key.shift => self.prev_candidate(),

            // Enter: 確定
            Key::Enter => self.confirm_candidate(),

            // Shift + 英字: 確定して新しい▽モード開始
            Key::Char(ch) if key.shift && ch.is_ascii_uppercase() => {
                self.confirm_and_start_precomp(ch.to_ascii_lowercase())
            }

            // 英小文字: 確定して次の入力を開始
            Key::Char(ch) if ch.is_ascii_lowercase() && !key.ctrl => {
                self.confirm_and_continue(*ch)
            }

            _ => EngineResponse::PassThrough,
        }
    }

    fn next_candidate(&mut self) -> EngineResponse {
        if let CompositionState::Conversion {
            selected,
            candidates,
            ..
        } = &mut self.composition
        {
            if *selected + 1 < candidates.len() {
                *selected += 1;
            }
            // 最後の候補の場合はそのまま（TODO: 辞書登録モード）
        }
        self.make_composition_response()
    }

    fn prev_candidate(&mut self) -> EngineResponse {
        let back_to_precomp =
            if let CompositionState::Conversion {
                selected, reading, ..
            } = &mut self.composition
            {
                if *selected > 0 {
                    *selected -= 1;
                    None
                } else {
                    Some(reading.clone())
                }
            } else {
                return EngineResponse::Consumed;
            };

        if let Some(reading) = back_to_precomp {
            self.composition = CompositionState::PreComposition {
                reading,
                pending_roman: String::new(),
            };
        }
        self.make_composition_response()
    }

    fn confirm_candidate(&mut self) -> EngineResponse {
        let text = self.get_confirmed_text();
        self.composition = CompositionState::Direct;
        self.okuri_prefix = None;

        if text.is_empty() {
            EngineResponse::Consumed
        } else {
            EngineResponse::Commit(text)
        }
    }

    fn confirm_and_continue(&mut self, ch: char) -> EngineResponse {
        let confirmed = self.get_confirmed_text();
        self.composition = CompositionState::Direct;
        self.okuri_prefix = None;

        let output = self.romaji.feed(ch, &self.romaji_table);
        let mut total = confirmed;
        total.push_str(&self.kana_output(output.committed));

        if total.is_empty() {
            EngineResponse::UpdateComposition {
                display: output.pending,
                candidates: None,
            }
        } else {
            EngineResponse::Commit(total)
        }
    }

    fn confirm_and_start_precomp(&mut self, ch: char) -> EngineResponse {
        let confirmed = self.get_confirmed_text();
        self.okuri_prefix = None;

        self.composition = CompositionState::PreComposition {
            reading: String::new(),
            pending_roman: String::new(),
        };

        let output = self.romaji.feed(ch, &self.romaji_table);
        self.apply_romaji_to_precomp(&output.committed, &output.pending);

        if confirmed.is_empty() {
            self.make_composition_response()
        } else {
            EngineResponse::Commit(confirmed)
        }
    }

    fn cancel_to_precomp(&mut self) -> EngineResponse {
        let reading = if let CompositionState::Conversion { reading, .. } = &self.composition {
            reading.clone()
        } else {
            String::new()
        };

        self.composition = CompositionState::PreComposition {
            reading,
            pending_roman: String::new(),
        };
        self.okuri_prefix = None;
        self.romaji.clear();
        self.make_composition_response()
    }

    // =========================================================
    // ヘルパー
    // =========================================================

    fn get_confirmed_text(&self) -> String {
        if let CompositionState::Conversion {
            candidates,
            selected,
            okuri,
            ..
        } = &self.composition
        {
            let word = candidates
                .get(*selected)
                .map(|c| c.word.as_str())
                .unwrap_or("");
            let okuri_str = okuri.as_deref().unwrap_or("");
            format!("{}{}", word, okuri_str)
        } else {
            String::new()
        }
    }

    fn make_composition_response(&self) -> EngineResponse {
        match self.composition_text() {
            Some(display) => {
                let candidates = if let CompositionState::Conversion { candidates, .. } =
                    &self.composition
                {
                    Some(candidates.iter().map(|c| c.word.clone()).collect())
                } else {
                    None
                };
                EngineResponse::UpdateComposition {
                    display,
                    candidates,
                }
            }
            None => EngineResponse::Consumed,
        }
    }

    fn apply_romaji_to_precomp(&mut self, committed: &str, pending: &str) {
        if let CompositionState::PreComposition {
            reading,
            pending_roman,
        } = &mut self.composition
        {
            reading.push_str(committed);
            *pending_roman = pending.to_string();
        }
    }

    fn update_pending_roman(&mut self, pending: &str) {
        if let CompositionState::PreComposition { pending_roman, .. } = &mut self.composition {
            *pending_roman = pending.to_string();
        }
    }

    fn lookup_all_dicts(&self, reading: &str) -> Vec<Candidate> {
        let mut results = Vec::new();
        for dict in &self.dictionaries {
            if let Some(entries) = dict.lookup(reading) {
                for entry in entries {
                    results.push(Candidate {
                        word: entry.word.clone(),
                        annotation: entry.annotation.clone(),
                    });
                }
            }
        }
        results
    }

    pub fn reset_state(&mut self) {
        self.composition = CompositionState::Direct;
        self.romaji.clear();
        self.okuri_prefix = None;
    }

    fn cancel_composition(&mut self) -> EngineResponse {
        self.reset_state();
        EngineResponse::UpdateComposition {
            display: String::new(),
            candidates: None,
        }
    }

    /// ひらがなのテキストを現在の入力モードに合わせて変換
    ///
    /// カタカナモードの場合はカタカナに変換、それ以外はそのまま返す。
    fn kana_output(&self, hiragana: String) -> String {
        if self.input_mode == InputMode::Katakana {
            hiragana_to_katakana(&hiragana)
        } else {
            hiragana
        }
    }
}

impl Default for SkkEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// ひらがなをカタカナに変換
fn hiragana_to_katakana(s: &str) -> String {
    s.chars()
        .map(|c| {
            let cp = c as u32;
            if (0x3041..=0x3096).contains(&cp) {
                // ひらがな → カタカナ（+0x60）
                char::from_u32(cp + 0x60).unwrap_or(c)
            } else {
                c
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dict::Dictionary;

    // === テストヘルパー ===

    fn key(ch: char) -> KeyEvent {
        KeyEvent {
            key: Key::Char(ch),
            shift: false,
            ctrl: false,
            alt: false,
        }
    }

    fn shift_key(ch: char) -> KeyEvent {
        KeyEvent {
            key: Key::Char(ch),
            shift: true,
            ctrl: false,
            alt: false,
        }
    }

    fn ctrl_key(ch: char) -> KeyEvent {
        KeyEvent {
            key: Key::Char(ch),
            shift: false,
            ctrl: true,
            alt: false,
        }
    }

    fn ctrl_space() -> KeyEvent {
        KeyEvent {
            key: Key::Space,
            shift: false,
            ctrl: true,
            alt: false,
        }
    }

    fn space() -> KeyEvent {
        KeyEvent {
            key: Key::Space,
            shift: false,
            ctrl: false,
            alt: false,
        }
    }

    fn enter() -> KeyEvent {
        KeyEvent {
            key: Key::Enter,
            shift: false,
            ctrl: false,
            alt: false,
        }
    }

    fn backspace() -> KeyEvent {
        KeyEvent {
            key: Key::Backspace,
            shift: false,
            ctrl: false,
            alt: false,
        }
    }

    fn escape() -> KeyEvent {
        KeyEvent {
            key: Key::Escape,
            shift: false,
            ctrl: false,
            alt: false,
        }
    }

    fn hiragana_engine() -> SkkEngine {
        let mut engine = SkkEngine::new();
        engine.input_mode = InputMode::Hiragana;
        engine
    }

    fn engine_with_dict() -> SkkEngine {
        let mut engine = hiragana_engine();
        engine.add_dictionary(test_dict());
        engine
    }

    fn test_dict() -> Dictionary {
        Dictionary::from_str(
            "\
おおk /大/多/
かk /書/欠/掛/
たべr /食/
かんじ /漢字/感じ/幹事/
とうきょう /東京/
にほん /日本/
ひと /人/
き /木/気/
",
        )
    }

    // === IME 制御テスト ===

    #[test]
    fn test_ctrl_space_toggle() {
        let mut engine = SkkEngine::new();
        assert_eq!(engine.current_mode(), InputMode::Ascii);

        assert_eq!(engine.process_key(ctrl_space()), EngineResponse::Consumed);
        assert_eq!(engine.current_mode(), InputMode::Hiragana);

        assert_eq!(engine.process_key(ctrl_space()), EngineResponse::Consumed);
        assert_eq!(engine.current_mode(), InputMode::Ascii);
    }

    #[test]
    fn test_ctrl_j_ime_on() {
        let mut engine = SkkEngine::new();
        assert_eq!(engine.process_key(ctrl_key('j')), EngineResponse::Consumed);
        assert_eq!(engine.current_mode(), InputMode::Hiragana);
    }

    #[test]
    fn test_ctrl_semicolon_ime_off() {
        let mut engine = hiragana_engine();
        assert_eq!(engine.process_key(ctrl_key(';')), EngineResponse::Consumed);
        assert_eq!(engine.current_mode(), InputMode::Ascii);
    }

    // === ASCII モードテスト ===

    #[test]
    fn test_ascii_passthrough() {
        let mut engine = SkkEngine::new();
        assert_eq!(engine.process_key(key('a')), EngineResponse::PassThrough);
        assert_eq!(engine.process_key(space()), EngineResponse::PassThrough);
        assert_eq!(engine.process_key(enter()), EngineResponse::PassThrough);
    }

    // === Direct モード（ひらがな直接入力）テスト ===

    #[test]
    fn test_direct_basic_kana() {
        let mut engine = hiragana_engine();
        // "ka" → "か"
        assert_eq!(
            engine.process_key(key('k')),
            EngineResponse::UpdateComposition {
                display: "k".into(),
                candidates: None
            }
        );
        assert_eq!(
            engine.process_key(key('a')),
            EngineResponse::Commit("か".into())
        );
        assert_eq!(engine.composition_text(), None);
    }

    #[test]
    fn test_direct_sokuon() {
        let mut engine = hiragana_engine();
        engine.process_key(key('k'));
        let r = engine.process_key(key('k'));
        assert_eq!(r, EngineResponse::Commit("っ".into()));
        assert_eq!(engine.composition_text(), Some("k".to_string()));

        assert_eq!(
            engine.process_key(key('a')),
            EngineResponse::Commit("か".into())
        );
        assert_eq!(engine.composition_text(), None);
    }

    #[test]
    fn test_direct_n_before_consonant() {
        let mut engine = hiragana_engine();
        // "kan" → "か" + pending "n"
        engine.process_key(key('k'));
        engine.process_key(key('a'));
        engine.process_key(key('n'));
        assert_eq!(engine.composition_text(), Some("n".to_string()));

        // "nk" → "ん" + pending "k"
        let r = engine.process_key(key('k'));
        assert_eq!(r, EngineResponse::Commit("ん".into()));
        assert_eq!(engine.composition_text(), Some("k".to_string()));
    }

    #[test]
    fn test_direct_enter_flushes_n() {
        let mut engine = hiragana_engine();
        engine.process_key(key('n'));
        assert_eq!(
            engine.process_key(enter()),
            EngineResponse::Commit("ん".into())
        );
    }

    #[test]
    fn test_direct_backspace_pending() {
        let mut engine = hiragana_engine();
        engine.process_key(key('k'));
        assert_eq!(engine.composition_text(), Some("k".to_string()));

        engine.process_key(backspace());
        assert_eq!(engine.composition_text(), None);
    }

    #[test]
    fn test_direct_passthrough() {
        let mut engine = hiragana_engine();
        assert_eq!(engine.process_key(space()), EngineResponse::PassThrough);
    }

    // === モード切り替えテスト ===

    #[test]
    fn test_l_to_ascii() {
        let mut engine = hiragana_engine();
        engine.process_key(key('l'));
        assert_eq!(engine.current_mode(), InputMode::Ascii);
    }

    #[test]
    fn test_l_flushes_pending_n() {
        let mut engine = hiragana_engine();
        engine.process_key(key('n'));
        let r = engine.process_key(key('l'));
        assert_eq!(r, EngineResponse::Commit("ん".into()));
        assert_eq!(engine.current_mode(), InputMode::Ascii);
    }

    #[test]
    fn test_shift_l_to_zenkaku() {
        let mut engine = hiragana_engine();
        engine.process_key(shift_key('L'));
        assert_eq!(engine.current_mode(), InputMode::ZenkakuAscii);
    }

    // === カタカナモードテスト ===

    fn katakana_engine() -> SkkEngine {
        let mut engine = SkkEngine::new();
        engine.input_mode = InputMode::Katakana;
        engine
    }

    #[test]
    fn test_q_toggle_hiragana_to_katakana() {
        let mut engine = hiragana_engine();
        let r = engine.process_key(key('q'));
        assert_eq!(r, EngineResponse::Consumed);
        assert_eq!(engine.current_mode(), InputMode::Katakana);
    }

    #[test]
    fn test_q_toggle_katakana_to_hiragana() {
        let mut engine = katakana_engine();
        let r = engine.process_key(key('q'));
        assert_eq!(r, EngineResponse::Consumed);
        assert_eq!(engine.current_mode(), InputMode::Hiragana);
    }

    #[test]
    fn test_katakana_basic_input() {
        let mut engine = katakana_engine();
        // "ka" → "カ"
        engine.process_key(key('k'));
        let r = engine.process_key(key('a'));
        assert_eq!(r, EngineResponse::Commit("カ".into()));
    }

    #[test]
    fn test_katakana_sokuon() {
        let mut engine = katakana_engine();
        engine.process_key(key('k'));
        let r = engine.process_key(key('k'));
        assert_eq!(r, EngineResponse::Commit("ッ".into()));

        let r = engine.process_key(key('a'));
        assert_eq!(r, EngineResponse::Commit("カ".into()));
    }

    #[test]
    fn test_katakana_enter_flushes_n() {
        let mut engine = katakana_engine();
        engine.process_key(key('n'));
        let r = engine.process_key(enter());
        assert_eq!(r, EngineResponse::Commit("ン".into()));
    }

    #[test]
    fn test_q_toggle_flushes_pending_as_current_mode() {
        // ひらがなモードで "n" → q → "ん" がひらがなで確定、カタカナモードへ
        let mut engine = hiragana_engine();
        engine.process_key(key('n'));
        let r = engine.process_key(key('q'));
        assert_eq!(r, EngineResponse::Commit("ん".into()));
        assert_eq!(engine.current_mode(), InputMode::Katakana);
    }

    #[test]
    fn test_q_toggle_from_katakana_flushes_as_katakana() {
        // カタカナモードで "n" → q → "ン" がカタカナで確定、ひらがなモードへ
        let mut engine = katakana_engine();
        engine.process_key(key('n'));
        let r = engine.process_key(key('q'));
        assert_eq!(r, EngineResponse::Commit("ン".into()));
        assert_eq!(engine.current_mode(), InputMode::Hiragana);
    }

    #[test]
    fn test_katakana_precomp_same_as_hiragana() {
        // カタカナモードでも▽モードの読みはひらがな
        let mut engine = katakana_engine();
        engine.process_key(shift_key('K'));
        engine.process_key(key('a'));
        assert_eq!(engine.composition_text(), Some("▽か".to_string()));
    }

    #[test]
    fn test_katakana_l_flushes_as_katakana() {
        let mut engine = katakana_engine();
        engine.process_key(key('n'));
        let r = engine.process_key(key('l'));
        assert_eq!(r, EngineResponse::Commit("ン".into()));
        assert_eq!(engine.current_mode(), InputMode::Ascii);
    }

    #[test]
    fn test_katakana_confirm_and_continue() {
        // カタカナモードで▼確定後の続き入力もカタカナ
        let mut engine = katakana_engine();
        engine.add_dictionary(test_dict());
        engine.process_key(shift_key('K'));
        engine.process_key(key('a'));
        engine.process_key(key('n'));
        engine.process_key(key('j'));
        engine.process_key(key('i'));
        engine.process_key(space()); // ▼漢字

        let r = engine.process_key(key('a'));
        // 漢字（辞書候補はそのまま）+ ア（カタカナ）
        assert_eq!(r, EngineResponse::Commit("漢字ア".into()));
    }

    // === PreComposition モード（▽モード）テスト ===

    #[test]
    fn test_start_precomposition() {
        let mut engine = hiragana_engine();
        let r = engine.process_key(shift_key('K'));
        assert_eq!(
            r,
            EngineResponse::UpdateComposition {
                display: "▽k".into(),
                candidates: None
            }
        );
    }

    #[test]
    fn test_precomposition_reading() {
        let mut engine = hiragana_engine();
        engine.process_key(shift_key('K'));
        engine.process_key(key('a'));
        assert_eq!(engine.composition_text(), Some("▽か".to_string()));

        engine.process_key(key('n'));
        assert_eq!(engine.composition_text(), Some("▽かn".to_string()));

        engine.process_key(key('j'));
        assert_eq!(engine.composition_text(), Some("▽かんj".to_string()));

        engine.process_key(key('i'));
        assert_eq!(engine.composition_text(), Some("▽かんじ".to_string()));
    }

    #[test]
    fn test_precomposition_cancel() {
        let mut engine = hiragana_engine();
        engine.process_key(shift_key('K'));
        engine.process_key(key('a'));
        engine.process_key(ctrl_key('g'));
        assert_eq!(engine.composition_text(), None);
        assert!(matches!(engine.composition, CompositionState::Direct));
    }

    #[test]
    fn test_precomposition_cancel_escape() {
        let mut engine = hiragana_engine();
        engine.process_key(shift_key('K'));
        engine.process_key(key('a'));
        engine.process_key(escape());
        assert_eq!(engine.composition_text(), None);
    }

    #[test]
    fn test_precomposition_confirm_hiragana() {
        let mut engine = hiragana_engine();
        engine.process_key(shift_key('K'));
        engine.process_key(key('a'));
        engine.process_key(key('n'));
        engine.process_key(key('j'));
        engine.process_key(key('i'));

        let r = engine.process_key(ctrl_key('j'));
        assert_eq!(r, EngineResponse::Commit("かんじ".into()));
        assert!(matches!(engine.composition, CompositionState::Direct));
    }

    #[test]
    fn test_precomposition_confirm_enter() {
        let mut engine = hiragana_engine();
        engine.process_key(shift_key('T'));
        engine.process_key(key('e'));
        engine.process_key(key('s'));
        engine.process_key(key('u'));
        engine.process_key(key('t'));
        engine.process_key(key('o'));

        let r = engine.process_key(enter());
        assert_eq!(r, EngineResponse::Commit("てすと".into()));
    }

    #[test]
    fn test_precomposition_katakana() {
        let mut engine = hiragana_engine();
        engine.process_key(shift_key('K'));
        engine.process_key(key('a'));
        engine.process_key(key('t'));
        engine.process_key(key('a'));
        engine.process_key(key('k'));
        engine.process_key(key('a'));
        engine.process_key(key('n'));
        engine.process_key(key('a'));

        let r = engine.process_key(key('q'));
        assert_eq!(r, EngineResponse::Commit("カタカナ".into()));
    }

    #[test]
    fn test_precomposition_backspace_pending() {
        let mut engine = hiragana_engine();
        engine.process_key(shift_key('K'));
        engine.process_key(key('a'));
        engine.process_key(key('s'));
        assert_eq!(engine.composition_text(), Some("▽かs".to_string()));

        engine.process_key(backspace());
        assert_eq!(engine.composition_text(), Some("▽か".to_string()));
    }

    #[test]
    fn test_precomposition_backspace_reading() {
        let mut engine = hiragana_engine();
        engine.process_key(shift_key('K'));
        engine.process_key(key('a'));
        assert_eq!(engine.composition_text(), Some("▽か".to_string()));

        engine.process_key(backspace());
        // 読みが空 → キャンセル
        assert_eq!(engine.composition_text(), None);
    }

    // === Conversion モード（▼モード）テスト ===

    #[test]
    fn test_conversion_basic() {
        let mut engine = engine_with_dict();
        // "Kanji" + Space
        engine.process_key(shift_key('K'));
        engine.process_key(key('a'));
        engine.process_key(key('n'));
        engine.process_key(key('j'));
        engine.process_key(key('i'));

        let r = engine.process_key(space());
        assert_eq!(
            r,
            EngineResponse::UpdateComposition {
                display: "▼漢字".into(),
                candidates: Some(vec!["漢字".into(), "感じ".into(), "幹事".into()])
            }
        );
    }

    #[test]
    fn test_conversion_next_candidate() {
        let mut engine = engine_with_dict();
        engine.process_key(shift_key('K'));
        engine.process_key(key('a'));
        engine.process_key(key('n'));
        engine.process_key(key('j'));
        engine.process_key(key('i'));
        engine.process_key(space()); // ▼漢字

        engine.process_key(space()); // next → 感じ
        assert_eq!(engine.composition_text(), Some("▼感じ".to_string()));

        engine.process_key(space()); // next → 幹事
        assert_eq!(engine.composition_text(), Some("▼幹事".to_string()));
    }

    #[test]
    fn test_conversion_prev_candidate() {
        let mut engine = engine_with_dict();
        engine.process_key(shift_key('K'));
        engine.process_key(key('a'));
        engine.process_key(key('n'));
        engine.process_key(key('j'));
        engine.process_key(key('i'));
        engine.process_key(space()); // ▼漢字
        engine.process_key(space()); // ▼感じ

        engine.process_key(key('x')); // prev → 漢字
        assert_eq!(engine.composition_text(), Some("▼漢字".to_string()));
    }

    #[test]
    fn test_conversion_prev_at_first_returns_to_precomp() {
        let mut engine = engine_with_dict();
        engine.process_key(shift_key('K'));
        engine.process_key(key('a'));
        engine.process_key(key('n'));
        engine.process_key(key('j'));
        engine.process_key(key('i'));
        engine.process_key(space()); // ▼漢字

        engine.process_key(key('x')); // prev at first → ▽かんじ
        assert_eq!(engine.composition_text(), Some("▽かんじ".to_string()));
    }

    #[test]
    fn test_conversion_confirm_enter() {
        let mut engine = engine_with_dict();
        engine.process_key(shift_key('K'));
        engine.process_key(key('a'));
        engine.process_key(key('n'));
        engine.process_key(key('j'));
        engine.process_key(key('i'));
        engine.process_key(space());

        let r = engine.process_key(enter());
        assert_eq!(r, EngineResponse::Commit("漢字".into()));
        assert!(matches!(engine.composition, CompositionState::Direct));
    }

    #[test]
    fn test_conversion_confirm_ctrl_j() {
        let mut engine = engine_with_dict();
        engine.process_key(shift_key('K'));
        engine.process_key(key('a'));
        engine.process_key(key('n'));
        engine.process_key(key('j'));
        engine.process_key(key('i'));
        engine.process_key(space());
        engine.process_key(space()); // 感じ

        let r = engine.process_key(ctrl_key('j'));
        assert_eq!(r, EngineResponse::Commit("感じ".into()));
    }

    #[test]
    fn test_conversion_cancel() {
        let mut engine = engine_with_dict();
        engine.process_key(shift_key('K'));
        engine.process_key(key('a'));
        engine.process_key(key('n'));
        engine.process_key(key('j'));
        engine.process_key(key('i'));
        engine.process_key(space());

        engine.process_key(ctrl_key('g'));
        assert_eq!(engine.composition_text(), Some("▽かんじ".to_string()));
    }

    #[test]
    fn test_conversion_confirm_and_continue() {
        let mut engine = engine_with_dict();
        // "Kanji" → 変換 → "a" で確定して "あ" を入力
        engine.process_key(shift_key('K'));
        engine.process_key(key('a'));
        engine.process_key(key('n'));
        engine.process_key(key('j'));
        engine.process_key(key('i'));
        engine.process_key(space());

        let r = engine.process_key(key('a'));
        assert_eq!(r, EngineResponse::Commit("漢字あ".into()));
    }

    #[test]
    fn test_conversion_confirm_and_new_precomp() {
        let mut engine = engine_with_dict();
        engine.process_key(shift_key('K'));
        engine.process_key(key('a'));
        engine.process_key(key('n'));
        engine.process_key(key('j'));
        engine.process_key(key('i'));
        engine.process_key(space());

        // Shift+H で確定して新しい▽モード
        let r = engine.process_key(shift_key('H'));
        assert_eq!(r, EngineResponse::Commit("漢字".into()));
        assert_eq!(engine.composition_text(), Some("▽h".to_string()));
    }

    #[test]
    fn test_conversion_not_found() {
        let mut engine = engine_with_dict();
        engine.process_key(shift_key('Z'));
        engine.process_key(key('z'));
        engine.process_key(key('z'));

        let r = engine.process_key(space());
        // 候補なし → Consumed、▽モードのまま
        assert_eq!(r, EngineResponse::Consumed);
        assert!(matches!(
            engine.composition,
            CompositionState::PreComposition { .. }
        ));
    }

    // === 送り仮名テスト ===

    #[test]
    fn test_okuri_basic() {
        let mut engine = engine_with_dict();
        // "OoKi" → おお + 送り k + i → "き" → 辞書引き "おおk"
        engine.process_key(shift_key('O'));
        engine.process_key(key('o'));
        assert_eq!(engine.composition_text(), Some("▽おお".to_string()));

        engine.process_key(shift_key('K')); // 送り仮名開始
        assert_eq!(engine.composition_text(), Some("▽おおk".to_string()));

        let r = engine.process_key(key('i')); // "ki" → "き" → 辞書引き
        assert_eq!(
            r,
            EngineResponse::UpdateComposition {
                display: "▼大き".into(),
                candidates: Some(vec!["大".into(), "多".into()])
            }
        );
    }

    #[test]
    fn test_okuri_confirm() {
        let mut engine = engine_with_dict();
        engine.process_key(shift_key('O'));
        engine.process_key(key('o'));
        engine.process_key(shift_key('K'));
        engine.process_key(key('i'));

        let r = engine.process_key(enter());
        assert_eq!(r, EngineResponse::Commit("大き".into()));
    }

    #[test]
    fn test_okuri_confirm_and_continue() {
        let mut engine = engine_with_dict();
        // "OoKi" → ▼大き → "i" で確定して "い" を追加
        engine.process_key(shift_key('O'));
        engine.process_key(key('o'));
        engine.process_key(shift_key('K'));
        engine.process_key(key('i'));

        let r = engine.process_key(key('i'));
        assert_eq!(r, EngineResponse::Commit("大きい".into()));
    }

    // === 統合テスト ===

    #[test]
    fn test_full_workflow_toukyou() {
        // ASCII モードから開始して Ctrl-Space で IME ON
        let mut engine = SkkEngine::new();
        engine.add_dictionary(test_dict());

        engine.process_key(ctrl_space());
        assert_eq!(engine.current_mode(), InputMode::Hiragana);

        // "Toukyou" → ▽とうきょう
        engine.process_key(shift_key('T'));
        engine.process_key(key('o'));
        engine.process_key(key('u'));
        engine.process_key(key('k'));
        engine.process_key(key('y'));
        engine.process_key(key('o'));
        engine.process_key(key('u'));
        assert_eq!(
            engine.composition_text(),
            Some("▽とうきょう".to_string())
        );

        // Space で変換
        let r = engine.process_key(space());
        assert_eq!(
            r,
            EngineResponse::UpdateComposition {
                display: "▼東京".into(),
                candidates: Some(vec!["東京".into()])
            }
        );

        // Enter で確定
        let r = engine.process_key(enter());
        assert_eq!(r, EngineResponse::Commit("東京".into()));
        assert_eq!(engine.composition_text(), None);
    }

    #[test]
    fn test_full_workflow_with_okuri() {
        let mut engine = engine_with_dict();
        engine.input_mode = InputMode::Hiragana;

        // "KaKi" → 書き
        engine.process_key(shift_key('K'));
        engine.process_key(key('a'));
        engine.process_key(shift_key('K'));
        engine.process_key(key('i'));
        assert_eq!(engine.composition_text(), Some("▼書き".to_string()));

        // "masu" で確定して "ます" を続けて入力
        let r = engine.process_key(key('m'));
        assert_eq!(r, EngineResponse::Commit("書き".into()));
        // "m" は pending
        assert_eq!(engine.composition_text(), Some("m".to_string()));

        engine.process_key(key('a'));
        engine.process_key(key('s'));
        engine.process_key(key('u'));
        // "ます" が確定されている（各 Commit で）
    }

    #[test]
    fn test_hiragana_to_katakana() {
        assert_eq!(hiragana_to_katakana("かたかな"), "カタカナ");
        assert_eq!(hiragana_to_katakana("とうきょう"), "トウキョウ");
        assert_eq!(hiragana_to_katakana("abc"), "abc"); // ASCII はそのまま
        assert_eq!(hiragana_to_katakana(""), "");
    }
}
