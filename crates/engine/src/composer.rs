//! SKK 入力状態管理（コンポーザー）
//!
//! SkkEngine がキーイベントを受け取り、入力モードと変換状態に応じて
//! ローマ字変換・辞書検索・候補選択を行う。

use std::path::PathBuf;
use std::sync::Arc;

use crate::config::Config;
use crate::dict::{DictEntry, Dictionary};
use crate::romaji::{RomajiConverter, RomajiTable};
use crate::{
    Candidate, CandidateDisplay, CandidateInfo, CompositionState, EngineResponse, InputMode, Key,
    KeyEvent,
};

/// 辞書登録モードの保存状態（ネスト変換用）
struct SavedRegistration {
    reading: String,
    okuri: Option<String>,
    word: String,
    last_lookup_key: Option<String>,
}

/// SKK 変換エンジン
pub struct SkkEngine {
    config: Config,
    input_mode: InputMode,
    composition: CompositionState,
    romaji: RomajiConverter,
    romaji_table: RomajiTable,
    dictionaries: Vec<Arc<Dictionary>>,
    /// 送り仮名の子音プレフィックス（送りあり変換時に使用）
    okuri_prefix: Option<String>,
    /// ユーザー辞書
    user_dict: Option<Dictionary>,
    /// ユーザー辞書ファイルパス
    user_dict_path: Option<PathBuf>,
    /// 辞書登録時のルックアップキー保持用
    last_lookup_key: Option<String>,
    /// ネスト変換時に保存される登録モードの状態
    saved_registration: Option<SavedRegistration>,
    /// 登録モード中のリテラル入力サブモード（l: Ascii, L: ZenkakuAscii）
    registration_sub_mode: Option<InputMode>,
}

impl SkkEngine {
    /// 設定を指定してエンジンを生成
    pub fn new(config: Config) -> Self {
        let initial_mode = config.initial_mode;
        Self {
            config,
            input_mode: initial_mode,
            composition: CompositionState::Direct,
            romaji: RomajiConverter::new(),
            romaji_table: RomajiTable::default_table(),
            dictionaries: Vec::new(),
            okuri_prefix: None,
            user_dict: None,
            user_dict_path: None,
            last_lookup_key: None,
            saved_registration: None,
            registration_sub_mode: None,
        }
    }

    /// 設定への参照を取得
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// 辞書を追加（Arc で共有可能）
    pub fn add_dictionary(&mut self, dict: Arc<Dictionary>) {
        self.dictionaries.push(dict);
    }

    /// 辞書を追加（非共有、テスト用）
    pub fn add_dictionary_owned(&mut self, dict: Dictionary) {
        self.dictionaries.push(Arc::new(dict));
    }

    /// ユーザー辞書を設定（ファイルがなければ空辞書で作成）
    pub fn set_user_dictionary(&mut self, path: PathBuf) {
        let dict = Dictionary::load(&path).unwrap_or_default();
        self.user_dict = Some(dict);
        self.user_dict_path = Some(path);
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
        let base = match &self.composition {
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
            CompositionState::Registration {
                reading,
                word,
                pending_roman,
                ..
            } => Some(format!("{}{}[登録:{}]", word, pending_roman, reading)),
        };

        // ネスト変換中は登録プレフィックスを付加
        if let Some(saved) = &self.saved_registration {
            let inner = base.unwrap_or_default();
            Some(format!("{}{}[登録:{}]", saved.word, inner, saved.reading))
        } else {
            base
        }
    }

    /// 候補ウィンドウ表示用の情報を取得
    ///
    /// Conversion 状態のときのみ Some を返す。
    pub fn candidate_info(&self) -> Option<CandidateInfo> {
        if let CompositionState::Conversion {
            candidates,
            selected,
            ..
        } = &self.composition
        {
            Some(CandidateInfo {
                candidates: candidates
                    .iter()
                    .map(|c| CandidateDisplay {
                        word: c.word.clone(),
                        annotation: c.annotation.clone(),
                    })
                    .collect(),
                selected: *selected,
            })
        } else {
            None
        }
    }

    /// 現在のコンポジション状態への参照を取得
    pub fn composition_state(&self) -> &CompositionState {
        &self.composition
    }

    /// ローマ字バッファにペンディング入力があるか
    pub fn has_pending_romaji(&self) -> bool {
        !self.romaji.pending().is_empty()
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
            InputMode::ZenkakuAscii => self.handle_zenkaku_ascii(key),
            _ => EngineResponse::PassThrough,
        }
    }

    fn handle_hiragana(&mut self, key: KeyEvent) -> EngineResponse {
        let response = match self.composition.clone() {
            CompositionState::Direct => self.handle_direct(key),
            CompositionState::PreComposition { .. } => self.handle_pre_composition(key),
            CompositionState::Conversion { .. } => self.handle_conversion(key),
            CompositionState::Registration { .. } => self.handle_registration(key),
        };
        self.intercept_for_registration(response)
    }

    /// ネスト変換の結果を登録モードにフィードバック
    fn intercept_for_registration(&mut self, response: EngineResponse) -> EngineResponse {
        if self.saved_registration.is_none() {
            return response;
        }

        match response {
            EngineResponse::Commit(ref text) => {
                let saved = self.saved_registration.as_mut().unwrap();
                saved.word.push_str(text);

                if matches!(self.composition, CompositionState::PreComposition { .. }) {
                    // confirm_and_start_precomp: ネスト内で新しい▽モード開始
                    // saved_registration は維持
                    self.make_composition_response()
                } else {
                    // 通常確定: Registration モードに復帰
                    let saved = self.saved_registration.take().unwrap();
                    let pending = self.romaji.pending().to_string();
                    self.last_lookup_key = saved.last_lookup_key;
                    self.composition = CompositionState::Registration {
                        reading: saved.reading,
                        okuri: saved.okuri,
                        word: saved.word,
                        pending_roman: pending,
                    };
                    self.make_composition_response()
                }
            }
            _ => {
                // キャンセル等で Direct に戻った場合、Registration に復帰
                if matches!(self.composition, CompositionState::Direct) {
                    let saved = self.saved_registration.take().unwrap();
                    self.last_lookup_key = saved.last_lookup_key;
                    self.composition = CompositionState::Registration {
                        reading: saved.reading,
                        okuri: saved.okuri,
                        word: saved.word,
                        pending_roman: String::new(),
                    };
                    self.make_composition_response()
                } else {
                    response
                }
            }
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
        // Ctrl+H: Backspace と同じ
        let is_backspace =
            key.key == Key::Backspace || (key.ctrl && key.key == Key::Char('h'));

        // Ctrl 系
        if key.ctrl && !is_backspace {
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
            // enter_ascii: ASCII モード
            Key::Char(ch) if !key.shift && *ch == self.config.enter_ascii => {
                let flushed = self.romaji.flush();
                let flushed = self.kana_output(flushed);
                self.input_mode = InputMode::Ascii;
                if flushed.is_empty() {
                    EngineResponse::Consumed
                } else {
                    EngineResponse::Commit(flushed)
                }
            }

            // enter_zenkaku: 全角英数モード（▽モード開始より優先）
            Key::Char(ch) if key.shift && *ch == self.config.enter_zenkaku => {
                let flushed = self.romaji.flush();
                let flushed = self.kana_output(flushed);
                self.input_mode = InputMode::ZenkakuAscii;
                if flushed.is_empty() {
                    EngineResponse::Consumed
                } else {
                    EngineResponse::Commit(flushed)
                }
            }

            // toggle_kana: ひらがな/カタカナモード切替
            Key::Char(ch) if !key.shift && !key.ctrl && *ch == self.config.toggle_kana => {
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
            _ if is_backspace => {
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

        // toggle_kana: カタカナ変換
        if !key.ctrl && !key.shift && key.key == Key::Char(self.config.toggle_kana) {
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

        self.last_lookup_key = Some(lookup_key.clone());

        let candidates = self.lookup_all_dicts(&lookup_key);
        if candidates.is_empty() {
            if self.saved_registration.is_some() {
                // ネスト中は二重登録しない: ▽モードに戻る
                self.composition = CompositionState::PreComposition {
                    reading,
                    pending_roman: String::new(),
                };
                return EngineResponse::Consumed;
            }
            // 候補なし → 登録モード
            self.composition = CompositionState::Registration {
                reading,
                okuri: Some(okuri_kana),
                word: String::new(),
                pending_roman: String::new(),
            };
            self.make_composition_response()
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

        self.last_lookup_key = Some(reading.clone());

        let candidates = self.lookup_all_dicts(&reading);
        if candidates.is_empty() {
            if self.saved_registration.is_some() {
                // ネスト中は二重登録しない: ▽モードのまま
                return EngineResponse::Consumed;
            }
            // 候補なし → 登録モード
            self.composition = CompositionState::Registration {
                reading,
                okuri: None,
                word: String::new(),
                pending_roman: String::new(),
            };
            self.make_composition_response()
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

            // prev_candidate: 前候補
            Key::Char(ch) if !key.ctrl && !key.shift && *ch == self.config.prev_candidate => self.prev_candidate(),

            // Enter: 確定
            Key::Enter => self.confirm_candidate(),

            // Shift + 英字: 確定して新しい▽モード開始
            Key::Char(ch) if key.shift && ch.is_ascii_uppercase() => {
                self.confirm_and_start_precomp(ch.to_ascii_lowercase())
            }

            // 数字キー (1-9): 候補を直接選択して確定
            Key::Char(ch) if ('1'..='9').contains(ch) && !key.ctrl => {
                self.select_candidate_by_number((*ch as u32 - '0' as u32) as usize)
            }

            // 英小文字: 確定して次の入力を開始
            Key::Char(ch) if ch.is_ascii_lowercase() && !key.ctrl => {
                self.confirm_and_continue(*ch)
            }

            _ => EngineResponse::PassThrough,
        }
    }

    fn next_candidate(&mut self) -> EngineResponse {
        let enter_registration = if let CompositionState::Conversion {
            selected,
            candidates,
            ..
        } = &mut self.composition
        {
            if *selected + 1 < candidates.len() {
                *selected += 1;
                false
            } else if self.saved_registration.is_some() {
                false // ネスト中は二重登録しない: 最後の候補にとどまる
            } else {
                true // 最後の候補の次 → 登録モード
            }
        } else {
            false
        };

        if enter_registration {
            if let CompositionState::Conversion {
                reading, okuri, ..
            } = &self.composition
            {
                let reading = reading.clone();
                let okuri = okuri.clone();
                self.composition = CompositionState::Registration {
                    reading,
                    okuri,
                    word: String::new(),
                    pending_roman: String::new(),
                };
            }
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

    fn select_candidate_by_number(&mut self, number: usize) -> EngineResponse {
        if let CompositionState::Conversion {
            candidates,
            selected,
            okuri,
            ..
        } = &self.composition
        {
            // ページ内の番号 → 絶対インデックス
            let page_start = (*selected / 9) * 9;
            let absolute = page_start + (number - 1);
            if let Some(candidate) = candidates.get(absolute) {
                let okuri_str = okuri.as_deref().unwrap_or("");
                let text = format!("{}{}", candidate.word, okuri_str);
                self.composition = CompositionState::Direct;
                self.okuri_prefix = None;
                return EngineResponse::Commit(text);
            }
        }
        // 範囲外の番号 → 無視
        EngineResponse::Consumed
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
    // ZenkakuAscii モード（全角英数）
    // =========================================================

    fn handle_zenkaku_ascii(&mut self, key: KeyEvent) -> EngineResponse {
        if key.ctrl {
            return match &key.key {
                Key::Char('j') => {
                    self.input_mode = InputMode::Hiragana;
                    EngineResponse::Consumed
                }
                _ => EngineResponse::PassThrough,
            };
        }

        match &key.key {
            // enter_ascii: ASCII モードへ
            Key::Char(ch) if !key.shift && *ch == self.config.enter_ascii => {
                self.input_mode = InputMode::Ascii;
                EngineResponse::Consumed
            }
            // toggle_kana: ひらがなモードへ
            Key::Char(ch) if !key.shift && *ch == self.config.toggle_kana => {
                self.input_mode = InputMode::Hiragana;
                EngineResponse::Consumed
            }
            // Space → 全角スペース
            Key::Space => EngineResponse::Commit("\u{3000}".to_string()),
            // 印字可能文字 → 全角変換
            Key::Char(ch) if !ch.is_control() => {
                EngineResponse::Commit(to_fullwidth(*ch).to_string())
            }
            // Enter, Backspace, Escape, Tab → PassThrough
            _ => EngineResponse::PassThrough,
        }
    }

    // =========================================================
    // Registration モード（辞書登録）
    // =========================================================

    fn handle_registration(&mut self, key: KeyEvent) -> EngineResponse {
        // リテラル入力サブモード中はサブモードハンドラに委譲
        if self.registration_sub_mode.is_some() {
            return self.handle_registration_literal(key);
        }

        // Ctrl-J / Enter: 登録確定
        if (key.ctrl && key.key == Key::Char('j')) || key.key == Key::Enter {
            return self.confirm_registration();
        }

        // Ctrl-G / Escape: キャンセル → ▽モードに戻る
        if (key.ctrl && key.key == Key::Char('g')) || key.key == Key::Escape {
            return self.cancel_registration();
        }

        // Backspace
        if key.key == Key::Backspace {
            return self.registration_backspace();
        }

        // enter_ascii: ASCII リテラル入力サブモード開始
        if !key.shift && !key.ctrl && key.key == Key::Char(self.config.enter_ascii) {
            self.romaji.clear();
            if let CompositionState::Registration { pending_roman, .. } = &mut self.composition {
                *pending_roman = String::new();
            }
            self.registration_sub_mode = Some(InputMode::Ascii);
            return self.make_composition_response();
        }

        // enter_zenkaku: 全角英数リテラル入力サブモード開始
        if key.shift && key.key == Key::Char(self.config.enter_zenkaku) {
            self.romaji.clear();
            if let CompositionState::Registration { pending_roman, .. } = &mut self.composition {
                *pending_roman = String::new();
            }
            self.registration_sub_mode = Some(InputMode::ZenkakuAscii);
            return self.make_composition_response();
        }

        // Shift + 英字: ネスト▽モード開始
        if key.shift {
            if let Key::Char(ch) = key.key {
                if ch.is_ascii_uppercase() {
                    return self.start_nested_precomp(ch.to_ascii_lowercase());
                }
            }
        }

        // 英小文字・記号・数字: ローマ字入力（テーブルにない文字はそのまま追加）
        if let Key::Char(ch) = key.key {
            if !key.ctrl {
                return self.feed_registration_char(ch);
            }
        }

        EngineResponse::Consumed
    }

    fn start_nested_precomp(&mut self, ch: char) -> EngineResponse {
        // pending のローマ字を登録テキストにフラッシュ
        let flushed = self.romaji.flush();
        if let CompositionState::Registration {
            reading,
            okuri,
            word,
            ..
        } = &mut self.composition
        {
            word.push_str(&flushed);

            // 登録状態を保存
            self.saved_registration = Some(SavedRegistration {
                reading: reading.clone(),
                okuri: okuri.clone(),
                word: word.clone(),
                last_lookup_key: self.last_lookup_key.take(),
            });
        }

        // ▽モードに遷移
        self.composition = CompositionState::PreComposition {
            reading: String::new(),
            pending_roman: String::new(),
        };

        let output = self.romaji.feed(ch, &self.romaji_table);
        self.apply_romaji_to_precomp(&output.committed, &output.pending);

        self.make_composition_response()
    }

    fn feed_registration_char(&mut self, ch: char) -> EngineResponse {
        let output = self.romaji.feed(ch, &self.romaji_table);
        if let CompositionState::Registration {
            word,
            pending_roman,
            ..
        } = &mut self.composition
        {
            if output.committed.is_empty() && output.pending.is_empty() {
                // ローマ字テーブルにマッチしない文字 → そのまま追加
                word.push(ch);
            } else {
                word.push_str(&output.committed);
                *pending_roman = output.pending;
            }
        }
        self.make_composition_response()
    }


    fn confirm_registration(&mut self) -> EngineResponse {
        self.registration_sub_mode = None;
        let flushed = self.romaji.flush();
        if let CompositionState::Registration {
            reading,
            okuri,
            word,
            ..
        } = &self.composition
        {
            let mut word = word.clone();
            word.push_str(&flushed);

            if word.is_empty() {
                // 空の単語 → キャンセルと同じ
                return self.cancel_registration();
            }

            let reading = reading.clone();
            let okuri = okuri.clone();

            // ユーザー辞書に登録
            if let Some(lookup_key) = &self.last_lookup_key {
                self.register_word(lookup_key.clone(), word.clone());
            }

            // 登録した単語 + 送り仮名を確定出力
            let okuri_str = okuri.as_deref().unwrap_or("");
            let commit_text = format!("{}{}", word, okuri_str);

            self.composition = CompositionState::Direct;
            self.okuri_prefix = None;
            self.last_lookup_key = None;
            let _ = reading; // suppress unused warning

            return EngineResponse::Commit(commit_text);
        }
        EngineResponse::Consumed
    }

    /// 登録モード中の ASCII/全角英数リテラル入力サブモード
    fn handle_registration_literal(&mut self, key: KeyEvent) -> EngineResponse {
        let sub_mode = self.registration_sub_mode.unwrap();

        // Ctrl-J: サブモード終了 → 通常のローマ字登録入力に戻る
        if key.ctrl && key.key == Key::Char('j') {
            self.registration_sub_mode = None;
            return self.make_composition_response();
        }

        // Enter: 登録確定
        if key.key == Key::Enter {
            self.registration_sub_mode = None;
            return self.confirm_registration();
        }

        // Ctrl-G / Escape: キャンセル
        if (key.ctrl && key.key == Key::Char('g')) || key.key == Key::Escape {
            self.registration_sub_mode = None;
            return self.cancel_registration();
        }

        // Backspace
        if key.key == Key::Backspace {
            return self.registration_backspace();
        }

        // Space
        if key.key == Key::Space {
            let ch = if sub_mode == InputMode::ZenkakuAscii {
                '\u{3000}' // 全角スペース
            } else {
                ' '
            };
            if let CompositionState::Registration { word, .. } = &mut self.composition {
                word.push(ch);
            }
            return self.make_composition_response();
        }

        // 印字可能文字 → リテラル追加（全角英数なら全角変換）
        if let Key::Char(ch) = key.key {
            if !key.ctrl {
                let ch = if sub_mode == InputMode::ZenkakuAscii {
                    to_fullwidth(ch)
                } else {
                    ch
                };
                if let CompositionState::Registration { word, .. } = &mut self.composition {
                    word.push(ch);
                }
                return self.make_composition_response();
            }
        }

        EngineResponse::Consumed
    }

    fn cancel_registration(&mut self) -> EngineResponse {
        self.romaji.clear();
        self.registration_sub_mode = None;
        self.saved_registration = None;
        if let CompositionState::Registration { reading, .. } = &self.composition {
            let reading = reading.clone();
            self.composition = CompositionState::PreComposition {
                reading,
                pending_roman: String::new(),
            };
            self.okuri_prefix = None;
        }
        self.make_composition_response()
    }

    fn registration_backspace(&mut self) -> EngineResponse {
        // まず pending のローマ字を削除
        if self.romaji.backspace() {
            if let CompositionState::Registration { pending_roman, .. } = &mut self.composition {
                *pending_roman = self.romaji.pending().to_string();
            }
            return self.make_composition_response();
        }
        // 次に word の文字を削除
        if let CompositionState::Registration { word, .. } = &mut self.composition {
            word.pop();
        }
        self.make_composition_response()
    }

    fn register_word(&mut self, dict_key: String, word: String) {
        if let Some(user_dict) = &mut self.user_dict {
            user_dict.add_entry(
                &dict_key,
                DictEntry {
                    word,
                    annotation: None,
                },
            );
            if let Some(path) = &self.user_dict_path {
                let _ = user_dict.save(path);
            }
        }
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
        // ユーザー辞書を最優先
        if let Some(user_dict) = &self.user_dict {
            if let Some(entries) = user_dict.lookup(reading) {
                for entry in entries {
                    results.push(Candidate {
                        word: entry.word.clone(),
                        annotation: entry.annotation.clone(),
                    });
                }
            }
        }
        for dict in &self.dictionaries {
            if let Some(entries) = dict.lookup(reading) {
                for entry in entries {
                    // ユーザー辞書と重複する候補をスキップ
                    if !results.iter().any(|r| r.word == entry.word) {
                        results.push(Candidate {
                            word: entry.word.clone(),
                            annotation: entry.annotation.clone(),
                        });
                    }
                }
            }
        }
        results
    }

    pub fn reset_state(&mut self) {
        self.composition = CompositionState::Direct;
        self.romaji.clear();
        self.okuri_prefix = None;
        self.last_lookup_key = None;
        self.saved_registration = None;
        self.registration_sub_mode = None;
    }

    fn cancel_composition(&mut self) -> EngineResponse {
        self.composition = CompositionState::Direct;
        self.romaji.clear();
        self.okuri_prefix = None;
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
        Self::new(Config::default())
    }
}

/// ASCII 文字を全角に変換 (0x21-0x7E → U+FF01-U+FF5E)
fn to_fullwidth(ch: char) -> char {
    let cp = ch as u32;
    if (0x21..=0x7E).contains(&cp) {
        char::from_u32(cp + 0xFEE0).unwrap_or(ch)
    } else {
        ch
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
        let mut engine = SkkEngine::default();
        engine.input_mode = InputMode::Hiragana;
        engine
    }

    fn engine_with_dict() -> SkkEngine {
        let mut engine = hiragana_engine();
        engine.add_dictionary_owned(test_dict());
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
        let mut engine = SkkEngine::default();
        assert_eq!(engine.current_mode(), InputMode::Ascii);

        assert_eq!(engine.process_key(ctrl_space()), EngineResponse::Consumed);
        assert_eq!(engine.current_mode(), InputMode::Hiragana);

        assert_eq!(engine.process_key(ctrl_space()), EngineResponse::Consumed);
        assert_eq!(engine.current_mode(), InputMode::Ascii);
    }

    #[test]
    fn test_ctrl_j_ime_on() {
        let mut engine = SkkEngine::default();
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
        let mut engine = SkkEngine::default();
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
        let mut engine = SkkEngine::default();
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
        engine.add_dictionary_owned(test_dict());
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

    // === ZenkakuAscii モードテスト ===

    fn zenkaku_engine() -> SkkEngine {
        let mut engine = SkkEngine::default();
        engine.input_mode = InputMode::ZenkakuAscii;
        engine
    }

    #[test]
    fn test_zenkaku_ascii_basic() {
        let mut engine = zenkaku_engine();
        assert_eq!(
            engine.process_key(key('a')),
            EngineResponse::Commit("\u{FF41}".into()) // ａ
        );
        assert_eq!(
            engine.process_key(key('z')),
            EngineResponse::Commit("\u{FF5A}".into()) // ｚ
        );
    }

    #[test]
    fn test_zenkaku_ascii_uppercase() {
        let mut engine = zenkaku_engine();
        assert_eq!(
            engine.process_key(shift_key('A')),
            EngineResponse::Commit("\u{FF21}".into()) // Ａ
        );
        assert_eq!(
            engine.process_key(shift_key('Z')),
            EngineResponse::Commit("\u{FF3A}".into()) // Ｚ
        );
    }

    #[test]
    fn test_zenkaku_ascii_digits() {
        let mut engine = zenkaku_engine();
        assert_eq!(
            engine.process_key(key('0')),
            EngineResponse::Commit("\u{FF10}".into()) // ０
        );
        assert_eq!(
            engine.process_key(key('9')),
            EngineResponse::Commit("\u{FF19}".into()) // ９
        );
    }

    #[test]
    fn test_zenkaku_ascii_symbols() {
        let mut engine = zenkaku_engine();
        assert_eq!(
            engine.process_key(key('!')),
            EngineResponse::Commit("\u{FF01}".into()) // ！
        );
        assert_eq!(
            engine.process_key(key('~')),
            EngineResponse::Commit("\u{FF5E}".into()) // ～
        );
    }

    #[test]
    fn test_zenkaku_ascii_space() {
        let mut engine = zenkaku_engine();
        assert_eq!(
            engine.process_key(space()),
            EngineResponse::Commit("\u{3000}".into()) // 全角スペース
        );
    }

    #[test]
    fn test_zenkaku_l_to_ascii() {
        let mut engine = zenkaku_engine();
        assert_eq!(engine.process_key(key('l')), EngineResponse::Consumed);
        assert_eq!(engine.current_mode(), InputMode::Ascii);
    }

    #[test]
    fn test_zenkaku_q_to_hiragana() {
        let mut engine = zenkaku_engine();
        assert_eq!(engine.process_key(key('q')), EngineResponse::Consumed);
        assert_eq!(engine.current_mode(), InputMode::Hiragana);
    }

    #[test]
    fn test_zenkaku_ctrl_j_to_hiragana() {
        let mut engine = zenkaku_engine();
        assert_eq!(engine.process_key(ctrl_key('j')), EngineResponse::Consumed);
        assert_eq!(engine.current_mode(), InputMode::Hiragana);
    }

    #[test]
    fn test_zenkaku_passthrough_keys() {
        let mut engine = zenkaku_engine();
        assert_eq!(engine.process_key(enter()), EngineResponse::PassThrough);
        assert_eq!(engine.process_key(backspace()), EngineResponse::PassThrough);
        assert_eq!(engine.process_key(escape()), EngineResponse::PassThrough);
    }

    #[test]
    fn test_enter_zenkaku_from_hiragana() {
        let mut engine = hiragana_engine();
        engine.process_key(shift_key('L'));
        assert_eq!(engine.current_mode(), InputMode::ZenkakuAscii);

        assert_eq!(
            engine.process_key(key('a')),
            EngineResponse::Commit("\u{FF41}".into())
        );
    }

    #[test]
    fn test_zenkaku_roundtrip() {
        let mut engine = zenkaku_engine();
        // ZenkakuAscii → l → Ascii → Ctrl-Space → Hiragana → L → ZenkakuAscii
        engine.process_key(key('l'));
        assert_eq!(engine.current_mode(), InputMode::Ascii);
        engine.process_key(ctrl_space());
        assert_eq!(engine.current_mode(), InputMode::Hiragana);
        engine.process_key(shift_key('L'));
        assert_eq!(engine.current_mode(), InputMode::ZenkakuAscii);
    }

    #[test]
    fn test_to_fullwidth() {
        assert_eq!(to_fullwidth('a'), '\u{FF41}');
        assert_eq!(to_fullwidth('A'), '\u{FF21}');
        assert_eq!(to_fullwidth('0'), '\u{FF10}');
        assert_eq!(to_fullwidth('!'), '\u{FF01}');
        assert_eq!(to_fullwidth('~'), '\u{FF5E}');
        // ASCII 範囲外はそのまま
        assert_eq!(to_fullwidth(' '), ' ');
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
    fn test_conversion_not_found_enters_registration() {
        let mut engine = engine_with_dict();
        engine.process_key(shift_key('Z'));
        engine.process_key(key('z'));
        engine.process_key(key('z'));

        let r = engine.process_key(space());
        // 候補なし → 登録モードに遷移
        assert!(matches!(r, EngineResponse::UpdateComposition { .. }));
        assert!(matches!(
            engine.composition,
            CompositionState::Registration { .. }
        ));
        assert_eq!(
            engine.composition_text(),
            Some("[登録:っっ]".to_string())
        );
    }

    // === 数字キー候補選択テスト ===

    #[test]
    fn test_number_key_select_candidate() {
        let mut engine = engine_with_dict();
        engine.process_key(shift_key('K'));
        engine.process_key(key('a'));
        engine.process_key(key('n'));
        engine.process_key(key('j'));
        engine.process_key(key('i'));
        engine.process_key(space()); // ▼漢字

        // '1' → 最初の候補（漢字）を確定
        let r = engine.process_key(key('1'));
        assert_eq!(r, EngineResponse::Commit("漢字".into()));
        assert!(matches!(engine.composition, CompositionState::Direct));
    }

    #[test]
    fn test_number_key_select_second() {
        let mut engine = engine_with_dict();
        engine.process_key(shift_key('K'));
        engine.process_key(key('a'));
        engine.process_key(key('n'));
        engine.process_key(key('j'));
        engine.process_key(key('i'));
        engine.process_key(space()); // ▼漢字

        // '2' → 2番目の候補（感じ）を確定
        let r = engine.process_key(key('2'));
        assert_eq!(r, EngineResponse::Commit("感じ".into()));
    }

    #[test]
    fn test_number_key_select_third() {
        let mut engine = engine_with_dict();
        engine.process_key(shift_key('K'));
        engine.process_key(key('a'));
        engine.process_key(key('n'));
        engine.process_key(key('j'));
        engine.process_key(key('i'));
        engine.process_key(space()); // ▼漢字

        // '3' → 3番目の候補（幹事）を確定
        let r = engine.process_key(key('3'));
        assert_eq!(r, EngineResponse::Commit("幹事".into()));
    }

    #[test]
    fn test_number_key_out_of_range() {
        let mut engine = engine_with_dict();
        engine.process_key(shift_key('K'));
        engine.process_key(key('a'));
        engine.process_key(key('n'));
        engine.process_key(key('j'));
        engine.process_key(key('i'));
        engine.process_key(space()); // ▼漢字（3候補しかない）

        // '5' → 範囲外なので Consumed、状態変わらず
        let r = engine.process_key(key('5'));
        assert_eq!(r, EngineResponse::Consumed);
        assert!(matches!(
            engine.composition,
            CompositionState::Conversion { .. }
        ));
    }

    #[test]
    fn test_number_key_with_okuri() {
        let mut engine = engine_with_dict();
        // "OoKi" → ▼大き
        engine.process_key(shift_key('O'));
        engine.process_key(key('o'));
        engine.process_key(shift_key('K'));
        engine.process_key(key('i'));

        // '2' → 2番目の候補（多）+ 送り仮名（き）
        let r = engine.process_key(key('2'));
        assert_eq!(r, EngineResponse::Commit("多き".into()));
    }

    #[test]
    fn test_number_key_paged_selection() {
        // 12候補ある辞書を作り、ページ2の候補を数字キーで選択
        let mut engine = hiragana_engine();
        let dict = Dictionary::from_str(
            "てすと /A/B/C/D/E/F/G/H/I/J/K/L/\n",
        );
        engine.add_dictionary_owned(dict);

        engine.process_key(shift_key('T'));
        engine.process_key(key('e'));
        engine.process_key(key('s'));
        engine.process_key(key('u'));
        engine.process_key(key('t'));
        engine.process_key(key('o'));
        engine.process_key(space()); // selected=0 (page 0)

        // Space 9回で selected=9 (page 1)
        for _ in 0..9 {
            engine.process_key(space());
        }
        let info = engine.candidate_info().unwrap();
        assert_eq!(info.selected, 9);

        // ページ1で '1' → 候補[9] = "J" を確定
        let r = engine.process_key(key('1'));
        assert_eq!(r, EngineResponse::Commit("J".into()));
    }

    #[test]
    fn test_number_key_page_boundary() {
        let mut engine = hiragana_engine();
        let dict = Dictionary::from_str(
            "てすと /A/B/C/D/E/F/G/H/I/J/K/L/\n",
        );
        engine.add_dictionary_owned(dict);

        engine.process_key(shift_key('T'));
        engine.process_key(key('e'));
        engine.process_key(key('s'));
        engine.process_key(key('u'));
        engine.process_key(key('t'));
        engine.process_key(key('o'));
        engine.process_key(space()); // selected=0

        // selected=8 (page 0 の最後)
        for _ in 0..8 {
            engine.process_key(space());
        }
        // ページ0で '9' → 候補[8] = "I"
        let r = engine.process_key(key('9'));
        assert_eq!(r, EngineResponse::Commit("I".into()));
    }

    // === candidate_info テスト ===

    #[test]
    fn test_candidate_info_none_in_direct() {
        let engine = hiragana_engine();
        assert!(engine.candidate_info().is_none());
    }

    #[test]
    fn test_candidate_info_none_in_precomp() {
        let mut engine = hiragana_engine();
        engine.process_key(shift_key('K'));
        engine.process_key(key('a'));
        assert!(engine.candidate_info().is_none());
    }

    #[test]
    fn test_candidate_info_in_conversion() {
        let mut engine = engine_with_dict();
        engine.process_key(shift_key('K'));
        engine.process_key(key('a'));
        engine.process_key(key('n'));
        engine.process_key(key('j'));
        engine.process_key(key('i'));
        engine.process_key(space());

        let info = engine.candidate_info().expect("should have candidates");
        assert_eq!(info.selected, 0);
        assert_eq!(info.candidates.len(), 3);
        assert_eq!(info.candidates[0].word, "漢字");
        assert_eq!(info.candidates[1].word, "感じ");
        assert_eq!(info.candidates[2].word, "幹事");
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
        let mut engine = SkkEngine::default();
        engine.add_dictionary_owned(test_dict());

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

    // === Registration モード（辞書登録）テスト ===

    #[test]
    fn test_registration_no_candidates() {
        // 候補なしで Space → 登録モードに遷移
        let mut engine = engine_with_dict();
        engine.process_key(shift_key('Z'));
        engine.process_key(key('z'));
        engine.process_key(key('z'));
        engine.process_key(space());

        assert!(matches!(
            engine.composition,
            CompositionState::Registration { .. }
        ));
    }

    #[test]
    fn test_registration_past_last_candidate() {
        // 最終候補を超えて Space → 登録モードに遷移
        let mut engine = hiragana_engine();
        let dict = Dictionary::from_str("ひと /人/\n");
        engine.add_dictionary_owned(dict);

        engine.process_key(shift_key('H'));
        engine.process_key(key('i'));
        engine.process_key(key('t'));
        engine.process_key(key('o'));
        engine.process_key(space()); // ▼人
        assert_eq!(engine.composition_text(), Some("▼人".to_string()));

        engine.process_key(space()); // 最後の候補を超える → 登録モード
        assert!(matches!(
            engine.composition,
            CompositionState::Registration { .. }
        ));
        assert_eq!(
            engine.composition_text(),
            Some("[登録:ひと]".to_string())
        );
    }

    #[test]
    fn test_registration_romaji_input() {
        // 登録モードでローマ字入力
        let mut engine = engine_with_dict();
        engine.process_key(shift_key('Z'));
        engine.process_key(key('z'));
        engine.process_key(key('z'));
        engine.process_key(space()); // → 登録モード

        engine.process_key(key('k'));
        engine.process_key(key('a'));
        engine.process_key(key('n'));
        engine.process_key(key('j'));
        engine.process_key(key('i'));

        assert_eq!(
            engine.composition_text(),
            Some("かんじ[登録:っっ]".to_string())
        );
    }

    #[test]
    fn test_registration_confirm_enter() {
        // Enter で確定 → Commit + 辞書登録
        let mut engine = hiragana_engine();
        engine.user_dict = Some(Dictionary::new());
        engine.process_key(shift_key('Z'));
        engine.process_key(key('z'));
        engine.process_key(key('z'));
        engine.process_key(space()); // → 登録モード

        engine.process_key(key('t'));
        engine.process_key(key('e'));
        engine.process_key(key('s'));
        engine.process_key(key('u'));
        engine.process_key(key('t'));
        engine.process_key(key('o'));

        let r = engine.process_key(enter());
        assert_eq!(r, EngineResponse::Commit("てすと".into()));
        assert!(matches!(engine.composition, CompositionState::Direct));

        // ユーザー辞書に登録されている
        let user_dict = engine.user_dict.as_ref().unwrap();
        let entries = user_dict.lookup("っっ").expect("should be registered");
        assert_eq!(entries[0].word, "てすと");
    }

    #[test]
    fn test_registration_confirm_ctrl_j() {
        let mut engine = hiragana_engine();
        engine.user_dict = Some(Dictionary::new());
        engine.process_key(shift_key('Z'));
        engine.process_key(key('z'));
        engine.process_key(key('z'));
        engine.process_key(space());

        engine.process_key(key('a'));
        let r = engine.process_key(ctrl_key('j'));
        assert_eq!(r, EngineResponse::Commit("あ".into()));
    }

    #[test]
    fn test_registration_cancel_escape() {
        // Escape でキャンセル → ▽モードに戻る
        let mut engine = engine_with_dict();
        engine.process_key(shift_key('Z'));
        engine.process_key(key('z'));
        engine.process_key(key('z'));
        engine.process_key(space());

        engine.process_key(key('a'));
        engine.process_key(escape());

        assert!(matches!(
            engine.composition,
            CompositionState::PreComposition { .. }
        ));
        assert_eq!(
            engine.composition_text(),
            Some("▽っっ".to_string())
        );
    }

    #[test]
    fn test_registration_cancel_ctrl_g() {
        let mut engine = engine_with_dict();
        engine.process_key(shift_key('Z'));
        engine.process_key(key('z'));
        engine.process_key(key('z'));
        engine.process_key(space());

        engine.process_key(ctrl_key('g'));
        assert!(matches!(
            engine.composition,
            CompositionState::PreComposition { .. }
        ));
    }

    #[test]
    fn test_registration_backspace() {
        // Backspace: pending → word の順に削除
        let mut engine = hiragana_engine();
        engine.process_key(shift_key('Z'));
        engine.process_key(key('z'));
        engine.process_key(key('z'));
        engine.process_key(space());

        engine.process_key(key('a')); // "あ"
        engine.process_key(key('k')); // pending "k"
        assert_eq!(
            engine.composition_text(),
            Some("あk[登録:っっ]".to_string())
        );

        engine.process_key(backspace()); // pending "k" 削除
        assert_eq!(
            engine.composition_text(),
            Some("あ[登録:っっ]".to_string())
        );

        engine.process_key(backspace()); // word "あ" 削除
        assert_eq!(
            engine.composition_text(),
            Some("[登録:っっ]".to_string())
        );
    }

    #[test]
    fn test_registration_empty_word_confirm() {
        // 空の単語で確定 → キャンセルと同じ
        let mut engine = hiragana_engine();
        engine.process_key(shift_key('Z'));
        engine.process_key(key('z'));
        engine.process_key(key('z'));
        engine.process_key(space());

        engine.process_key(enter()); // 空で確定
        assert!(matches!(
            engine.composition,
            CompositionState::PreComposition { .. }
        ));
    }

    #[test]
    fn test_registered_word_appears_in_conversion() {
        // 登録した単語が次の変換で候補に出る
        let mut engine = hiragana_engine();
        engine.user_dict = Some(Dictionary::new());

        // "zzz" で登録
        engine.process_key(shift_key('Z'));
        engine.process_key(key('z'));
        engine.process_key(key('z'));
        engine.process_key(space());
        engine.process_key(key('t'));
        engine.process_key(key('e'));
        engine.process_key(key('s'));
        engine.process_key(key('u'));
        engine.process_key(key('t'));
        engine.process_key(key('o'));
        engine.process_key(enter()); // 登録

        // もう一度同じ読みで変換
        engine.process_key(shift_key('Z'));
        engine.process_key(key('z'));
        engine.process_key(key('z'));
        let r = engine.process_key(space());

        // 登録した "てすと" が候補に出る
        assert!(matches!(r, EngineResponse::UpdateComposition { .. }));
        assert_eq!(engine.composition_text(), Some("▼てすと".to_string()));
    }

    #[test]
    fn test_user_dict_priority_over_system() {
        // ユーザー辞書がシステム辞書より優先される
        let mut engine = engine_with_dict();
        let mut user_dict = Dictionary::new();
        user_dict.add_entry(
            "かんじ",
            DictEntry {
                word: "幹事".to_string(),
                annotation: None,
            },
        );
        engine.user_dict = Some(user_dict);

        engine.process_key(shift_key('K'));
        engine.process_key(key('a'));
        engine.process_key(key('n'));
        engine.process_key(key('j'));
        engine.process_key(key('i'));
        engine.process_key(space());

        // ユーザー辞書の "幹事" が最初の候補
        assert_eq!(engine.composition_text(), Some("▼幹事".to_string()));
    }

    #[test]
    fn test_registration_okuri_confirm() {
        // 送りあり変換で候補なし → 登録 → 確定時に送り仮名付き
        let mut engine = hiragana_engine();
        engine.user_dict = Some(Dictionary::new());

        // "ZzKi" → 送りあり変換、候補なし → 登録モード
        engine.process_key(shift_key('Z'));
        engine.process_key(key('z'));
        engine.process_key(shift_key('K'));
        engine.process_key(key('i'));

        assert!(matches!(
            engine.composition,
            CompositionState::Registration { .. }
        ));

        // 登録文字を入力
        engine.process_key(key('a'));
        let r = engine.process_key(enter());
        // "あ" + 送り仮名 "き" で確定
        assert_eq!(r, EngineResponse::Commit("あき".into()));
    }

    // === Registration ネスト変換テスト ===

    #[test]
    fn test_registration_nested_precomp() {
        // 登録モードで Shift+K → ネスト▽モード表示
        let mut engine = engine_with_dict();
        engine.process_key(shift_key('Z'));
        engine.process_key(key('z'));
        engine.process_key(key('z'));
        engine.process_key(space()); // → [登録:っっ]

        engine.process_key(shift_key('K'));
        assert_eq!(
            engine.composition_text(),
            Some("▽k[登録:っっ]".to_string())
        );
    }

    #[test]
    fn test_registration_nested_conversion_confirm() {
        // 登録モードでネスト変換して確定 → 結果が登録テキストに追加
        let mut engine = engine_with_dict();
        engine.process_key(shift_key('Z'));
        engine.process_key(key('z'));
        engine.process_key(key('z'));
        engine.process_key(space()); // → [登録:っっ]

        // Shift+K → ▽ → "kanji" → Space → ▼漢字 → Enter
        engine.process_key(shift_key('K'));
        engine.process_key(key('a'));
        engine.process_key(key('n'));
        engine.process_key(key('j'));
        engine.process_key(key('i'));
        engine.process_key(space()); // ネスト▼漢字
        assert_eq!(
            engine.composition_text(),
            Some("▼漢字[登録:っっ]".to_string())
        );

        engine.process_key(enter()); // 確定 → 登録テキストに追加
        assert!(matches!(
            engine.composition,
            CompositionState::Registration { .. }
        ));
        assert_eq!(
            engine.composition_text(),
            Some("漢字[登録:っっ]".to_string())
        );
    }

    #[test]
    fn test_registration_nested_confirm_and_continue() {
        // ネスト▼モードで英字打ち → 確定して登録テキストに追加
        let mut engine = engine_with_dict();
        engine.process_key(shift_key('Z'));
        engine.process_key(key('z'));
        engine.process_key(key('z'));
        engine.process_key(space());

        engine.process_key(shift_key('K'));
        engine.process_key(key('a'));
        engine.process_key(key('n'));
        engine.process_key(key('j'));
        engine.process_key(key('i'));
        engine.process_key(space()); // ▼漢字

        engine.process_key(key('a')); // 確定 + "あ" → "漢字あ" が登録テキストに
        assert!(matches!(
            engine.composition,
            CompositionState::Registration { .. }
        ));
        assert_eq!(
            engine.composition_text(),
            Some("漢字あ[登録:っっ]".to_string())
        );
    }

    #[test]
    fn test_registration_nested_cancel_precomp() {
        // ネスト▽でEscape → 登録モードに戻る
        let mut engine = engine_with_dict();
        engine.process_key(shift_key('Z'));
        engine.process_key(key('z'));
        engine.process_key(key('z'));
        engine.process_key(space());

        engine.process_key(shift_key('K'));
        engine.process_key(key('a'));
        engine.process_key(escape()); // ▽キャンセル → 登録モードに復帰

        assert!(matches!(
            engine.composition,
            CompositionState::Registration { .. }
        ));
        assert_eq!(
            engine.composition_text(),
            Some("[登録:っっ]".to_string())
        );
    }

    #[test]
    fn test_registration_nested_cancel_conversion() {
        // ネスト▼でEscape → ネスト▽に戻る（登録には戻らない）
        let mut engine = engine_with_dict();
        engine.process_key(shift_key('Z'));
        engine.process_key(key('z'));
        engine.process_key(key('z'));
        engine.process_key(space());

        engine.process_key(shift_key('K'));
        engine.process_key(key('a'));
        engine.process_key(key('n'));
        engine.process_key(key('j'));
        engine.process_key(key('i'));
        engine.process_key(space()); // ▼漢字

        engine.process_key(escape()); // ▼キャンセル → ▽かんじ（まだネスト内）
        assert!(matches!(
            engine.composition,
            CompositionState::PreComposition { .. }
        ));
        assert_eq!(
            engine.composition_text(),
            Some("▽かんじ[登録:っっ]".to_string())
        );
    }

    #[test]
    fn test_registration_cancel_clears_saved() {
        // ネスト変換後に登録キャンセル → saved_registration がクリアされ
        // [登録:よみ] がバッファに残らない
        let mut engine = engine_with_dict();
        engine.process_key(shift_key('Z'));
        engine.process_key(key('z'));
        engine.process_key(key('z'));
        engine.process_key(space()); // 候補なし → 登録モード

        // ネスト変換を開始して戻す
        engine.process_key(shift_key('K'));
        engine.process_key(key('a'));
        engine.process_key(escape()); // ネスト▽キャンセル → 登録モードに復帰

        assert!(matches!(
            engine.composition,
            CompositionState::Registration { .. }
        ));

        // 登録モードをキャンセル → ▽モードに戻る
        engine.process_key(escape());
        assert!(matches!(
            engine.composition,
            CompositionState::PreComposition { .. }
        ));
        // [登録:っっ] が残らないことを確認
        assert_eq!(
            engine.composition_text(),
            Some("▽っっ".to_string())
        );
    }

    #[test]
    fn test_registration_nested_hiragana_confirm() {
        // ネスト▽で Enter → ひらがなのまま登録テキストに追加
        let mut engine = engine_with_dict();
        engine.process_key(shift_key('Z'));
        engine.process_key(key('z'));
        engine.process_key(key('z'));
        engine.process_key(space());

        engine.process_key(shift_key('T'));
        engine.process_key(key('e'));
        engine.process_key(key('s'));
        engine.process_key(key('u'));
        engine.process_key(key('t'));
        engine.process_key(key('o'));
        engine.process_key(enter()); // ひらがな確定

        assert!(matches!(
            engine.composition,
            CompositionState::Registration { .. }
        ));
        assert_eq!(
            engine.composition_text(),
            Some("てすと[登録:っっ]".to_string())
        );
    }

    #[test]
    fn test_registration_nested_multiple_conversions() {
        // 複数回ネスト変換: 漢字→登録に戻る→もう一度変換
        let mut engine = engine_with_dict();
        engine.process_key(shift_key('Z'));
        engine.process_key(key('z'));
        engine.process_key(key('z'));
        engine.process_key(space());

        // 1回目: "漢字"
        engine.process_key(shift_key('K'));
        engine.process_key(key('a'));
        engine.process_key(key('n'));
        engine.process_key(key('j'));
        engine.process_key(key('i'));
        engine.process_key(space());
        engine.process_key(enter());
        assert_eq!(
            engine.composition_text(),
            Some("漢字[登録:っっ]".to_string())
        );

        // 2回目: "東京"
        engine.process_key(shift_key('T'));
        engine.process_key(key('o'));
        engine.process_key(key('u'));
        engine.process_key(key('k'));
        engine.process_key(key('y'));
        engine.process_key(key('o'));
        engine.process_key(key('u'));
        engine.process_key(space());
        engine.process_key(enter());
        assert_eq!(
            engine.composition_text(),
            Some("漢字東京[登録:っっ]".to_string())
        );
    }

    #[test]
    fn test_registration_nested_final_confirm() {
        // ネスト変換後に最終確定 → Commit
        let mut engine = engine_with_dict();
        engine.user_dict = Some(Dictionary::new());

        engine.process_key(shift_key('Z'));
        engine.process_key(key('z'));
        engine.process_key(key('z'));
        engine.process_key(space());

        // ネスト変換で "漢字" を登録テキストに追加
        engine.process_key(shift_key('K'));
        engine.process_key(key('a'));
        engine.process_key(key('n'));
        engine.process_key(key('j'));
        engine.process_key(key('i'));
        engine.process_key(space());
        engine.process_key(enter());

        // 最終確定
        let r = engine.process_key(enter());
        assert_eq!(r, EngineResponse::Commit("漢字".into()));
        assert!(matches!(engine.composition, CompositionState::Direct));

        // ユーザー辞書に登録されている
        let user_dict = engine.user_dict.as_ref().unwrap();
        let entries = user_dict.lookup("っっ").expect("should be registered");
        assert_eq!(entries[0].word, "漢字");
    }

    #[test]
    fn test_registration_literal_chars() {
        // 登録モードで記号やローマ字テーブルにない文字
        let mut engine = hiragana_engine();
        engine.user_dict = Some(Dictionary::new());
        engine.process_key(shift_key('Z'));
        engine.process_key(key('z'));
        engine.process_key(key('z'));
        engine.process_key(space()); // → 登録モード

        // 'l' で ASCII サブモード → 文字を直接入力
        engine.process_key(key('l'));
        engine.process_key(key('o'));
        engine.process_key(key('l'));
        assert_eq!(
            engine.composition_text(),
            Some("ol[登録:っっ]".to_string())
        );

        // Ctrl+J でサブモード終了
        engine.process_key(ctrl_key('j'));

        // 通常のローマ字入力
        engine.process_key(key('a'));
        assert_eq!(
            engine.composition_text(),
            Some("olあ[登録:っっ]".to_string())
        );
    }

    #[test]
    fn test_registration_l_ascii_submode() {
        // 登録モードで l → ASCII サブモード → リテラル入力
        let mut engine = hiragana_engine();
        engine.user_dict = Some(Dictionary::new());
        engine.process_key(shift_key('Z'));
        engine.process_key(key('z'));
        engine.process_key(key('z'));
        engine.process_key(space()); // → 登録モード

        engine.process_key(key('l')); // ASCII サブモード開始
        assert!(engine.registration_sub_mode.is_some());

        // 'i', 'n' はローマ字変換されず、そのまま追加
        engine.process_key(key('L'));
        engine.process_key(key('i'));
        engine.process_key(key('n'));
        engine.process_key(key('u'));
        engine.process_key(key('x'));
        assert_eq!(
            engine.composition_text(),
            Some("Linux[登録:っっ]".to_string())
        );

        // Ctrl+J でサブモード終了 → 通常ローマ字入力に復帰
        engine.process_key(ctrl_key('j'));
        assert!(engine.registration_sub_mode.is_none());
        // 以降はローマ字入力
        engine.process_key(key('a'));
        assert_eq!(
            engine.composition_text(),
            Some("Linuxあ[登録:っっ]".to_string())
        );
    }

    #[test]
    fn test_registration_shift_l_zenkaku_submode() {
        // 登録モードで Shift+L → 全角英数サブモード
        let mut engine = hiragana_engine();
        engine.process_key(shift_key('Z'));
        engine.process_key(key('z'));
        engine.process_key(key('z'));
        engine.process_key(space());

        engine.process_key(shift_key('L')); // 全角英数サブモード
        assert_eq!(engine.registration_sub_mode, Some(InputMode::ZenkakuAscii));

        engine.process_key(key('A'));
        engine.process_key(key('B'));
        assert_eq!(
            engine.composition_text(),
            Some("ＡＢ[登録:っっ]".to_string())
        );
    }

    #[test]
    fn test_registration_symbols_and_digits() {
        // 登録モードで記号・数字を入力
        let mut engine = hiragana_engine();
        engine.process_key(shift_key('Z'));
        engine.process_key(key('z'));
        engine.process_key(key('z'));
        engine.process_key(space());

        // 数字と記号はローマ字テーブルにない → そのまま追加
        engine.process_key(KeyEvent {
            key: Key::Char('1'),
            shift: false,
            ctrl: false,
            alt: false,
        });
        engine.process_key(KeyEvent {
            key: Key::Char('+'),
            shift: false,
            ctrl: false,
            alt: false,
        });
        assert_eq!(
            engine.composition_text(),
            Some("1+[登録:っっ]".to_string())
        );
    }

    #[test]
    fn test_hiragana_to_katakana() {
        assert_eq!(hiragana_to_katakana("かたかな"), "カタカナ");
        assert_eq!(hiragana_to_katakana("とうきょう"), "トウキョウ");
        assert_eq!(hiragana_to_katakana("abc"), "abc"); // ASCII はそのまま
        assert_eq!(hiragana_to_katakana(""), "");
    }

}
