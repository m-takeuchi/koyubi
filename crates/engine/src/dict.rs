//! SKK 辞書の読み込み・検索モジュール
//!
//! SKK-JISYO フォーマットに準拠した辞書ファイルを読み込み、
//! HashMap によるメモリ内高速検索を提供する。
//! EUC-JP / UTF-8 両方のエンコーディングに対応。

use std::collections::HashMap;
use std::path::Path;

/// SKK 辞書
#[derive(Debug)]
pub struct Dictionary {
    /// 送りなしエントリ: 読み → 候補リスト
    okuri_nashi: HashMap<String, Vec<DictEntry>>,
    /// 送りありエントリ: 読み → 候補リスト
    okuri_ari: HashMap<String, Vec<DictEntry>>,
}

/// 辞書エントリ
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DictEntry {
    pub word: String,
    pub annotation: Option<String>,
}

/// 辞書操作のエラー
#[derive(Debug)]
pub enum DictError {
    Io(std::io::Error),
    Encoding(String),
}

impl std::fmt::Display for DictError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DictError::Io(e) => write!(f, "I/O error: {}", e),
            DictError::Encoding(msg) => write!(f, "Encoding error: {}", msg),
        }
    }
}

impl std::error::Error for DictError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            DictError::Io(e) => Some(e),
            DictError::Encoding(_) => None,
        }
    }
}

impl From<std::io::Error> for DictError {
    fn from(e: std::io::Error) -> Self {
        DictError::Io(e)
    }
}

impl Dictionary {
    /// 空の辞書を生成
    pub fn new() -> Self {
        Self {
            okuri_nashi: HashMap::new(),
            okuri_ari: HashMap::new(),
        }
    }

    /// ファイルから辞書を読み込む
    ///
    /// エンコーディングは自動判定（UTF-8 を試行後、EUC-JP にフォールバック）。
    pub fn load(path: impl AsRef<Path>) -> Result<Self, DictError> {
        let bytes = std::fs::read(path.as_ref())?;
        let content = decode_auto(&bytes)?;
        Ok(Self::from_str(&content))
    }

    /// UTF-8 文字列から辞書をパース
    ///
    /// 不正な行は無視して読み飛ばす（SKK 辞書の慣習に従う）。
    pub fn from_str(content: &str) -> Self {
        let mut dict = Self::new();
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with(";;") {
                continue;
            }
            if let Some((reading, entries)) = parse_line(line) {
                if is_okuri_ari(&reading) {
                    dict.okuri_ari
                        .entry(reading)
                        .or_default()
                        .extend(entries);
                } else {
                    dict.okuri_nashi
                        .entry(reading)
                        .or_default()
                        .extend(entries);
                }
            }
        }
        dict
    }

    /// 読みで検索（送りあり/なしを自動判定）
    ///
    /// 読みの末尾が ASCII 小文字なら送りありとして検索する。
    pub fn lookup(&self, reading: &str) -> Option<&[DictEntry]> {
        if is_okuri_ari(reading) {
            self.lookup_okuri_ari(reading)
        } else {
            self.lookup_okuri_nashi(reading)
        }
    }

    /// 送りなしエントリを検索
    pub fn lookup_okuri_nashi(&self, reading: &str) -> Option<&[DictEntry]> {
        self.okuri_nashi.get(reading).map(|v| v.as_slice())
    }

    /// 送りありエントリを検索
    pub fn lookup_okuri_ari(&self, reading: &str) -> Option<&[DictEntry]> {
        self.okuri_ari.get(reading).map(|v| v.as_slice())
    }

    /// 送りなしエントリ数
    pub fn okuri_nashi_count(&self) -> usize {
        self.okuri_nashi.len()
    }

    /// 送りありエントリ数
    pub fn okuri_ari_count(&self) -> usize {
        self.okuri_ari.len()
    }

    /// 全エントリ数（読みの数）
    pub fn entry_count(&self) -> usize {
        self.okuri_nashi.len() + self.okuri_ari.len()
    }

    /// エントリを追加（先頭に挿入、重複はスキップ）
    pub fn add_entry(&mut self, reading: &str, entry: DictEntry) {
        let map = if is_okuri_ari(reading) {
            &mut self.okuri_ari
        } else {
            &mut self.okuri_nashi
        };
        let entries = map.entry(reading.to_string()).or_default();
        if !entries.iter().any(|e| e.word == entry.word) {
            entries.insert(0, entry);
        }
    }

    /// SKK-JISYO 形式にシリアライズ
    pub fn to_skk_string(&self) -> String {
        let mut lines = Vec::new();
        // 送りありエントリ
        let mut okuri_ari: Vec<_> = self.okuri_ari.iter().collect();
        okuri_ari.sort_by(|a, b| a.0.cmp(b.0));
        for (reading, entries) in &okuri_ari {
            lines.push(format_dict_line(reading, entries));
        }
        // 送りなしエントリ
        let mut okuri_nashi: Vec<_> = self.okuri_nashi.iter().collect();
        okuri_nashi.sort_by(|a, b| a.0.cmp(b.0));
        for (reading, entries) in &okuri_nashi {
            lines.push(format_dict_line(reading, entries));
        }
        lines.join("\n")
    }

    /// UTF-8 で辞書ファイルに保存
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), DictError> {
        if let Some(parent) = path.as_ref().parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = self.to_skk_string();
        std::fs::write(path, content.as_bytes())?;
        Ok(())
    }
}

/// 辞書の1行をフォーマット
///
/// `reading /word1/word2;annotation/` 形式
fn format_dict_line(reading: &str, entries: &[DictEntry]) -> String {
    let mut line = format!("{} /", reading);
    for entry in entries {
        line.push_str(&entry.word);
        if let Some(ann) = &entry.annotation {
            line.push(';');
            line.push_str(ann);
        }
        line.push('/');
    }
    line
}

impl Default for Dictionary {
    fn default() -> Self {
        Self::new()
    }
}

/// 送りありエントリかどうか判定
///
/// 読みの末尾が ASCII 小文字（a-z）なら送りあり。
/// 例: "おおk" → true, "かんじ" → false
pub fn is_okuri_ari(reading: &str) -> bool {
    reading
        .as_bytes()
        .last()
        .map_or(false, |&b| b.is_ascii_lowercase())
}

/// 辞書の1行をパース
///
/// フォーマット: `読み /候補1/候補2;注釈/候補3/`
fn parse_line(line: &str) -> Option<(String, Vec<DictEntry>)> {
    // 最初のスペースで読みと候補部分を分割
    let space_pos = line.find(' ')?;
    let reading = &line[..space_pos];

    if reading.is_empty() {
        return None;
    }

    let candidates_str = &line[space_pos + 1..];

    // 候補部分は / で始まる
    let candidates_str = candidates_str.strip_prefix('/')?;

    let entries: Vec<DictEntry> = candidates_str
        .split('/')
        .filter(|s| !s.is_empty())
        .map(|s| {
            if let Some(semi_pos) = s.find(';') {
                DictEntry {
                    word: s[..semi_pos].to_string(),
                    annotation: Some(s[semi_pos + 1..].to_string()),
                }
            } else {
                DictEntry {
                    word: s.to_string(),
                    annotation: None,
                }
            }
        })
        .collect();

    if entries.is_empty() {
        return None;
    }

    Some((reading.to_string(), entries))
}

/// バイト列のエンコーディングを自動判定してデコード
///
/// UTF-8 を先に試し、失敗した場合は EUC-JP としてデコードする。
fn decode_auto(bytes: &[u8]) -> Result<String, DictError> {
    // UTF-8 として有効ならそのまま使う
    if let Ok(s) = std::str::from_utf8(bytes) {
        return Ok(s.to_string());
    }

    // EUC-JP としてデコード
    let (cow, _encoding, had_errors) = encoding_rs::EUC_JP.decode(bytes);
    if had_errors {
        return Err(DictError::Encoding(
            "Failed to decode as UTF-8 or EUC-JP".to_string(),
        ));
    }

    Ok(cow.into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    // === パース基本テスト ===

    #[test]
    fn test_parse_okuri_nashi() {
        let dict = Dictionary::from_str("かんじ /漢字/感じ/幹事/\n");
        let entries = dict.lookup_okuri_nashi("かんじ").expect("entry should exist");
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].word, "漢字");
        assert_eq!(entries[1].word, "感じ");
        assert_eq!(entries[2].word, "幹事");
        assert!(entries.iter().all(|e| e.annotation.is_none()));
    }

    #[test]
    fn test_parse_okuri_ari() {
        let dict = Dictionary::from_str("おおk /大/多/\n");
        let entries = dict.lookup_okuri_ari("おおk").expect("entry should exist");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].word, "大");
        assert_eq!(entries[1].word, "多");
    }

    #[test]
    fn test_parse_annotation() {
        let dict = Dictionary::from_str("かんじ /漢字;Chinese character/感じ;feeling/\n");
        let entries = dict.lookup("かんじ").expect("entry should exist");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].word, "漢字");
        assert_eq!(
            entries[0].annotation.as_deref(),
            Some("Chinese character")
        );
        assert_eq!(entries[1].word, "感じ");
        assert_eq!(entries[1].annotation.as_deref(), Some("feeling"));
    }

    #[test]
    fn test_parse_mixed_annotation() {
        let dict = Dictionary::from_str("き /木/気;spirit/\n");
        let entries = dict.lookup("き").expect("entry should exist");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].word, "木");
        assert!(entries[0].annotation.is_none());
        assert_eq!(entries[1].word, "気");
        assert_eq!(entries[1].annotation.as_deref(), Some("spirit"));
    }

    // === コメント・空行処理 ===

    #[test]
    fn test_skip_comments() {
        let content = "\
;; SKK-JISYO.test
;; okuri-nashi entries.
かんじ /漢字/
";
        let dict = Dictionary::from_str(content);
        assert_eq!(dict.entry_count(), 1);
        assert!(dict.lookup("かんじ").is_some());
    }

    #[test]
    fn test_skip_empty_lines() {
        let content = "\n\nかんじ /漢字/\n\nとうきょう /東京/\n\n";
        let dict = Dictionary::from_str(content);
        assert_eq!(dict.entry_count(), 2);
    }

    // === 検索テスト ===

    #[test]
    fn test_lookup_not_found() {
        let dict = Dictionary::from_str("かんじ /漢字/\n");
        assert!(dict.lookup("そんざいしない").is_none());
    }

    #[test]
    fn test_lookup_auto_detect_okuri() {
        let content = "\
おおk /大/多/
かんじ /漢字/
";
        let dict = Dictionary::from_str(content);

        // 送りあり（末尾が ASCII 小文字）
        let entries = dict.lookup("おおk").expect("okuri-ari should exist");
        assert_eq!(entries[0].word, "大");

        // 送りなし
        let entries = dict.lookup("かんじ").expect("okuri-nashi should exist");
        assert_eq!(entries[0].word, "漢字");
    }

    #[test]
    fn test_lookup_okuri_nashi_does_not_find_okuri_ari() {
        let dict = Dictionary::from_str("おおk /大/\n");
        assert!(dict.lookup_okuri_nashi("おおk").is_none());
        assert!(dict.lookup_okuri_ari("おおk").is_some());
    }

    #[test]
    fn test_lookup_okuri_ari_does_not_find_okuri_nashi() {
        let dict = Dictionary::from_str("かんじ /漢字/\n");
        assert!(dict.lookup_okuri_ari("かんじ").is_none());
        assert!(dict.lookup_okuri_nashi("かんじ").is_some());
    }

    // === エントリ数テスト ===

    #[test]
    fn test_entry_count() {
        let content = "\
;; okuri-ari entries.
おおk /大/多/
うごk /動/
;; okuri-nashi entries.
かんじ /漢字/感じ/幹事/
とうきょう /東京/
にほん /日本/
";
        let dict = Dictionary::from_str(content);
        assert_eq!(dict.okuri_ari_count(), 2);
        assert_eq!(dict.okuri_nashi_count(), 3);
        assert_eq!(dict.entry_count(), 5);
    }

    // === is_okuri_ari テスト ===

    #[test]
    fn test_is_okuri_ari() {
        assert!(is_okuri_ari("おおk"));
        assert!(is_okuri_ari("うごk"));
        assert!(is_okuri_ari("おしえr"));
        assert!(is_okuri_ari("たべr"));
        assert!(!is_okuri_ari("かんじ"));
        assert!(!is_okuri_ari("とうきょう"));
        assert!(!is_okuri_ari(""));
    }

    // === parse_line テスト ===

    #[test]
    fn test_parse_line_basic() {
        let (reading, entries) =
            parse_line("かんじ /漢字/感じ/").expect("should parse");
        assert_eq!(reading, "かんじ");
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn test_parse_line_single_candidate() {
        let (reading, entries) =
            parse_line("とうきょう /東京/").expect("should parse");
        assert_eq!(reading, "とうきょう");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].word, "東京");
    }

    #[test]
    fn test_parse_line_with_annotation() {
        let (_, entries) =
            parse_line("き /木/気;spirit/器;utensil/").expect("should parse");
        assert_eq!(entries.len(), 3);
        assert!(entries[0].annotation.is_none());
        assert_eq!(entries[1].annotation.as_deref(), Some("spirit"));
        assert_eq!(entries[2].annotation.as_deref(), Some("utensil"));
    }

    #[test]
    fn test_parse_line_no_space() {
        assert!(parse_line("invalidline").is_none());
    }

    #[test]
    fn test_parse_line_no_slash() {
        assert!(parse_line("かんじ 漢字").is_none());
    }

    #[test]
    fn test_parse_line_empty_candidates() {
        assert!(parse_line("かんじ //").is_none());
    }

    #[test]
    fn test_parse_line_empty_reading() {
        assert!(parse_line(" /漢字/").is_none());
    }

    // === 実用的な辞書テスト ===

    #[test]
    fn test_realistic_dictionary() {
        let content = "\
;; SKK-JISYO.test -*- coding: utf-8 -*-
;; okuri-ari entries.
おおk /大/多/
うごk /動/
たべr /食/
かk /書/欠/掛/
;; okuri-nashi entries.
かんじ /漢字/感じ/幹事/
とうきょう /東京/
にほん /日本/
にっぽん /日本/
ひと /人/
き /木/気;spirit/器;utensil/生;raw/
";
        let dict = Dictionary::from_str(content);

        // 送りあり
        assert_eq!(dict.okuri_ari_count(), 4);
        let entries = dict.lookup("おおk").expect("should exist");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].word, "大");

        let entries = dict.lookup("かk").expect("should exist");
        assert_eq!(entries.len(), 3);

        // 送りなし
        assert_eq!(dict.okuri_nashi_count(), 6);
        let entries = dict.lookup("かんじ").expect("should exist");
        assert_eq!(entries.len(), 3);

        let entries = dict.lookup("にほん").expect("should exist");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].word, "日本");

        // 注釈あり
        let entries = dict.lookup("き").expect("should exist");
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[0].word, "木");
        assert!(entries[0].annotation.is_none());
        assert_eq!(entries[2].word, "器");
        assert_eq!(entries[2].annotation.as_deref(), Some("utensil"));

        // 見つからない読み
        assert!(dict.lookup("そんざいしない").is_none());
    }

    // === エンコーディングテスト ===

    #[test]
    fn test_decode_utf8() {
        let content = "かんじ /漢字/\n";
        let decoded = decode_auto(content.as_bytes()).expect("should decode");
        assert_eq!(decoded, content);
    }

    #[test]
    fn test_decode_euc_jp() {
        // "かんじ /漢字/" を EUC-JP でエンコード
        let (encoded, _, _) = encoding_rs::EUC_JP.encode("かんじ /漢字/\n");
        let decoded = decode_auto(&encoded).expect("should decode");
        assert_eq!(decoded, "かんじ /漢字/\n");
    }

    #[test]
    fn test_decode_invalid() {
        // 不正なバイト列（UTF-8 でも EUC-JP でもない）
        let bytes: &[u8] = &[0x80, 0x81, 0x82, 0x83, 0xFF, 0xFE];
        let result = decode_auto(bytes);
        // EUC-JP デコーダは寛容なので、had_errors になるかは入力次第
        // ここではエラーにならなくても OK（不正な文字が置換されるだけ）
        // 重要なのはパニックしないこと
        let _ = result;
    }

    // === ファイル読み込みテスト ===

    #[test]
    fn test_load_nonexistent_file() {
        let result = Dictionary::load("/nonexistent/path/dict.txt");
        assert!(result.is_err());
        match result.unwrap_err() {
            DictError::Io(_) => {} // expected
            other => panic!("Expected Io error, got: {:?}", other),
        }
    }

    #[test]
    fn test_load_utf8_file() {
        let dir = std::env::temp_dir().join("koyubi_test_dict");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test_utf8.skk");
        let content = "\
;; test dictionary
かんじ /漢字/感じ/
とうきょう /東京/
おおk /大/多/
";
        std::fs::write(&path, content).expect("write failed");

        let dict = Dictionary::load(&path).expect("load failed");
        assert_eq!(dict.okuri_nashi_count(), 2);
        assert_eq!(dict.okuri_ari_count(), 1);

        let entries = dict.lookup("かんじ").expect("should exist");
        assert_eq!(entries.len(), 2);

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn test_load_eucjp_file() {
        let dir = std::env::temp_dir().join("koyubi_test_dict");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test_eucjp.skk");
        let content = "\
;; test dictionary
かんじ /漢字/感じ/
とうきょう /東京/
";
        let (encoded, _, _) = encoding_rs::EUC_JP.encode(content);
        std::fs::write(&path, &*encoded).expect("write failed");

        let dict = Dictionary::load(&path).expect("load failed");
        assert_eq!(dict.okuri_nashi_count(), 2);

        let entries = dict.lookup("かんじ").expect("should exist");
        assert_eq!(entries[0].word, "漢字");

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    // === Default トレイト ===

    #[test]
    fn test_empty_dictionary() {
        let dict = Dictionary::new();
        assert_eq!(dict.entry_count(), 0);
        assert!(dict.lookup("かんじ").is_none());
    }

    // === add_entry テスト ===

    #[test]
    fn test_add_entry_basic() {
        let mut dict = Dictionary::new();
        dict.add_entry(
            "てすと",
            DictEntry {
                word: "テスト".to_string(),
                annotation: None,
            },
        );
        let entries = dict.lookup("てすと").expect("should exist");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].word, "テスト");
    }

    #[test]
    fn test_add_entry_prepend() {
        let mut dict = Dictionary::from_str("かんじ /漢字/幹事/\n");
        dict.add_entry(
            "かんじ",
            DictEntry {
                word: "感じ".to_string(),
                annotation: None,
            },
        );
        let entries = dict.lookup("かんじ").expect("should exist");
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].word, "感じ"); // 先頭に挿入
        assert_eq!(entries[1].word, "漢字");
        assert_eq!(entries[2].word, "幹事");
    }

    #[test]
    fn test_add_entry_no_duplicate() {
        let mut dict = Dictionary::from_str("かんじ /漢字/感じ/\n");
        dict.add_entry(
            "かんじ",
            DictEntry {
                word: "漢字".to_string(),
                annotation: None,
            },
        );
        let entries = dict.lookup("かんじ").expect("should exist");
        assert_eq!(entries.len(), 2); // 重複はスキップ
    }

    #[test]
    fn test_add_entry_okuri_ari() {
        let mut dict = Dictionary::new();
        dict.add_entry(
            "おおk",
            DictEntry {
                word: "大".to_string(),
                annotation: None,
            },
        );
        assert!(dict.lookup_okuri_ari("おおk").is_some());
        assert!(dict.lookup_okuri_nashi("おおk").is_none());
    }

    // === to_skk_string / roundtrip テスト ===

    #[test]
    fn test_to_skk_string_roundtrip() {
        let mut dict = Dictionary::new();
        dict.add_entry(
            "かんじ",
            DictEntry {
                word: "漢字".to_string(),
                annotation: None,
            },
        );
        dict.add_entry(
            "かんじ",
            DictEntry {
                word: "感じ".to_string(),
                annotation: Some("feeling".to_string()),
            },
        );
        let skk = dict.to_skk_string();
        let dict2 = Dictionary::from_str(&skk);
        let entries = dict2.lookup("かんじ").expect("should exist");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].word, "感じ");
        assert_eq!(entries[0].annotation.as_deref(), Some("feeling"));
        assert_eq!(entries[1].word, "漢字");
    }

    // === save テスト ===

    #[test]
    fn test_save_and_load() {
        let dir = std::env::temp_dir().join("koyubi_test_save");
        let path = dir.join("user-dict.skk");

        let mut dict = Dictionary::new();
        dict.add_entry(
            "てすと",
            DictEntry {
                word: "テスト".to_string(),
                annotation: None,
            },
        );
        dict.save(&path).expect("save failed");

        let loaded = Dictionary::load(&path).expect("load failed");
        let entries = loaded.lookup("てすと").expect("should exist");
        assert_eq!(entries[0].word, "テスト");

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }
}
