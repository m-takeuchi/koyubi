//! 設定ファイル管理
//!
//! `%APPDATA%\Koyubi\config.toml` を読み込み、エンジンの動作をカスタマイズする。
//! ファイルがない場合やパースエラー時はデフォルト値を使用する。

use std::path::Path;

use serde::Deserialize;

use crate::InputMode;

/// Koyubi 設定
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    /// SandS (Space and Shift) 機能の有効/無効
    pub sands_enabled: bool,
    /// Emacs キーバインド (Ctrl+F/B/A/E/N/P/D/K) の有効/無効
    pub emacs_bindings_enabled: bool,
    /// システム辞書パス（空の場合は自動検出）
    pub system_dict_paths: Vec<String>,
    /// ユーザー辞書パス（None の場合は %APPDATA%\Koyubi\dict\user-dict.skk）
    pub user_dict_path: Option<String>,
    /// カタカナ変換キー
    pub toggle_kana: char,
    /// ASCII モード切替キー
    pub enter_ascii: char,
    /// 全角英数モード切替キー（大文字で指定）
    pub enter_zenkaku: char,
    /// 前候補キー
    pub prev_candidate: char,
    /// 起動時の入力モード
    pub initial_mode: InputMode,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            sands_enabled: true,
            emacs_bindings_enabled: true,
            system_dict_paths: Vec::new(),
            user_dict_path: None,
            toggle_kana: 'q',
            enter_ascii: 'l',
            enter_zenkaku: 'L',
            prev_candidate: 'x',
            initial_mode: InputMode::Ascii,
        }
    }
}

impl Config {
    /// TOML ファイルから設定を読み込む。
    ///
    /// ファイルが存在しない場合やパースに失敗した場合はデフォルト値を返す。
    pub fn load(path: &Path) -> Self {
        let content = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(_) => return Self::default(),
        };
        Self::from_str(&content)
    }

    /// TOML 文字列から設定をパースする。
    ///
    /// パースに失敗した場合はデフォルト値を返す。
    pub fn from_str(s: &str) -> Self {
        toml::from_str(s).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_values() {
        let config = Config::default();
        assert!(config.sands_enabled);
        assert!(config.emacs_bindings_enabled);
        assert!(config.system_dict_paths.is_empty());
        assert!(config.user_dict_path.is_none());
        assert_eq!(config.toggle_kana, 'q');
        assert_eq!(config.enter_ascii, 'l');
        assert_eq!(config.enter_zenkaku, 'L');
        assert_eq!(config.prev_candidate, 'x');
        assert_eq!(config.initial_mode, InputMode::Ascii);
    }

    #[test]
    fn empty_string_returns_default() {
        let config = Config::from_str("");
        assert!(config.sands_enabled);
        assert_eq!(config.toggle_kana, 'q');
        assert_eq!(config.initial_mode, InputMode::Ascii);
    }

    #[test]
    fn partial_toml() {
        let config = Config::from_str("sands_enabled = false");
        assert!(!config.sands_enabled);
        // 他はデフォルト
        assert!(config.emacs_bindings_enabled);
        assert_eq!(config.toggle_kana, 'q');
    }

    #[test]
    fn full_toml() {
        let toml = r#"
sands_enabled = false
emacs_bindings_enabled = false
system_dict_paths = ["/path/to/SKK-JISYO.L", "/path/to/SKK-JISYO.M"]
user_dict_path = "/path/to/user-dict.skk"
toggle_kana = "z"
enter_ascii = ";"
enter_zenkaku = ":"
prev_candidate = "b"
initial_mode = "hiragana"
"#;
        let config = Config::from_str(toml);
        assert!(!config.sands_enabled);
        assert!(!config.emacs_bindings_enabled);
        assert_eq!(config.system_dict_paths, vec![
            "/path/to/SKK-JISYO.L",
            "/path/to/SKK-JISYO.M",
        ]);
        assert_eq!(config.user_dict_path.as_deref(), Some("/path/to/user-dict.skk"));
        assert_eq!(config.toggle_kana, 'z');
        assert_eq!(config.enter_ascii, ';');
        assert_eq!(config.enter_zenkaku, ':');
        assert_eq!(config.prev_candidate, 'b');
        assert_eq!(config.initial_mode, InputMode::Hiragana);
    }

    #[test]
    fn invalid_toml_returns_default() {
        let config = Config::from_str("this is not valid toml {{{}}}");
        assert!(config.sands_enabled);
        assert_eq!(config.toggle_kana, 'q');
    }

    #[test]
    fn missing_file_returns_default() {
        let config = Config::load(Path::new("/nonexistent/path/config.toml"));
        assert!(config.sands_enabled);
        assert_eq!(config.toggle_kana, 'q');
    }

    #[test]
    fn initial_mode_variants() {
        for (toml_val, expected) in [
            ("ascii", InputMode::Ascii),
            ("hiragana", InputMode::Hiragana),
            ("katakana", InputMode::Katakana),
            ("hankaku_katakana", InputMode::HankakuKatakana),
            ("zenkaku_ascii", InputMode::ZenkakuAscii),
        ] {
            let toml = format!("initial_mode = \"{}\"", toml_val);
            let config = Config::from_str(&toml);
            assert_eq!(config.initial_mode, expected, "failed for {}", toml_val);
        }
    }

    #[test]
    fn dict_paths_empty_default() {
        let config = Config::from_str("system_dict_paths = []");
        assert!(config.system_dict_paths.is_empty());
    }

    #[test]
    fn unknown_fields_ignored() {
        let config = Config::from_str("unknown_field = 42\nsands_enabled = false");
        assert!(!config.sands_enabled);
    }
}
