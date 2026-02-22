//! Koyubi SKK Text Service — ITfTextInputProcessor + ITfKeyEventSink + ITfCompositionSink
//!
//! TSF のメインエントリポイント。キー入力をエンジンに渡し、
//! 結果に応じてテキスト挿入やコンポジション操作を行う。

use std::cell::{Cell, RefCell};
use std::io::Write as _;
use std::path::PathBuf;
use std::rc::Rc;

use koyubi_engine::composer::SkkEngine;
use koyubi_engine::config::Config;
use koyubi_engine::dict::Dictionary;
use koyubi_engine::{EngineResponse, InputMode};

use windows::Win32::System::LibraryLoader::GetModuleFileNameW;

/// デバッグログをファイルに書き出す
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

use windows::Win32::Foundation::{HINSTANCE, LPARAM, RECT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyboardState, SendInput, INPUT, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_KEYUP, VIRTUAL_KEY, VK_BACK, VK_CONTROL, VK_DELETE, VK_END, VK_MENU, VK_SHIFT,
    VK_SPACE,
};
use windows::Win32::UI::WindowsAndMessaging::GetMessageExtraInfo;
use windows::Win32::UI::TextServices::{
    ITfComposition, ITfCompositionSink, ITfCompositionSink_Impl, ITfContext, ITfEditSession,
    ITfKeyEventSink, ITfKeyEventSink_Impl, ITfKeystrokeMgr, ITfLangBarItemButton,
    ITfTextInputProcessor, ITfTextInputProcessor_Impl, ITfThreadMgr, TF_ES_READWRITE, TF_ES_SYNC,
};
use windows::core::{implement, Interface as _};
use windows_core::{AsImpl as _, BOOL, GUID, IUnknownImpl as _};

use crate::candidate_ui::CandidateWindow;
use crate::edit_session::{
    CommitEditSession, CommitWithCompositionEditSession, CompositionEditSession,
    EndCompositionEditSession, GetTextExtEditSession,
};
use crate::globals;
use crate::key_event::{self, EmacsAction};
use crate::lang_bar::{self, LangBarButton};

/// SandS (Space and Shift) の状態
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SandsState {
    /// Space 非押下
    Idle,
    /// Space 押下、他キーまだなし
    SpaceDown,
    /// Space+他キーが発生 → Space は Shift として使用中
    ShiftActive,
}

/// SendInput で注入したキーを識別するためのタグ（"KYUB"）
const SANDS_INJECTED: usize = 0x4B595542;

/// Koyubi SKK Text Service
#[implement(ITfTextInputProcessor, ITfKeyEventSink, ITfCompositionSink)]
pub struct TextService {
    thread_mgr: RefCell<Option<ITfThreadMgr>>,
    client_id: Cell<u32>,
    engine: RefCell<SkkEngine>,
    composition: RefCell<Option<ITfComposition>>,
    candidate_window: RefCell<CandidateWindow>,
    lang_bar_button: RefCell<Option<ITfLangBarItemButton>>,
    sands_state: Cell<SandsState>,
}

impl TextService {
    pub fn new() -> Self {
        globals::inc_ref_count();
        Self {
            thread_mgr: RefCell::new(None),
            client_id: Cell::new(0),
            engine: RefCell::new(SkkEngine::default()),
            composition: RefCell::new(None),
            candidate_window: RefCell::new(CandidateWindow::new()),
            lang_bar_button: RefCell::new(None),
            sands_state: Cell::new(SandsState::Idle),
        }
    }

    /// 確定テキストを挿入する
    ///
    /// コンポジションがある場合はその範囲を置換して終了。
    /// ない場合はカーソル位置に直接挿入。
    fn do_commit_text(&self, context: &ITfContext, text: &str) -> windows::core::Result<()> {
        dbglog!("do_commit_text: text={:?} has_composition={}", text, self.composition.borrow().is_some());
        if let Some(comp) = self.composition.borrow_mut().take() {
            let session = CommitWithCompositionEditSession::new(context.clone(), comp, text.to_string());
            let session: ITfEditSession = session.into();
            unsafe {
                let hr = context.RequestEditSession(
                    self.client_id.get(),
                    &session,
                    TF_ES_SYNC | TF_ES_READWRITE,
                )?;
                dbglog!("do_commit_text (with comp): RequestEditSession hr={:?}", hr);
            }
        } else {
            let session = CommitEditSession::new(context.clone(), text.to_string());
            let session: ITfEditSession = session.into();
            unsafe {
                let hr = context.RequestEditSession(
                    self.client_id.get(),
                    &session,
                    TF_ES_SYNC | TF_ES_READWRITE,
                )?;
                dbglog!("do_commit_text (no comp): RequestEditSession hr={:?}", hr);
            }
        }
        Ok(())
    }

    /// コンポジションを開始または更新する
    fn do_update_composition(
        &self,
        context: &ITfContext,
        display: &str,
        sink: ITfCompositionSink,
    ) -> windows::core::Result<()> {
        let result = Rc::new(RefCell::new(None));
        let existing = self.composition.borrow().clone();
        let session = CompositionEditSession::new(
            context.clone(),
            display.to_string(),
            existing,
            sink,
            result.clone(),
        );
        let session: ITfEditSession = session.into();
        unsafe {
            let _ = context.RequestEditSession(
                self.client_id.get(),
                &session,
                TF_ES_SYNC | TF_ES_READWRITE,
            )?;
        }
        *self.composition.borrow_mut() = result.borrow().clone();
        Ok(())
    }

    /// コンポジションを終了する（キャンセル時）
    fn do_end_composition(&self, context: &ITfContext) -> windows::core::Result<()> {
        if let Some(comp) = self.composition.borrow_mut().take() {
            let session = EndCompositionEditSession::new(comp);
            let session: ITfEditSession = session.into();
            unsafe {
                let _ = context.RequestEditSession(
                    self.client_id.get(),
                    &session,
                    TF_ES_SYNC | TF_ES_READWRITE,
                )?;
            }
        }
        Ok(())
    }

    /// コンポジション位置のスクリーン座標を取得する
    fn get_text_ext(&self, context: &ITfContext) -> Option<RECT> {
        let comp = self.composition.borrow().clone()?;
        let result = Rc::new(RefCell::new(None));
        let session =
            GetTextExtEditSession::new(context.clone(), comp, result.clone());
        let session: ITfEditSession = session.into();
        unsafe {
            let _ = context.RequestEditSession(
                self.client_id.get(),
                &session,
                TF_ES_SYNC | TF_ES_READWRITE,
            );
        }
        let val = result.borrow().clone();
        val
    }

    /// エンジンのレスポンスを処理し、TSF 状態を同期する
    ///
    /// `comp_sink` は TSF コンポジション更新に必要。呼び出し元で `self.to_interface()` して渡す。
    fn handle_engine_response(
        &self,
        context: &ITfContext,
        response: EngineResponse,
        vk: u16,
        comp_sink: ITfCompositionSink,
    ) -> windows::core::Result<BOOL> {
        match response {
            EngineResponse::PassThrough => {
                // Ctrl+H: エンジンが PassThrough を返した場合、VK_BACK をシミュレート
                if vk == 0x48 {
                    dbglog!("handle_engine_response: Ctrl+H PassThrough -> simulate VK_BACK");
                    send_simulated_key(VK_BACK);
                    return Ok(BOOL(1));
                }
                return Ok(BOOL(0));
            }
            EngineResponse::Commit(text) => {
                self.do_commit_text(context, &text)?;
            }
            EngineResponse::UpdateComposition { .. } | EngineResponse::Consumed => {
                // Handled by sync below
            }
        }

        // TSF コンポジション状態をエンジン状態と同期
        let comp_text = self.engine.borrow().composition_text();
        if let Some(text) = comp_text {
            self.do_update_composition(context, &text, comp_sink)?;
        } else if self.composition.borrow().is_some() {
            self.do_end_composition(context)?;
        }

        // 候補ウィンドウの表示/非表示を同期
        self.sync_candidate_window(context);

        // 言語バーにモード変更を通知
        self.notify_mode_change();

        Ok(BOOL(1))
    }

    /// 通常のキー処理パス（Emacs キーバインド + エンジン）
    fn process_key_normal(
        &self,
        context: &ITfContext,
        wparam: WPARAM,
        lparam: LPARAM,
        comp_sink: ITfCompositionSink,
    ) -> windows::core::Result<BOOL> {
        let vk = wparam.0 as u16;

        // Emacs キーバインド処理（エンジンの前にチェック）
        // Ctrl+H はエンジン経由で処理するため除外
        if self.engine.borrow().config().emacs_bindings_enabled {
            let mut kbd_state = [0u8; 256];
            unsafe {
                if GetKeyboardState(&mut kbd_state).is_err() {
                    kbd_state.fill(0);
                }
            }
            let ctrl = kbd_state[VK_CONTROL.0 as usize] & 0x80 != 0;
            let shift = kbd_state[VK_SHIFT.0 as usize] & 0x80 != 0;

            if ctrl && !shift {
                match key_event::emacs_action(vk) {
                    Some(EmacsAction::SimulateKey(target)) => {
                        dbglog!("process_key_normal: Emacs SimulateKey vk=0x{:02X} -> target=0x{:04X}", vk, target.0);
                        send_simulated_key(target);
                        return Ok(BOOL(1));
                    }
                    Some(EmacsAction::KillLine) => {
                        dbglog!("process_key_normal: Emacs KillLine");
                        send_kill_line();
                        return Ok(BOOL(1));
                    }
                    None => {}
                }
            }
        }

        let key_event = match key_event::to_key_event(wparam, lparam) {
            Some(ev) => {
                dbglog!("process_key_normal: vk=0x{:02X} -> {:?}", wparam.0, ev);
                ev
            }
            None => {
                dbglog!("process_key_normal: vk=0x{:02X} -> None (ignored)", wparam.0);
                return Ok(BOOL(0));
            }
        };

        let response = self.engine.borrow_mut().process_key(key_event);
        dbglog!("process_key_normal: response={:?}", response);

        self.handle_engine_response(context, response, vk, comp_sink)
    }

    /// 言語バーボタンにモード変更を通知する
    fn notify_mode_change(&self) {
        if let Some(ref button) = *self.lang_bar_button.borrow() {
            let mode = self.engine.borrow().current_mode();
            let impl_ref: &LangBarButton = unsafe { button.as_impl() };
            impl_ref.update_mode(mode);
        }
    }

    /// 候補ウィンドウの表示/非表示を同期する
    fn sync_candidate_window(&self, context: &ITfContext) {
        let info = self.engine.borrow().candidate_info();
        if let Some(info) = info {
            let rect = self.get_text_ext(context).unwrap_or(RECT {
                left: 100,
                top: 100,
                right: 200,
                bottom: 120,
            });
            // ページ計算: 9候補/ページ
            let page_start = (info.selected / 9) * 9;
            let page_end = (page_start + 9).min(info.candidates.len());
            let page_candidates = &info.candidates[page_start..page_end];
            let selected_in_page = info.selected - page_start;
            let total_pages = (info.candidates.len() + 8) / 9;
            let current_page = info.selected / 9;
            self.candidate_window.borrow().show(
                page_candidates,
                selected_in_page,
                current_page,
                total_pages,
                &rect,
            );
        } else {
            self.candidate_window.borrow().hide();
        }
    }
}

impl Drop for TextService {
    fn drop(&mut self) {
        globals::dec_ref_count();
    }
}

/// DLL のパスからディレクトリを取得
fn get_dll_directory() -> Option<PathBuf> {
    let hmodule = globals::dll_instance();
    let mut buf = [0u16; 260];
    let len = unsafe { GetModuleFileNameW(Some(hmodule), &mut buf) } as usize;
    if len == 0 {
        return None;
    }
    let path = PathBuf::from(String::from_utf16_lossy(&buf[..len]));
    path.parent().map(|p| p.to_path_buf())
}

/// 辞書を検索パスから読み込む
fn load_dictionaries(engine: &mut SkkEngine) {
    // config で明示的にパスが指定されている場合はそれを使用
    let config_paths = engine.config().system_dict_paths.clone();
    if !config_paths.is_empty() {
        for path_str in &config_paths {
            let path = PathBuf::from(path_str);
            match Dictionary::load(&path) {
                Ok(dict) => {
                    dbglog!(
                        "Dictionary loaded (config): {:?} ({} entries)",
                        path,
                        dict.entry_count()
                    );
                    engine.add_dictionary(dict);
                }
                Err(e) => {
                    dbglog!("Dictionary not found (config): {:?} ({:?})", path, e);
                }
            }
        }
        return;
    }

    // 自動検出
    let mut search_paths = Vec::new();

    // 1. DLL と同じディレクトリ
    if let Some(dll_dir) = get_dll_directory() {
        search_paths.push(dll_dir.join("SKK-JISYO.L"));
        // 2. ワークスペースルート（開発用: DLL は target/x86_64-.../release/ の3階層下）
        if let Some(workspace) = dll_dir
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
        {
            search_paths.push(workspace.join("dict").join("SKK-JISYO.L"));
        }
    }

    // 3. %APPDATA%\Koyubi\dict\
    if let Ok(appdata) = std::env::var("APPDATA") {
        search_paths.push(
            PathBuf::from(appdata)
                .join("Koyubi")
                .join("dict")
                .join("SKK-JISYO.L"),
        );
    }

    for path in &search_paths {
        match Dictionary::load(path) {
            Ok(dict) => {
                dbglog!(
                    "Dictionary loaded: {:?} ({} entries)",
                    path,
                    dict.entry_count()
                );
                engine.add_dictionary(dict);
                return; // 最初に見つかった辞書を使用
            }
            Err(e) => {
                dbglog!("Dictionary not found: {:?} ({:?})", path, e);
            }
        }
    }
    dbglog!("WARNING: No dictionary found");
}

/// ユーザー辞書パスを設定
fn load_user_dictionary(engine: &mut SkkEngine) {
    let path = if let Some(ref user_path) = engine.config().user_dict_path {
        PathBuf::from(user_path)
    } else if let Ok(appdata) = std::env::var("APPDATA") {
        PathBuf::from(appdata)
            .join("Koyubi")
            .join("dict")
            .join("user-dict.skk")
    } else {
        return;
    };
    dbglog!("User dictionary path: {:?}", path);
    engine.set_user_dictionary(path);
}

/// SendInput 用のヘルパー: キーイベントを1つ作成
fn ki(vk: VIRTUAL_KEY, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    let mut input = INPUT {
        r#type: INPUT_KEYBOARD,
        ..Default::default()
    };
    input.Anonymous.ki = KEYBDINPUT {
        wVk: vk,
        dwFlags: flags,
        ..Default::default()
    };
    input
}

/// 単一キーの SendInput シミュレーション
///
/// Ctrl が物理的に押されたままだとアプリが Ctrl+キー と解釈するため、
/// Ctrl を一時的に離してからキーを送信し、再度 Ctrl を押す。
fn send_simulated_key(vk: VIRTUAL_KEY) {
    let inputs = [
        ki(VK_CONTROL, KEYEVENTF_KEYUP),     // Ctrl を離す
        ki(vk, KEYBD_EVENT_FLAGS(0)),          // キー押下
        ki(vk, KEYEVENTF_KEYUP),              // キー離す
        ki(VK_CONTROL, KEYBD_EVENT_FLAGS(0)), // Ctrl を再度押す
    ];
    unsafe {
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

/// 行末まで削除（Ctrl+K）: Shift+End で選択 → Delete で削除
///
/// Ctrl を離す前に Shift を押し始めることで、Ctrl→無修飾→Shift の遷移を避ける。
/// （Windows が Ctrl UP + Shift DOWN を IME 切り替えシーケンスと誤認するのを防止）
fn send_kill_line() {
    let inputs = [
        ki(VK_SHIFT, KEYBD_EVENT_FLAGS(0)),   // Shift 押す（Ctrl+Shift 状態）
        ki(VK_CONTROL, KEYEVENTF_KEYUP),     // Ctrl を離す（Shift のみ）
        ki(VK_END, KEYBD_EVENT_FLAGS(0)),     // End 押す（Shift+End = 行末まで選択）
        ki(VK_END, KEYEVENTF_KEYUP),         // End 離す
        ki(VK_SHIFT, KEYEVENTF_KEYUP),       // Shift 離す
        ki(VK_DELETE, KEYBD_EVENT_FLAGS(0)),  // Delete 押す
        ki(VK_DELETE, KEYEVENTF_KEYUP),      // Delete 離す
        ki(VK_CONTROL, KEYBD_EVENT_FLAGS(0)), // Ctrl を再度押す
    ];
    unsafe {
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

/// SendInput 用のヘルパー: dwExtraInfo にタグを付与したキーイベント
///
/// SandS が注入したキーが TSF に再到達したとき、タグで識別してスルーさせる。
fn ki_tagged(vk: VIRTUAL_KEY, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    let mut input = INPUT {
        r#type: INPUT_KEYBOARD,
        ..Default::default()
    };
    input.Anonymous.ki = KEYBDINPUT {
        wVk: vk,
        dwFlags: flags,
        dwExtraInfo: SANDS_INJECTED,
        ..Default::default()
    };
    input
}

/// SandS: Shift+key を SendInput で注入する（Ascii モード用）
fn send_shifted_key(vk: VIRTUAL_KEY) {
    let inputs = [
        ki_tagged(VK_SHIFT, KEYBD_EVENT_FLAGS(0)),  // Shift 押下
        ki_tagged(vk, KEYBD_EVENT_FLAGS(0)),         // キー押下
        ki_tagged(vk, KEYEVENTF_KEYUP),              // キー離す
        ki_tagged(VK_SHIFT, KEYEVENTF_KEYUP),        // Shift 離す
    ];
    unsafe {
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

/// SandS: Space タップを SendInput で注入する（Ascii モード用）
fn send_space() {
    let inputs = [
        ki_tagged(VK_SPACE, KEYBD_EVENT_FLAGS(0)),   // Space 押下
        ki_tagged(VK_SPACE, KEYEVENTF_KEYUP),        // Space 離す
    ];
    unsafe {
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

// =========================================================
// ITfTextInputProcessor
// =========================================================

impl ITfTextInputProcessor_Impl for TextService_Impl {
    fn Activate(
        &self,
        ptim: windows::core::Ref<'_, ITfThreadMgr>,
        tid: u32,
    ) -> windows::core::Result<()> {
        dbglog!("Activate: tid={}", tid);
        let thread_mgr: ITfThreadMgr = ptim.ok()?.clone();

        // キーイベントシンクを登録
        let keystroke_mgr: ITfKeystrokeMgr = thread_mgr.cast()?;
        let key_sink: ITfKeyEventSink = self.to_interface();
        unsafe {
            keystroke_mgr.AdviseKeyEventSink(tid, &key_sink, true)?;
        }
        dbglog!("Activate: AdviseKeyEventSink OK");

        *self.thread_mgr.borrow_mut() = Some(thread_mgr);
        self.client_id.set(tid);

        // 設定ファイル読み込み
        let config = if let Ok(appdata) = std::env::var("APPDATA") {
            let config_path = PathBuf::from(&appdata).join("Koyubi").join("config.toml");
            dbglog!("Loading config from: {:?}", config_path);
            Config::load(&config_path)
        } else {
            dbglog!("APPDATA not set, using default config");
            Config::default()
        };
        dbglog!("Config: sands={}, emacs={}, initial_mode={:?}",
            config.sands_enabled, config.emacs_bindings_enabled, config.initial_mode);
        *self.engine.borrow_mut() = SkkEngine::new(config);

        // 辞書読み込み
        load_dictionaries(&mut self.engine.borrow_mut());

        // ユーザー辞書設定
        load_user_dictionary(&mut self.engine.borrow_mut());

        // 候補ウィンドウ作成
        let hinstance = HINSTANCE(globals::dll_instance().0);
        if let Err(e) = self.candidate_window.borrow().create(hinstance) {
            dbglog!("CandidateWindow::create failed: {:?}", e);
        }

        // 言語バーボタン登録
        let button = LangBarButton::new();
        let button_iface: ITfLangBarItemButton = button.into();
        if let Some(ref tm) = *self.thread_mgr.borrow() {
            let impl_ref: &LangBarButton = unsafe { button_iface.as_impl() };
            if let Err(e) = lang_bar::add_to_lang_bar(tm, &button_iface) {
                dbglog!("LangBarButton::add_to_lang_bar failed: {:?}", e);
            }
            impl_ref.update_mode(self.engine.borrow().current_mode());
        }
        *self.lang_bar_button.borrow_mut() = Some(button_iface);

        Ok(())
    }

    fn Deactivate(&self) -> windows::core::Result<()> {
        // 候補ウィンドウ破棄
        self.candidate_window.borrow().destroy();

        // 言語バーボタン削除
        if let Some(ref button) = *self.lang_bar_button.borrow() {
            if let Some(ref thread_mgr) = *self.thread_mgr.borrow() {
                let _ = lang_bar::remove_from_lang_bar(thread_mgr, button);
            }
        }
        *self.lang_bar_button.borrow_mut() = None;

        // コンポジションを破棄
        *self.composition.borrow_mut() = None;

        // キーイベントシンク登録解除
        let client_id = self.client_id.get();
        if let Some(ref thread_mgr) = *self.thread_mgr.borrow() {
            if let Ok(keystroke_mgr) = thread_mgr.cast::<ITfKeystrokeMgr>() {
                unsafe {
                    let _ = keystroke_mgr.UnadviseKeyEventSink(client_id);
                }
            }
        }

        // エンジン状態リセット
        self.engine.borrow_mut().reset_state();
        self.sands_state.set(SandsState::Idle);

        *self.thread_mgr.borrow_mut() = None;
        self.client_id.set(0);
        Ok(())
    }
}

// =========================================================
// ITfKeyEventSink
// =========================================================

impl ITfKeyEventSink_Impl for TextService_Impl {
    fn OnSetFocus(&self, _fforeground: BOOL) -> windows::core::Result<()> {
        Ok(())
    }

    fn OnTestKeyDown(
        &self,
        _pic: windows::core::Ref<'_, ITfContext>,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> windows::core::Result<BOOL> {
        let vk = wparam.0 as u16;
        let sands_enabled = self.engine.borrow().config().sands_enabled;

        // SandS が注入したキーはスルー
        if sands_enabled && unsafe { GetMessageExtraInfo() }.0 as usize == SANDS_INJECTED {
            dbglog!("OnTestKeyDown: vk=0x{:02X} SANDS_INJECTED -> pass", vk);
            return Ok(BOOL(0));
        }

        // Space キー（Ctrl なし）→ SandS で消費
        if sands_enabled && vk == VK_SPACE.0 {
            let mut kbd_state = [0u8; 256];
            unsafe {
                if GetKeyboardState(&mut kbd_state).is_err() {
                    kbd_state.fill(0);
                }
            }
            let ctrl = kbd_state[VK_CONTROL.0 as usize] & 0x80 != 0;
            if !ctrl {
                dbglog!("OnTestKeyDown: Space (SandS) -> eat");
                return Ok(BOOL(1));
            }
        }

        // SandS が SpaceDown/ShiftActive のとき、非修飾キーを消費
        if sands_enabled {
            let sands = self.sands_state.get();
            if sands == SandsState::SpaceDown || sands == SandsState::ShiftActive {
                // 修飾キー単体は除外
                if vk != VK_SHIFT.0 && vk != VK_CONTROL.0 && vk != VK_MENU.0 {
                    let mut kbd_state = [0u8; 256];
                    unsafe {
                        if GetKeyboardState(&mut kbd_state).is_err() {
                            kbd_state.fill(0);
                        }
                    }
                    let ctrl = kbd_state[VK_CONTROL.0 as usize] & 0x80 != 0;
                    let alt = kbd_state[VK_MENU.0 as usize] & 0x80 != 0;
                    if !ctrl && !alt {
                        dbglog!("OnTestKeyDown: vk=0x{:02X} SandS {:?} -> eat", vk, sands);
                        return Ok(BOOL(1));
                    }
                }
            }
        }

        // 通常の判定
        let eat = key_event::should_eat_key(wparam, lparam, &self.engine.borrow());
        dbglog!("OnTestKeyDown: vk=0x{:02X} eat={} mode={:?}", wparam.0, eat, self.engine.borrow().current_mode());
        Ok(BOOL(eat as i32))
    }

    fn OnTestKeyUp(
        &self,
        _pic: windows::core::Ref<'_, ITfContext>,
        wparam: WPARAM,
        _lparam: LPARAM,
    ) -> windows::core::Result<BOOL> {
        let vk = wparam.0 as u16;

        // Space up + SandS 状態 → 消費
        if self.engine.borrow().config().sands_enabled && vk == VK_SPACE.0 {
            let sands = self.sands_state.get();
            if sands == SandsState::SpaceDown || sands == SandsState::ShiftActive {
                dbglog!("OnTestKeyUp: Space SandS {:?} -> eat", sands);
                return Ok(BOOL(1));
            }
        }

        Ok(BOOL(0))
    }

    fn OnKeyDown(
        &self,
        pic: windows::core::Ref<'_, ITfContext>,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> windows::core::Result<BOOL> {
        let context: ITfContext = pic.ok()?.clone();
        let vk = wparam.0 as u16;
        let sands_enabled = self.engine.borrow().config().sands_enabled;

        // SandS が注入したキーはスルー
        if sands_enabled && unsafe { GetMessageExtraInfo() }.0 as usize == SANDS_INJECTED {
            dbglog!("OnKeyDown: vk=0x{:02X} SANDS_INJECTED -> pass", vk);
            return Ok(BOOL(0));
        }

        // Space キー（Ctrl なし）→ SandS 状態遷移
        if sands_enabled && vk == VK_SPACE.0 {
            let mut kbd_state = [0u8; 256];
            unsafe {
                if GetKeyboardState(&mut kbd_state).is_err() {
                    kbd_state.fill(0);
                }
            }
            let ctrl = kbd_state[VK_CONTROL.0 as usize] & 0x80 != 0;
            if !ctrl {
                let is_repeat = (lparam.0 >> 30) & 1 != 0;
                if is_repeat {
                    // リピートは無視
                    dbglog!("OnKeyDown: Space repeat -> ignore");
                    return Ok(BOOL(1));
                }
                dbglog!("OnKeyDown: Space -> SandS SpaceDown");
                self.sands_state.set(SandsState::SpaceDown);
                return Ok(BOOL(1));
            }
        }

        // SandS が SpaceDown/ShiftActive のとき、非修飾キーを Shift 付きで処理
        let sands = self.sands_state.get();
        if sands_enabled && (sands == SandsState::SpaceDown || sands == SandsState::ShiftActive) {
            if vk != VK_SHIFT.0 && vk != VK_CONTROL.0 && vk != VK_MENU.0 {
                let mut kbd_state = [0u8; 256];
                unsafe {
                    if GetKeyboardState(&mut kbd_state).is_err() {
                        kbd_state.fill(0);
                    }
                }
                let ctrl = kbd_state[VK_CONTROL.0 as usize] & 0x80 != 0;
                let alt = kbd_state[VK_MENU.0 as usize] & 0x80 != 0;
                if !ctrl && !alt {
                    self.sands_state.set(SandsState::ShiftActive);
                    dbglog!("OnKeyDown: vk=0x{:02X} SandS -> ShiftActive", vk);

                    // Ascii モード → SendInput で Shift+key を注入
                    if self.engine.borrow().current_mode() == InputMode::Ascii {
                        dbglog!("OnKeyDown: SandS Ascii -> send_shifted_key(0x{:02X})", vk);
                        send_shifted_key(VIRTUAL_KEY(vk));
                        return Ok(BOOL(1));
                    }

                    // 非 Ascii → Shift 強制でエンジンに渡す
                    let key_event = match key_event::to_key_event_with_forced_shift(wparam, lparam) {
                        Some(ev) => {
                            dbglog!("OnKeyDown: SandS forced shift -> {:?}", ev);
                            ev
                        }
                        None => {
                            dbglog!("OnKeyDown: SandS forced shift -> None (ignored)");
                            return Ok(BOOL(0));
                        }
                    };

                    let response = self.engine.borrow_mut().process_key(key_event);
                    dbglog!("OnKeyDown: SandS response={:?}", response);
                    let sink: ITfCompositionSink = self.to_interface();
                    return self.handle_engine_response(&context, response, vk, sink);
                }
            }
        }

        // 通常のキー処理
        let sink: ITfCompositionSink = self.to_interface();
        self.process_key_normal(&context, wparam, lparam, sink)
    }

    fn OnKeyUp(
        &self,
        pic: windows::core::Ref<'_, ITfContext>,
        wparam: WPARAM,
        _lparam: LPARAM,
    ) -> windows::core::Result<BOOL> {
        let vk = wparam.0 as u16;

        if self.engine.borrow().config().sands_enabled && vk == VK_SPACE.0 {
            let sands = self.sands_state.get();
            match sands {
                SandsState::SpaceDown => {
                    // Space タップ: 他キーなしで Space を離した
                    self.sands_state.set(SandsState::Idle);
                    dbglog!("OnKeyUp: Space tap (SpaceDown -> Idle)");

                    // Ascii モード → SendInput で Space を注入
                    if self.engine.borrow().current_mode() == InputMode::Ascii {
                        dbglog!("OnKeyUp: SandS Ascii -> send_space()");
                        send_space();
                        return Ok(BOOL(1));
                    }

                    // 非 Ascii → エンジンに Space を渡す
                    let context: ITfContext = pic.ok()?.clone();
                    let key_event = koyubi_engine::KeyEvent {
                        key: koyubi_engine::Key::Space,
                        shift: false,
                        ctrl: false,
                        alt: false,
                    };
                    let response = self.engine.borrow_mut().process_key(key_event);
                    dbglog!("OnKeyUp: Space tap response={:?}", response);
                    let sink: ITfCompositionSink = self.to_interface();
                    return self.handle_engine_response(&context, response, VK_SPACE.0, sink);
                }
                SandsState::ShiftActive => {
                    // Space+他キーが発生した後の Space up → 抑制
                    self.sands_state.set(SandsState::Idle);
                    dbglog!("OnKeyUp: Space (ShiftActive -> Idle), suppress");
                    return Ok(BOOL(1));
                }
                SandsState::Idle => {}
            }
        }

        Ok(BOOL(0))
    }

    fn OnPreservedKey(
        &self,
        _pic: windows::core::Ref<'_, ITfContext>,
        _rguid: *const GUID,
    ) -> windows::core::Result<BOOL> {
        Ok(BOOL(0))
    }
}

// =========================================================
// ITfCompositionSink
// =========================================================

impl ITfCompositionSink_Impl for TextService_Impl {
    fn OnCompositionTerminated(
        &self,
        _ecwrite: u32,
        _pcomposition: windows::core::Ref<'_, ITfComposition>,
    ) -> windows::core::Result<()> {
        // TSF がコンポジションを強制終了した場合（フォーカス移動等）
        // エンジン状態をリセットし、コンポジション参照をクリア
        self.engine.borrow_mut().reset_state();
        self.sands_state.set(SandsState::Idle);
        *self.composition.borrow_mut() = None;
        self.candidate_window.borrow().hide();
        Ok(())
    }
}
