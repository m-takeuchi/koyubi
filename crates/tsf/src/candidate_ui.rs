//! 候補ウィンドウ（Win32 ポップアップ）
//!
//! 変換候補を番号付きリストで表示する。
//! WS_POPUP + WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE で
//! タスクバー非表示・最前面・フォーカス奪取なしのウィンドウを作成する。

use std::cell::Cell;
use std::io::Write as _;
use std::mem;
use std::ptr;

use koyubi_engine::CandidateDisplay;

use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, DeleteObject, EndPaint, FillRect, GetStockObject, InvalidateRect,
    SelectObject, SetBkMode, SetTextColor, TextOutW, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET,
    DEFAULT_QUALITY, FF_MODERN, HBRUSH, HFONT, HGDIOBJ, NULL_BRUSH, OUT_TT_ONLY_PRECIS,
    PAINTSTRUCT, TRANSPARENT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetWindowLongPtrW, MoveWindow,
    RegisterClassExW, SetWindowLongPtrW, ShowWindow, GWLP_USERDATA, SW_HIDE, SW_SHOWNOACTIVATE,
    WM_PAINT, WNDCLASSEXW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

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

/// 候補ウィンドウに渡す描画データ
struct CandidateData {
    candidates: Vec<CandidateDisplay>,
    selected: usize,
    current_page: usize,
    total_pages: usize,
    font: HFONT,
}

/// 候補ウィンドウ
pub struct CandidateWindow {
    hwnd: Cell<HWND>,
    font: Cell<HFONT>,
}

/// ヌル終端付き UTF-16 エンコード（Win32 API 用）
fn to_wide_null(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// UTF-16 エンコード（スライスとして渡す用）
fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

impl CandidateWindow {
    pub fn new() -> Self {
        Self {
            hwnd: Cell::new(HWND::default()),
            font: Cell::new(HFONT::default()),
        }
    }

    /// ウィンドウクラスを登録し、ウィンドウを作成する（非表示状態）
    pub fn create(&self, hinstance: HINSTANCE) -> windows::core::Result<()> {
        let class_name = to_wide_null("KoyubiCandidateWindow");

        let wc = WNDCLASSEXW {
            cbSize: mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(candidate_wnd_proc),
            hInstance: hinstance,
            lpszClassName: windows::core::PCWSTR(class_name.as_ptr()),
            hbrBackground: unsafe { HBRUSH(GetStockObject(NULL_BRUSH).0) },
            ..Default::default()
        };

        unsafe {
            RegisterClassExW(&wc);
        }

        // フォント作成: Yu Gothic UI 16px
        let font_name = to_wide_null("Yu Gothic UI");
        let font = unsafe {
            CreateFontW(
                16, // height
                0,  // width
                0,  // escapement
                0,  // orientation
                400, // weight (normal)
                0,  // italic
                0,  // underline
                0,  // strikeout
                DEFAULT_CHARSET,
                OUT_TT_ONLY_PRECIS,
                CLIP_DEFAULT_PRECIS,
                DEFAULT_QUALITY,
                FF_MODERN.0 as u32,
                windows::core::PCWSTR(font_name.as_ptr()),
            )
        };
        self.font.set(font);

        let ex_style = WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE;
        let hwnd = unsafe {
            CreateWindowExW(
                ex_style,
                windows::core::PCWSTR(class_name.as_ptr()),
                windows::core::PCWSTR(ptr::null()),
                WS_POPUP,
                0,
                0,
                1,
                1,
                None,
                None,
                Some(hinstance),
                None,
            )?
        };

        self.hwnd.set(hwnd);
        dbglog!("CandidateWindow::create: hwnd={:?}", hwnd);
        Ok(())
    }

    /// 候補を表示する
    ///
    /// `candidates` は現在のページの候補のみ（最大9件）。
    /// `selected` はページ内の選択インデックス。
    pub fn show(
        &self,
        candidates: &[CandidateDisplay],
        selected: usize,
        current_page: usize,
        total_pages: usize,
        position: &RECT,
    ) {
        let hwnd = self.hwnd.get();
        if hwnd == HWND::default() {
            return;
        }

        // 描画データをセット
        let data = Box::new(CandidateData {
            candidates: candidates.to_vec(),
            selected,
            current_page,
            total_pages,
            font: self.font.get(),
        });
        let data_ptr = Box::into_raw(data);

        // 前のデータを解放
        let old_ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) };
        if old_ptr != 0 {
            unsafe {
                drop(Box::from_raw(old_ptr as *mut CandidateData));
            }
        }
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, data_ptr as isize);
        }

        // ウィンドウサイズ計算
        let item_height = 22i32;
        let padding = 4i32;
        let count = candidates.len() as i32;
        // ページ表示用の行（2ページ以上の場合のみ）
        let footer_height = if total_pages > 1 { item_height } else { 0 };
        let width = 200i32;
        let height = count * item_height + padding * 2 + footer_height;

        // コンポジション位置の下に表示
        let x = position.left;
        let y = position.bottom;

        unsafe {
            let _ = MoveWindow(hwnd, x, y, width, height, false);
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            let _ = InvalidateRect(Some(hwnd), None, true);
        }
    }

    /// ウィンドウを非表示にする
    pub fn hide(&self) {
        let hwnd = self.hwnd.get();
        if hwnd == HWND::default() {
            return;
        }

        // データを解放
        let old_ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) };
        if old_ptr != 0 {
            unsafe {
                drop(Box::from_raw(old_ptr as *mut CandidateData));
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            }
        }

        unsafe {
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
    }

    /// ウィンドウを破棄する
    pub fn destroy(&self) {
        let hwnd = self.hwnd.get();
        if hwnd == HWND::default() {
            return;
        }

        // データを解放
        let old_ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) };
        if old_ptr != 0 {
            unsafe {
                drop(Box::from_raw(old_ptr as *mut CandidateData));
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            }
        }

        // フォントを破棄
        let font = self.font.get();
        if font != HFONT::default() {
            unsafe {
                let _ = DeleteObject(HGDIOBJ(font.0));
            }
            self.font.set(HFONT::default());
        }

        unsafe {
            let _ = DestroyWindow(hwnd);
        }
        self.hwnd.set(HWND::default());
    }
}

/// ウィンドウプロシージャ
unsafe extern "system" fn candidate_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_PAINT {
        paint_candidates(hwnd);
        return LRESULT(0);
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

/// 候補リストを描画
unsafe fn paint_candidates(hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(hwnd, &mut ps);

    let data_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const CandidateData;
    if data_ptr.is_null() {
        let _ = EndPaint(hwnd, &ps);
        return;
    }
    let data = &*data_ptr;

    // 事前作成されたフォントを使用
    let old_font = SelectObject(hdc, HGDIOBJ(data.font.0));

    let _ = SetBkMode(hdc, TRANSPARENT);

    let mut client_rect = RECT::default();
    let _ = GetClientRect(hwnd, &mut client_rect);

    // 背景: 白
    let bg_brush =
        HBRUSH(GetStockObject(windows::Win32::Graphics::Gdi::WHITE_BRUSH).0);
    FillRect(hdc, &client_rect, bg_brush);

    let item_height = 22i32;
    let padding = 4i32;
    let text_x = padding + 4;

    // ハイライト用ブラシ
    let highlight_brush = windows::Win32::Graphics::Gdi::CreateSolidBrush(
        windows::Win32::Foundation::COLORREF(0x00FFD8A0), // 淡い青
    );

    for (i, candidate) in data.candidates.iter().enumerate() {
        let y = padding + (i as i32) * item_height;

        // 選択行のハイライト
        if i == data.selected {
            let highlight_rect = RECT {
                left: client_rect.left,
                top: y,
                right: client_rect.right,
                bottom: y + item_height,
            };
            FillRect(hdc, &highlight_rect, highlight_brush);
        }

        // テキスト色: 黒
        let _ = SetTextColor(
            hdc,
            windows::Win32::Foundation::COLORREF(0x00000000),
        );

        // "1. 漢字" 形式
        let label = if let Some(ref ann) = candidate.annotation {
            format!("{}. {} ; {}", i + 1, candidate.word, ann)
        } else {
            format!("{}. {}", i + 1, candidate.word)
        };
        let wide = to_wide(&label);
        let _ = TextOutW(hdc, text_x, y + 2, &wide);
    }

    // ページ表示（2ページ以上の場合）
    if data.total_pages > 1 {
        let footer_y = padding + (data.candidates.len() as i32) * item_height;
        let _ = SetTextColor(
            hdc,
            windows::Win32::Foundation::COLORREF(0x00808080), // グレー
        );
        let page_label = format!("[{}/{}]", data.current_page + 1, data.total_pages);
        let wide = to_wide(&page_label);
        let _ = TextOutW(hdc, text_x, footer_y + 2, &wide);
    }

    // クリーンアップ
    SelectObject(hdc, old_font);
    let _ = DeleteObject(HGDIOBJ(highlight_brush.0));
    let _ = EndPaint(hwnd, &ps);
}
