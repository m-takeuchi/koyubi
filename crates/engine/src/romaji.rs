//! ローマ字→かな変換モジュール
//!
//! テーブル駆動のステートマシンでローマ字入力をひらがなに変換する。
//! カスタムルール（AZIK 等）にも将来対応可能な設計。

use std::collections::{HashMap, HashSet};

/// ローマ字→かな変換テーブル
///
/// エントリは `(ローマ字, 出力かな, 残りローマ字)` の三つ組。
/// 例: `("ka", "か", "")`, `("kk", "っ", "k")`
pub struct RomajiTable {
    entries: HashMap<String, (String, String)>,
    prefixes: HashSet<String>,
}

/// ローマ字変換の出力
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RomajiOutput {
    /// 確定されたかな文字列（空の場合はまだ未確定）
    pub committed: String,
    /// 現在の未確定ローマ字（表示用）
    pub pending: String,
}

/// ステートフルなローマ字→かな変換器
pub struct RomajiConverter {
    pending: String,
}

impl RomajiTable {
    /// デフォルトの SKK ローマ字テーブルを生成
    pub fn default_table() -> Self {
        let entries: HashMap<String, (String, String)> = DEFAULT_ENTRIES
            .iter()
            .map(|&(r, k, rem)| (r.to_string(), (k.to_string(), rem.to_string())))
            .collect();

        let mut prefixes = HashSet::new();
        for key in entries.keys() {
            for i in 1..=key.len() {
                prefixes.insert(key[..i].to_string());
            }
        }

        Self { entries, prefixes }
    }

    /// ローマ字文字列から完全一致するエントリを検索
    pub fn lookup(&self, s: &str) -> Option<&(String, String)> {
        self.entries.get(s)
    }

    /// 指定文字列で始まるエントリが存在するか（入力途中の判定に使用）
    pub fn is_valid_prefix(&self, s: &str) -> bool {
        self.prefixes.contains(s)
    }
}

impl RomajiConverter {
    pub fn new() -> Self {
        Self {
            pending: String::new(),
        }
    }

    /// 1文字入力を処理し、変換結果を返す
    pub fn feed(&mut self, ch: char, table: &RomajiTable) -> RomajiOutput {
        self.pending.push(ch);

        // 完全一致するエントリがあれば変換
        if let Some((kana, remainder)) = table.lookup(&self.pending) {
            let committed = kana.clone();
            self.pending = remainder.clone();
            return RomajiOutput {
                committed,
                pending: self.pending.clone(),
            };
        }

        // 有効なプレフィックスなら入力待ち継続
        if table.is_valid_prefix(&self.pending) {
            return RomajiOutput {
                committed: String::new(),
                pending: self.pending.clone(),
            };
        }

        // 無効な組み合わせ: 新しい文字だけで再試行
        let ch_str = ch.to_string();

        // 新しい文字単体で完全一致する場合
        if let Some((kana, remainder)) = table.lookup(&ch_str) {
            let committed = kana.clone();
            self.pending = remainder.clone();
            return RomajiOutput {
                committed,
                pending: self.pending.clone(),
            };
        }

        // 新しい文字単体が有効なプレフィックスの場合
        if table.is_valid_prefix(&ch_str) {
            self.pending = ch_str;
            return RomajiOutput {
                committed: String::new(),
                pending: self.pending.clone(),
            };
        }

        // どのエントリにもマッチしない文字（数字・記号等）
        self.pending.clear();
        RomajiOutput {
            committed: String::new(),
            pending: String::new(),
        }
    }

    /// 現在の未確定ローマ字を取得
    pub fn pending(&self) -> &str {
        &self.pending
    }

    /// 未確定状態をクリア（キャンセル時等）
    pub fn clear(&mut self) {
        self.pending.clear();
    }

    /// 未確定ローマ字の末尾を1文字削除（Backspace用）
    ///
    /// 削除できた場合は true、既に空なら false を返す。
    pub fn backspace(&mut self) -> bool {
        self.pending.pop().is_some()
    }

    /// 未確定状態を確定する（Enter/確定時に呼ぶ）
    ///
    /// 未確定が "n" の場合は "ん" を出力する。
    /// それ以外の未確定文字は破棄される。
    pub fn flush(&mut self) -> String {
        if self.pending == "n" {
            self.pending.clear();
            "ん".to_string()
        } else {
            self.pending.clear();
            String::new()
        }
    }
}

impl Default for RomajiConverter {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// デフォルトローマ字テーブル
// (ローマ字, 出力かな, 残りローマ字)
// ---------------------------------------------------------------------------
const DEFAULT_ENTRIES: &[(&str, &str, &str)] = &[
    // === 母音 ===
    ("a", "あ", ""),
    ("i", "い", ""),
    ("u", "う", ""),
    ("e", "え", ""),
    ("o", "お", ""),
    // === か行 ===
    ("ka", "か", ""),
    ("ki", "き", ""),
    ("ku", "く", ""),
    ("ke", "け", ""),
    ("ko", "こ", ""),
    // === さ行 ===
    ("sa", "さ", ""),
    ("si", "し", ""),
    ("su", "す", ""),
    ("se", "せ", ""),
    ("so", "そ", ""),
    ("shi", "し", ""),
    ("sha", "しゃ", ""),
    ("shu", "しゅ", ""),
    ("she", "しぇ", ""),
    ("sho", "しょ", ""),
    // === た行 ===
    ("ta", "た", ""),
    ("ti", "ち", ""),
    ("tu", "つ", ""),
    ("te", "て", ""),
    ("to", "と", ""),
    ("chi", "ち", ""),
    ("tsu", "つ", ""),
    ("cha", "ちゃ", ""),
    ("chu", "ちゅ", ""),
    ("che", "ちぇ", ""),
    ("cho", "ちょ", ""),
    // === な行 ===
    ("na", "な", ""),
    ("ni", "に", ""),
    ("nu", "ぬ", ""),
    ("ne", "ね", ""),
    ("no", "の", ""),
    ("nn", "ん", ""),
    ("n'", "ん", ""),
    // === は行 ===
    ("ha", "は", ""),
    ("hi", "ひ", ""),
    ("hu", "ふ", ""),
    ("he", "へ", ""),
    ("ho", "ほ", ""),
    ("fu", "ふ", ""),
    ("fa", "ふぁ", ""),
    ("fi", "ふぃ", ""),
    ("fe", "ふぇ", ""),
    ("fo", "ふぉ", ""),
    // === ま行 ===
    ("ma", "ま", ""),
    ("mi", "み", ""),
    ("mu", "む", ""),
    ("me", "め", ""),
    ("mo", "も", ""),
    // === や行 ===
    ("ya", "や", ""),
    ("yu", "ゆ", ""),
    ("yo", "よ", ""),
    // === ら行 ===
    ("ra", "ら", ""),
    ("ri", "り", ""),
    ("ru", "る", ""),
    ("re", "れ", ""),
    ("ro", "ろ", ""),
    // === わ行 ===
    ("wa", "わ", ""),
    ("wi", "ゐ", ""),
    ("we", "ゑ", ""),
    ("wo", "を", ""),
    // === が行（濁音） ===
    ("ga", "が", ""),
    ("gi", "ぎ", ""),
    ("gu", "ぐ", ""),
    ("ge", "げ", ""),
    ("go", "ご", ""),
    // === ざ行 ===
    ("za", "ざ", ""),
    ("zi", "じ", ""),
    ("zu", "ず", ""),
    ("ze", "ぜ", ""),
    ("zo", "ぞ", ""),
    ("ji", "じ", ""),
    ("ja", "じゃ", ""),
    ("ju", "じゅ", ""),
    ("je", "じぇ", ""),
    ("jo", "じょ", ""),
    // === だ行 ===
    ("da", "だ", ""),
    ("di", "ぢ", ""),
    ("du", "づ", ""),
    ("de", "で", ""),
    ("do", "ど", ""),
    // === ば行 ===
    ("ba", "ば", ""),
    ("bi", "び", ""),
    ("bu", "ぶ", ""),
    ("be", "べ", ""),
    ("bo", "ぼ", ""),
    // === ぱ行（半濁音） ===
    ("pa", "ぱ", ""),
    ("pi", "ぴ", ""),
    ("pu", "ぷ", ""),
    ("pe", "ぺ", ""),
    ("po", "ぽ", ""),
    // === きゃ行（拗音） ===
    ("kya", "きゃ", ""),
    ("kyi", "きぃ", ""),
    ("kyu", "きゅ", ""),
    ("kye", "きぇ", ""),
    ("kyo", "きょ", ""),
    // === ぎゃ行 ===
    ("gya", "ぎゃ", ""),
    ("gyi", "ぎぃ", ""),
    ("gyu", "ぎゅ", ""),
    ("gye", "ぎぇ", ""),
    ("gyo", "ぎょ", ""),
    // === しゃ行 ===
    ("sya", "しゃ", ""),
    ("syi", "しぃ", ""),
    ("syu", "しゅ", ""),
    ("sye", "しぇ", ""),
    ("syo", "しょ", ""),
    // === じゃ行 ===
    ("jya", "じゃ", ""),
    ("jyi", "じぃ", ""),
    ("jyu", "じゅ", ""),
    ("jye", "じぇ", ""),
    ("jyo", "じょ", ""),
    ("zya", "じゃ", ""),
    ("zyi", "じぃ", ""),
    ("zyu", "じゅ", ""),
    ("zye", "じぇ", ""),
    ("zyo", "じょ", ""),
    // === ちゃ行 ===
    ("tya", "ちゃ", ""),
    ("tyi", "ちぃ", ""),
    ("tyu", "ちゅ", ""),
    ("tye", "ちぇ", ""),
    ("tyo", "ちょ", ""),
    // === にゃ行 ===
    ("nya", "にゃ", ""),
    ("nyi", "にぃ", ""),
    ("nyu", "にゅ", ""),
    ("nye", "にぇ", ""),
    ("nyo", "にょ", ""),
    // === ひゃ行 ===
    ("hya", "ひゃ", ""),
    ("hyi", "ひぃ", ""),
    ("hyu", "ひゅ", ""),
    ("hye", "ひぇ", ""),
    ("hyo", "ひょ", ""),
    // === みゃ行 ===
    ("mya", "みゃ", ""),
    ("myi", "みぃ", ""),
    ("myu", "みゅ", ""),
    ("mye", "みぇ", ""),
    ("myo", "みょ", ""),
    // === りゃ行 ===
    ("rya", "りゃ", ""),
    ("ryi", "りぃ", ""),
    ("ryu", "りゅ", ""),
    ("rye", "りぇ", ""),
    ("ryo", "りょ", ""),
    // === びゃ行 ===
    ("bya", "びゃ", ""),
    ("byi", "びぃ", ""),
    ("byu", "びゅ", ""),
    ("bye", "びぇ", ""),
    ("byo", "びょ", ""),
    // === ぴゃ行 ===
    ("pya", "ぴゃ", ""),
    ("pyi", "ぴぃ", ""),
    ("pyu", "ぴゅ", ""),
    ("pye", "ぴぇ", ""),
    ("pyo", "ぴょ", ""),
    // === ぢゃ行 ===
    ("dya", "ぢゃ", ""),
    ("dyi", "ぢぃ", ""),
    ("dyu", "ぢゅ", ""),
    ("dye", "ぢぇ", ""),
    ("dyo", "ぢょ", ""),
    // === てぃ行（外来語） ===
    ("tha", "てゃ", ""),
    ("thi", "てぃ", ""),
    ("thu", "てゅ", ""),
    ("the", "てぇ", ""),
    ("tho", "てょ", ""),
    // === でぃ行（外来語） ===
    ("dha", "でゃ", ""),
    ("dhi", "でぃ", ""),
    ("dhu", "でゅ", ""),
    ("dhe", "でぇ", ""),
    ("dho", "でょ", ""),
    // === うぃ行（外来語） ===
    ("wha", "うぁ", ""),
    ("whi", "うぃ", ""),
    ("whe", "うぇ", ""),
    ("who", "うぉ", ""),
    // === とぅ行（外来語） ===
    ("twa", "とぁ", ""),
    ("twi", "とぃ", ""),
    ("twu", "とぅ", ""),
    ("twe", "とぇ", ""),
    ("two", "とぉ", ""),
    // === どぅ行（外来語） ===
    ("dwa", "どぁ", ""),
    ("dwi", "どぃ", ""),
    ("dwu", "どぅ", ""),
    ("dwe", "どぇ", ""),
    ("dwo", "どぉ", ""),
    // === くぁ行（外来語） ===
    ("kwa", "くぁ", ""),
    ("kwi", "くぃ", ""),
    ("kwu", "くぅ", ""),
    ("kwe", "くぇ", ""),
    ("kwo", "くぉ", ""),
    // === ぐぁ行（外来語） ===
    ("gwa", "ぐぁ", ""),
    ("gwi", "ぐぃ", ""),
    ("gwu", "ぐぅ", ""),
    ("gwe", "ぐぇ", ""),
    ("gwo", "ぐぉ", ""),
    // === ゔ行 ===
    ("va", "ゔぁ", ""),
    ("vi", "ゔぃ", ""),
    ("vu", "ゔ", ""),
    ("ve", "ゔぇ", ""),
    ("vo", "ゔぉ", ""),
    // === 小書きかな（x プレフィックス） ===
    ("xa", "ぁ", ""),
    ("xi", "ぃ", ""),
    ("xu", "ぅ", ""),
    ("xe", "ぇ", ""),
    ("xo", "ぉ", ""),
    ("xya", "ゃ", ""),
    ("xyu", "ゅ", ""),
    ("xyo", "ょ", ""),
    ("xtu", "っ", ""),
    ("xtsu", "っ", ""),
    ("xwa", "ゎ", ""),
    ("xka", "ヵ", ""),
    ("xke", "ヶ", ""),
    // === z プレフィックス記号 ===
    ("zh", "←", ""),
    ("zj", "↓", ""),
    ("zk", "↑", ""),
    ("zl", "→", ""),
    ("z-", "〜", ""),
    ("z[", "『", ""),
    ("z]", "』", ""),
    ("z,", "‥", ""),
    ("z.", "…", ""),
    ("z/", "・", ""),
    // === 促音（二重子音） ===
    ("bb", "っ", "b"),
    ("cc", "っ", "c"),
    ("dd", "っ", "d"),
    ("ff", "っ", "f"),
    ("gg", "っ", "g"),
    ("hh", "っ", "h"),
    ("jj", "っ", "j"),
    ("kk", "っ", "k"),
    ("mm", "っ", "m"),
    ("pp", "っ", "p"),
    ("rr", "っ", "r"),
    ("ss", "っ", "s"),
    ("tt", "っ", "t"),
    ("vv", "っ", "v"),
    ("ww", "っ", "w"),
    ("yy", "っ", "y"),
    ("zz", "っ", "z"),
    // === n + 子音 → ん（n/y/母音以外） ===
    ("nb", "ん", "b"),
    ("nc", "ん", "c"),
    ("nd", "ん", "d"),
    ("nf", "ん", "f"),
    ("ng", "ん", "g"),
    ("nh", "ん", "h"),
    ("nj", "ん", "j"),
    ("nk", "ん", "k"),
    ("nl", "ん", ""),
    ("nm", "ん", "m"),
    ("np", "ん", "p"),
    ("nq", "ん", ""),
    ("nr", "ん", "r"),
    ("ns", "ん", "s"),
    ("nt", "ん", "t"),
    ("nv", "ん", "v"),
    ("nw", "ん", "w"),
    ("nx", "ん", "x"),
    ("nz", "ん", "z"),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn convert(input: &str) -> (String, String) {
        let table = RomajiTable::default_table();
        let mut conv = RomajiConverter::new();
        let mut result = String::new();
        for ch in input.chars() {
            let out = conv.feed(ch, &table);
            result.push_str(&out.committed);
        }
        let flushed = conv.flush();
        result.push_str(&flushed);
        (result, conv.pending().to_string())
    }

    fn convert_no_flush(input: &str) -> (String, String) {
        let table = RomajiTable::default_table();
        let mut conv = RomajiConverter::new();
        let mut result = String::new();
        for ch in input.chars() {
            let out = conv.feed(ch, &table);
            result.push_str(&out.committed);
        }
        (result, conv.pending().to_string())
    }

    // --- 基本母音 ---

    #[test]
    fn test_vowels() {
        assert_eq!(convert("aiueo"), ("あいうえお".to_string(), String::new()));
    }

    // --- 子音 + 母音 ---

    #[test]
    fn test_ka_row() {
        assert_eq!(convert("kakikukeko"), ("かきくけこ".to_string(), String::new()));
    }

    #[test]
    fn test_sa_row() {
        assert_eq!(convert("sasuseso"), ("さすせそ".to_string(), String::new()));
        assert_eq!(convert("si"), ("し".to_string(), String::new()));
        assert_eq!(convert("shi"), ("し".to_string(), String::new()));
    }

    #[test]
    fn test_ta_row() {
        assert_eq!(convert("tateto"), ("たてと".to_string(), String::new()));
        assert_eq!(convert("ti"), ("ち".to_string(), String::new()));
        assert_eq!(convert("chi"), ("ち".to_string(), String::new()));
        assert_eq!(convert("tu"), ("つ".to_string(), String::new()));
        assert_eq!(convert("tsu"), ("つ".to_string(), String::new()));
    }

    #[test]
    fn test_na_row() {
        assert_eq!(convert("naninuneno"), ("なにぬねの".to_string(), String::new()));
    }

    #[test]
    fn test_ha_row() {
        assert_eq!(convert("hahihuheho"), ("はひふへほ".to_string(), String::new()));
        assert_eq!(convert("fu"), ("ふ".to_string(), String::new()));
    }

    #[test]
    fn test_ma_row() {
        assert_eq!(convert("mamimumemo"), ("まみむめも".to_string(), String::new()));
    }

    #[test]
    fn test_ya_row() {
        assert_eq!(convert("yayuyo"), ("やゆよ".to_string(), String::new()));
    }

    #[test]
    fn test_ra_row() {
        assert_eq!(convert("rarirurero"), ("らりるれろ".to_string(), String::new()));
    }

    #[test]
    fn test_wa_row() {
        assert_eq!(convert("wawo"), ("わを".to_string(), String::new()));
    }

    // --- 濁音・半濁音 ---

    #[test]
    fn test_dakuten() {
        assert_eq!(convert("gagigugego"), ("がぎぐげご".to_string(), String::new()));
        assert_eq!(convert("zazizuzezo"), ("ざじずぜぞ".to_string(), String::new()));
        assert_eq!(convert("dadidudedo"), ("だぢづでど".to_string(), String::new()));
        assert_eq!(convert("babibubebo"), ("ばびぶべぼ".to_string(), String::new()));
    }

    #[test]
    fn test_handakuten() {
        assert_eq!(convert("papipupepo"), ("ぱぴぷぺぽ".to_string(), String::new()));
    }

    // --- 拗音 ---

    #[test]
    fn test_youon_ky() {
        assert_eq!(convert("kyakyukyo"), ("きゃきゅきょ".to_string(), String::new()));
    }

    #[test]
    fn test_youon_sh() {
        assert_eq!(convert("shashusho"), ("しゃしゅしょ".to_string(), String::new()));
    }

    #[test]
    fn test_youon_ch() {
        assert_eq!(convert("chachucho"), ("ちゃちゅちょ".to_string(), String::new()));
    }

    #[test]
    fn test_youon_ny() {
        assert_eq!(convert("nyanyunyo"), ("にゃにゅにょ".to_string(), String::new()));
    }

    #[test]
    fn test_youon_hy() {
        assert_eq!(convert("hyahyuhyo"), ("ひゃひゅひょ".to_string(), String::new()));
    }

    #[test]
    fn test_youon_my() {
        assert_eq!(convert("myamyumyo"), ("みゃみゅみょ".to_string(), String::new()));
    }

    #[test]
    fn test_youon_ry() {
        assert_eq!(convert("ryaryuryo"), ("りゃりゅりょ".to_string(), String::new()));
    }

    #[test]
    fn test_youon_gy() {
        assert_eq!(convert("gyagyugyo"), ("ぎゃぎゅぎょ".to_string(), String::new()));
    }

    #[test]
    fn test_youon_by() {
        assert_eq!(convert("byabyubyo"), ("びゃびゅびょ".to_string(), String::new()));
    }

    #[test]
    fn test_youon_py() {
        assert_eq!(convert("pyapyupyo"), ("ぴゃぴゅぴょ".to_string(), String::new()));
    }

    #[test]
    fn test_youon_ja() {
        assert_eq!(convert("jajujo"), ("じゃじゅじょ".to_string(), String::new()));
    }

    // --- 促音（っ） ---

    #[test]
    fn test_sokuon_kk() {
        assert_eq!(convert("kakko"), ("かっこ".to_string(), String::new()));
    }

    #[test]
    fn test_sokuon_tt() {
        assert_eq!(convert("kitto"), ("きっと".to_string(), String::new()));
    }

    #[test]
    fn test_sokuon_ss() {
        assert_eq!(convert("massugu"), ("まっすぐ".to_string(), String::new()));
    }

    #[test]
    fn test_sokuon_pp() {
        assert_eq!(convert("nippon"), ("にっぽん".to_string(), String::new()));
    }

    #[test]
    fn test_sokuon_cc() {
        assert_eq!(convert("kocchi"), ("こっち".to_string(), String::new()));
    }

    // --- ん の処理 ---

    #[test]
    fn test_nn() {
        assert_eq!(convert("nn"), ("ん".to_string(), String::new()));
    }

    #[test]
    fn test_n_apostrophe() {
        assert_eq!(convert("n'"), ("ん".to_string(), String::new()));
    }

    #[test]
    fn test_n_before_consonant() {
        assert_eq!(convert("kankei"), ("かんけい".to_string(), String::new()));
    }

    #[test]
    fn test_n_before_vowel() {
        // "na" は "な" であり、"ん" + "あ" にはならない
        assert_eq!(convert("na"), ("な".to_string(), String::new()));
    }

    #[test]
    fn test_n_at_end_flush() {
        // flush で末尾の n が ん になる
        assert_eq!(convert("kan"), ("かん".to_string(), String::new()));
    }

    #[test]
    fn test_n_at_end_no_flush() {
        // flush しなければ n は pending のまま
        assert_eq!(
            convert_no_flush("kan"),
            ("か".to_string(), "n".to_string())
        );
    }

    #[test]
    fn test_shinbun() {
        assert_eq!(convert("shinbun"), ("しんぶん".to_string(), String::new()));
    }

    #[test]
    fn test_konnichiha() {
        // "nn" → ん, 残りの "n" が次の "ni" → に の開始になる
        assert_eq!(
            convert("konnnichiha"),
            ("こんにちは".to_string(), String::new())
        );
        // n' で明示的に ん を確定する方法でも同じ結果
        assert_eq!(
            convert("kon'nichiha"),
            ("こんにちは".to_string(), String::new())
        );
        // "konnichiha"（n が2つ）は "こんいちは" になる（nn→ん, i→い）
        assert_eq!(
            convert("konnichiha"),
            ("こんいちは".to_string(), String::new())
        );
    }

    // --- 小書きかな ---

    #[test]
    fn test_small_kana() {
        assert_eq!(convert("xa"), ("ぁ".to_string(), String::new()));
        assert_eq!(convert("xi"), ("ぃ".to_string(), String::new()));
        assert_eq!(convert("xu"), ("ぅ".to_string(), String::new()));
        assert_eq!(convert("xe"), ("ぇ".to_string(), String::new()));
        assert_eq!(convert("xo"), ("ぉ".to_string(), String::new()));
        assert_eq!(convert("xtu"), ("っ".to_string(), String::new()));
        assert_eq!(convert("xtsu"), ("っ".to_string(), String::new()));
        assert_eq!(convert("xya"), ("ゃ".to_string(), String::new()));
        assert_eq!(convert("xyu"), ("ゅ".to_string(), String::new()));
        assert_eq!(convert("xyo"), ("ょ".to_string(), String::new()));
    }

    // --- z プレフィックス記号 ---

    #[test]
    fn test_z_symbols() {
        assert_eq!(convert("zh"), ("←".to_string(), String::new()));
        assert_eq!(convert("zj"), ("↓".to_string(), String::new()));
        assert_eq!(convert("zk"), ("↑".to_string(), String::new()));
        assert_eq!(convert("zl"), ("→".to_string(), String::new()));
    }

    // --- 外来語対応 ---

    #[test]
    fn test_fa_fi_fu_fe_fo() {
        assert_eq!(
            convert("faifu"),
            ("ふぁいふ".to_string(), String::new())
        );
    }

    #[test]
    fn test_thi_dhi() {
        assert_eq!(convert("thi"), ("てぃ".to_string(), String::new()));
        assert_eq!(convert("dhi"), ("でぃ".to_string(), String::new()));
    }

    // --- pending 状態の遷移テスト ---

    #[test]
    fn test_pending_single_consonant() {
        let table = RomajiTable::default_table();
        let mut conv = RomajiConverter::new();

        let out = conv.feed('k', &table);
        assert_eq!(out.committed, "");
        assert_eq!(out.pending, "k");

        let out = conv.feed('a', &table);
        assert_eq!(out.committed, "か");
        assert_eq!(out.pending, "");
    }

    #[test]
    fn test_pending_three_char_sequence() {
        let table = RomajiTable::default_table();
        let mut conv = RomajiConverter::new();

        let out = conv.feed('s', &table);
        assert_eq!(out.pending, "s");
        assert_eq!(out.committed, "");

        let out = conv.feed('h', &table);
        assert_eq!(out.pending, "sh");
        assert_eq!(out.committed, "");

        let out = conv.feed('i', &table);
        assert_eq!(out.pending, "");
        assert_eq!(out.committed, "し");
    }

    #[test]
    fn test_sokuon_leaves_pending() {
        let table = RomajiTable::default_table();
        let mut conv = RomajiConverter::new();

        let out = conv.feed('k', &table);
        assert_eq!(out.pending, "k");

        let out = conv.feed('k', &table);
        assert_eq!(out.committed, "っ");
        assert_eq!(out.pending, "k");

        let out = conv.feed('a', &table);
        assert_eq!(out.committed, "か");
        assert_eq!(out.pending, "");
    }

    // --- 実用的な文字列テスト ---

    #[test]
    fn test_toukyou() {
        assert_eq!(
            convert("toukyou"),
            ("とうきょう".to_string(), String::new())
        );
    }

    #[test]
    fn test_ookii() {
        assert_eq!(convert("ookii"), ("おおきい".to_string(), String::new()));
    }

    #[test]
    fn test_gakkou() {
        assert_eq!(
            convert("gakkou"),
            ("がっこう".to_string(), String::new())
        );
    }

    #[test]
    fn test_chottomatte() {
        assert_eq!(
            convert("chottomatte"),
            ("ちょっとまって".to_string(), String::new())
        );
    }

    #[test]
    fn test_nihongo() {
        assert_eq!(
            convert("nihongo"),
            ("にほんご".to_string(), String::new())
        );
    }

    // --- clear テスト ---

    #[test]
    fn test_clear() {
        let table = RomajiTable::default_table();
        let mut conv = RomajiConverter::new();
        conv.feed('k', &table);
        assert_eq!(conv.pending(), "k");
        conv.clear();
        assert_eq!(conv.pending(), "");
    }

    // --- テーブルの整合性テスト ---

    #[test]
    fn test_all_entries_have_valid_remainder_prefix() {
        let table = RomajiTable::default_table();
        for (romaji, (_, remainder)) in &table.entries {
            if !remainder.is_empty() {
                assert!(
                    table.is_valid_prefix(remainder),
                    "Entry {:?} has remainder {:?} which is not a valid prefix",
                    romaji,
                    remainder
                );
            }
        }
    }
}
