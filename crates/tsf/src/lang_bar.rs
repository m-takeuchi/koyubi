//! 言語バーボタン — ITfLangBarItemButton + ITfSource
//!
//! システムトレイにモード表示アイコンを提供する。
//! GDI でテキストベースのアイコンを動的生成する。

use std::cell::{Cell, RefCell};
use std::io::Write as _;
use std::path::PathBuf;

use koyubi_engine::InputMode;

use windows::core::{implement, BSTR, Interface as _};
use windows::Win32::Foundation::{POINT, RECT};
use windows::Win32::System::Ole::{
    CONNECT_E_ADVISELIMIT, CONNECT_E_CANNOTCONNECT, CONNECT_E_NOCONNECTION,
};
use windows_core::BOOL;
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleBitmap, CreateCompatibleDC, CreateFontW, DeleteDC, DeleteObject,
    GetDC, GetTextExtentPoint32W, ReleaseDC, SelectObject, SetBkColor, SetTextColor,
    TextOutW, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_QUALITY, FF_DONTCARE,
    FW_BOLD, HGDIOBJ, OUT_TT_ONLY_PRECIS,
};
use windows::Win32::UI::TextServices::{
    GUID_LBI_INPUTMODE, ITfLangBarItem, ITfLangBarItemButton, ITfLangBarItemButton_Impl,
    ITfLangBarItemMgr, ITfLangBarItemSink, ITfLangBarItem_Impl, ITfMenu, ITfSource,
    ITfSource_Impl, ITfThreadMgr, TF_LANGBARITEMINFO, TF_LBI_ICON, TF_LBI_STATUS,
    TF_LBI_TEXT, TF_LBI_TOOLTIP,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, HICON, ICONINFO, SW_SHOW};
use windows_core::IUnknown;

use crate::globals::CLSID_KOYUBI_TEXT_SERVICE;

/// デバッグログ
macro_rules! dbglog {
    ($($arg:tt)*) => {{
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(r"\\192.168.122.1\koyubi\debug.log")
        {
            let _ = writeln!(f, $($arg)*);
        }
    }};
}

// TF_LBI_STYLE 定数
const TF_LBI_STYLE_BTN_BUTTON: u32 = 0x00010000;
const TF_LBI_STYLE_SHOWNINTRAY: u32 = 0x00000002;


const DEFAULT_CONFIG_TOML: &str = r#"# Koyubi SKK 設定ファイル
# 変更後、IME を再有効化（入力切替）すると反映されます。

# SandS (Space and Shift) 機能
sands_enabled = true

# Emacs キーバインド (Ctrl+F/B/A/E/N/P/D/K)
emacs_bindings_enabled = true

# 起動時の入力モード ("Ascii", "Hiragana", "Katakana")
initial_mode = "Ascii"

# キーマップ
toggle_kana = "q"
enter_ascii = "l"
enter_zenkaku = "L"
prev_candidate = "x"

# 辞書パス（空欄の場合は自動検出）
# system_dict_paths = ["C:\\path\\to\\SKK-JISYO.L"]
# user_dict_path = "C:\\Users\\...\\AppData\\Roaming\\Koyubi\\dict\\user-dict.skk"
"#;

/// 言語バーボタン
#[implement(ITfLangBarItemButton, ITfLangBarItem, ITfSource)]
pub struct LangBarButton {
    info: TF_LANGBARITEMINFO,
    sink: RefCell<Option<ITfLangBarItemSink>>,
    current_mode: Cell<InputMode>,
}

impl LangBarButton {
    pub fn new() -> Self {
        let mut desc = [0u16; 32];
        let text: Vec<u16> = "Koyubi SKK".encode_utf16().collect();
        let len = text.len().min(31);
        desc[..len].copy_from_slice(&text[..len]);

        let info = TF_LANGBARITEMINFO {
            clsidService: CLSID_KOYUBI_TEXT_SERVICE,
            guidItem: GUID_LBI_INPUTMODE, // Windows 8+ で必須
            dwStyle: TF_LBI_STYLE_BTN_BUTTON | TF_LBI_STYLE_SHOWNINTRAY,
            ulSort: 0,
            szDescription: desc,
        };

        Self {
            info,
            sink: RefCell::new(None),
            current_mode: Cell::new(InputMode::Ascii),
        }
    }

    /// モード変更を通知
    pub fn update_mode(&self, mode: InputMode) {
        if self.current_mode.get() == mode {
            return;
        }
        self.current_mode.set(mode);
        if let Some(ref sink) = *self.sink.borrow() {
            let _ = unsafe { sink.OnUpdate(TF_LBI_ICON | TF_LBI_TEXT | TF_LBI_TOOLTIP) };
        }
    }
}

/// 言語バーにボタンを追加する
pub fn add_to_lang_bar(
    thread_mgr: &ITfThreadMgr,
    button: &ITfLangBarItemButton,
) -> windows::core::Result<()> {
    let mgr: ITfLangBarItemMgr = thread_mgr.cast()?;
    unsafe {
        mgr.AddItem(button)?; // ITfLangBarItemButton は ITfLangBarItem を継承
    }
    dbglog!("LangBarButton: added to lang bar");
    Ok(())
}

/// 言語バーからボタンを削除する
pub fn remove_from_lang_bar(
    thread_mgr: &ITfThreadMgr,
    button: &ITfLangBarItemButton,
) -> windows::core::Result<()> {
    let mgr: ITfLangBarItemMgr = thread_mgr.cast()?;
    unsafe {
        mgr.RemoveItem(button)?;
    }
    dbglog!("LangBarButton: removed from lang bar");
    Ok(())
}

/// モード別の表示文字
fn mode_label(mode: InputMode) -> &'static str {
    match mode {
        InputMode::Ascii => "A",
        InputMode::Hiragana => "\u{3042}",         // あ
        InputMode::Katakana => "\u{30A2}",          // ア
        InputMode::HankakuKatakana => "\u{FF71}",   // ｱ
        InputMode::ZenkakuAscii => "\u{5168}",      // 全
        InputMode::Abbrev => "ab",
    }
}

/// モード別のツールチップ
fn mode_tooltip(mode: InputMode) -> &'static str {
    match mode {
        InputMode::Ascii => "Koyubi SKK - ASCII",
        InputMode::Hiragana => "Koyubi SKK - \u{3072}\u{3089}\u{304C}\u{306A}",
        InputMode::Katakana => "Koyubi SKK - \u{30AB}\u{30BF}\u{30AB}\u{30CA}",
        InputMode::HankakuKatakana => "Koyubi SKK - \u{534A}\u{89D2}\u{30AB}\u{30BF}\u{30AB}\u{30CA}",
        InputMode::ZenkakuAscii => "Koyubi SKK - \u{5168}\u{89D2}\u{82F1}\u{6570}",
        InputMode::Abbrev => "Koyubi SKK - Abbrev",
    }
}

/// モード別のフォント名
fn mode_font(mode: InputMode) -> &'static str {
    match mode {
        InputMode::Ascii | InputMode::Abbrev => "Segoe UI",
        _ => "Yu Gothic UI",
    }
}

/// モード別のフォントサイズ
fn mode_font_size(mode: InputMode) -> i32 {
    match mode {
        InputMode::Abbrev => 10,
        _ => 14,
    }
}

// =========================================================
// ITfLangBarItem_Impl
// =========================================================

impl ITfLangBarItem_Impl for LangBarButton_Impl {
    fn GetInfo(&self, pinfo: *mut TF_LANGBARITEMINFO) -> windows::core::Result<()> {
        if !pinfo.is_null() {
            unsafe {
                *pinfo = self.info;
            }
        }
        Ok(())
    }

    fn GetStatus(&self) -> windows::core::Result<u32> {
        Ok(0) // 常に有効
    }

    fn Show(&self, _fshow: BOOL) -> windows::core::Result<()> {
        if let Some(ref sink) = *self.sink.borrow() {
            let _ = unsafe { sink.OnUpdate(TF_LBI_STATUS) };
        }
        Ok(())
    }

    fn GetTooltipString(&self) -> windows::core::Result<BSTR> {
        let tooltip = mode_tooltip(self.current_mode.get());
        Ok(BSTR::from(tooltip))
    }
}

// =========================================================
// ITfLangBarItemButton_Impl
// =========================================================

impl ITfLangBarItemButton_Impl for LangBarButton_Impl {
    fn OnClick(
        &self,
        click: windows::Win32::UI::TextServices::TfLBIClick,
        _pt: &POINT,
        _prcarea: *const RECT,
    ) -> windows::core::Result<()> {
        dbglog!("LangBarButton::OnClick: click={:?}", click);
        // 左クリックで直接設定ファイルを開く（メニューが出ない環境用フォールバック）
        open_config_file();
        Ok(())
    }

    fn InitMenu(
        &self,
        _pmenu: windows::core::Ref<'_, ITfMenu>,
    ) -> windows::core::Result<()> {
        Ok(())
    }

    fn OnMenuSelect(&self, _wid: u32) -> windows::core::Result<()> {
        Ok(())
    }

    fn GetIcon(&self) -> windows::core::Result<HICON> {
        let mode = self.current_mode.get();
        create_mode_icon(mode)
    }

    fn GetText(&self) -> windows::core::Result<BSTR> {
        let label = mode_label(self.current_mode.get());
        Ok(BSTR::from(label))
    }
}

// =========================================================
// ITfSource_Impl
// =========================================================

impl ITfSource_Impl for LangBarButton_Impl {
    fn AdviseSink(&self, riid: *const windows::core::GUID, punk: windows::core::Ref<'_, IUnknown>) -> windows::core::Result<u32> {
        let riid = unsafe { &*riid };
        if *riid != ITfLangBarItemSink::IID {
            return Err(CONNECT_E_CANNOTCONNECT.into());
        }
        if self.sink.borrow().is_some() {
            return Err(CONNECT_E_ADVISELIMIT.into());
        }
        let unk: IUnknown = punk.ok()?.clone();
        let sink: ITfLangBarItemSink = unk.cast()?;
        *self.sink.borrow_mut() = Some(sink);
        Ok(1) // cookie
    }

    fn UnadviseSink(&self, dwcookie: u32) -> windows::core::Result<()> {
        if dwcookie != 1 || self.sink.borrow().is_none() {
            return Err(CONNECT_E_NOCONNECTION.into());
        }
        *self.sink.borrow_mut() = None;
        Ok(())
    }
}

// =========================================================
// 設定ファイルを開く
// =========================================================

fn open_config_file() {
    let appdata = match std::env::var("APPDATA") {
        Ok(v) => v,
        Err(_) => return,
    };
    let dir = PathBuf::from(&appdata).join("Koyubi");
    let path = dir.join("config.toml");

    // ファイルがなければデフォルト設定を書き出す
    if !path.exists() {
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(&path, DEFAULT_CONFIG_TOML);
    }

    // 既定のエディタで開く
    let path_wide: Vec<u16> = path
        .to_string_lossy()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        ShellExecuteW(
            None,
            windows::core::w!("open"),
            windows::core::PCWSTR(path_wide.as_ptr()),
            None,
            None,
            SW_SHOW,
        );
    }
}

// =========================================================
// GDI アイコン生成
// =========================================================

/// モード別のテキストアイコンを 16x16 で生成する
fn create_mode_icon(mode: InputMode) -> windows::core::Result<HICON> {
    let label = mode_label(mode);
    let font_name = mode_font(mode);
    let font_size = mode_font_size(mode);
    let wide_label: Vec<u16> = label.encode_utf16().collect();

    unsafe {
        // スクリーン DC を取得
        let hdc_screen = GetDC(None);
        if hdc_screen.is_invalid() {
            return Err(windows::core::Error::from_win32());
        }

        // メモリ DC（カラー用）
        let hdc_color = CreateCompatibleDC(Some(hdc_screen));
        let bmp_color = CreateCompatibleBitmap(hdc_screen, 16, 16);
        let old_bmp_color = SelectObject(hdc_color, HGDIOBJ(bmp_color.0));

        // メモリ DC（マスク用）
        let hdc_mask = CreateCompatibleDC(Some(hdc_screen));
        let bmp_mask = CreateCompatibleBitmap(hdc_screen, 16, 16);
        let old_bmp_mask = SelectObject(hdc_mask, HGDIOBJ(bmp_mask.0));

        // フォント作成
        let font_name_wide: Vec<u16> = font_name.encode_utf16().chain(std::iter::once(0)).collect();
        let font = CreateFontW(
            font_size,
            0, 0, 0,
            FW_BOLD.0 as i32,
            0, 0, 0,
            DEFAULT_CHARSET,
            OUT_TT_ONLY_PRECIS,
            CLIP_DEFAULT_PRECIS,
            DEFAULT_QUALITY,
            FF_DONTCARE.0 as u32,
            windows::core::PCWSTR(font_name_wide.as_ptr()),
        );

        // カラービットマップ: 白背景 + 黒文字
        let old_font_color = SelectObject(hdc_color, HGDIOBJ(font.0));
        SetBkColor(hdc_color, windows::Win32::Foundation::COLORREF(0x00FFFFFF)); // 白
        SetTextColor(hdc_color, windows::Win32::Foundation::COLORREF(0x00000000)); // 黒

        // 背景を白で塗り潰し
        let bg_rect = RECT { left: 0, top: 0, right: 16, bottom: 16 };
        let white_brush = windows::Win32::Graphics::Gdi::CreateSolidBrush(
            windows::Win32::Foundation::COLORREF(0x00FFFFFF),
        );
        windows::Win32::Graphics::Gdi::FillRect(hdc_color, &bg_rect, white_brush);
        let _ = DeleteObject(HGDIOBJ(white_brush.0));

        // テキストをセンタリング
        let mut sz = windows::Win32::Foundation::SIZE::default();
        let _ = GetTextExtentPoint32W(hdc_color, &wide_label, &mut sz);
        let x = (16 - sz.cx) / 2;
        let y = (16 - sz.cy) / 2;
        let _ = TextOutW(hdc_color, x, y, &wide_label);

        // マスクビットマップ: 全て不透明（0 = 不透明）
        let black_brush = windows::Win32::Graphics::Gdi::CreateSolidBrush(
            windows::Win32::Foundation::COLORREF(0x00000000),
        );
        windows::Win32::Graphics::Gdi::FillRect(hdc_mask, &bg_rect, black_brush);
        let _ = DeleteObject(HGDIOBJ(black_brush.0));

        // HICON 生成
        let icon_info = ICONINFO {
            fIcon: BOOL(1),
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: bmp_mask,
            hbmColor: bmp_color,
        };
        let hicon = CreateIconIndirect(&icon_info)?;

        // クリーンアップ
        let _ = SelectObject(hdc_color, old_font_color);
        let _ = SelectObject(hdc_color, old_bmp_color);
        let _ = SelectObject(hdc_mask, old_bmp_mask);
        let _ = DeleteObject(HGDIOBJ(font.0));
        let _ = DeleteObject(HGDIOBJ(bmp_color.0));
        let _ = DeleteObject(HGDIOBJ(bmp_mask.0));
        let _ = DeleteDC(hdc_color);
        let _ = DeleteDC(hdc_mask);
        let _ = ReleaseDC(None, hdc_screen);

        dbglog!("create_mode_icon: mode={:?} icon={:?}", mode, hicon);
        Ok(hicon)
    }
}
