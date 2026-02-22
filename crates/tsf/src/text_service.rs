//! Koyubi SKK Text Service — ITfTextInputProcessor + ITfKeyEventSink + ITfCompositionSink
//!
//! TSF のメインエントリポイント。キー入力をエンジンに渡し、
//! 結果に応じてテキスト挿入やコンポジション操作を行う。

use std::cell::{Cell, RefCell};
use std::io::Write as _;
use std::path::PathBuf;
use std::rc::Rc;

use koyubi_engine::composer::SkkEngine;
use koyubi_engine::dict::Dictionary;
use koyubi_engine::EngineResponse;

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
    SendInput, INPUT, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
    VIRTUAL_KEY, VK_BACK, VK_CONTROL, VK_DELETE, VK_END, VK_SHIFT,
};
use windows::Win32::UI::TextServices::{
    ITfComposition, ITfCompositionSink, ITfCompositionSink_Impl, ITfContext, ITfEditSession,
    ITfKeyEventSink, ITfKeyEventSink_Impl, ITfKeystrokeMgr, ITfTextInputProcessor,
    ITfTextInputProcessor_Impl, ITfThreadMgr, TF_ES_READWRITE, TF_ES_SYNC,
};
use windows::core::{implement, Interface as _};
use windows_core::{BOOL, GUID, IUnknownImpl as _};

use crate::candidate_ui::CandidateWindow;
use crate::edit_session::{
    CommitEditSession, CommitWithCompositionEditSession, CompositionEditSession,
    EndCompositionEditSession, GetTextExtEditSession,
};
use crate::globals;
use crate::key_event::{self, EmacsAction};

/// Koyubi SKK Text Service
#[implement(ITfTextInputProcessor, ITfKeyEventSink, ITfCompositionSink)]
pub struct TextService {
    thread_mgr: RefCell<Option<ITfThreadMgr>>,
    client_id: Cell<u32>,
    engine: RefCell<SkkEngine>,
    composition: RefCell<Option<ITfComposition>>,
    candidate_window: RefCell<CandidateWindow>,
}

impl TextService {
    pub fn new() -> Self {
        globals::inc_ref_count();
        Self {
            thread_mgr: RefCell::new(None),
            client_id: Cell::new(0),
            engine: RefCell::new(SkkEngine::new()),
            composition: RefCell::new(None),
            candidate_window: RefCell::new(CandidateWindow::new()),
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
    if let Ok(appdata) = std::env::var("APPDATA") {
        let path = PathBuf::from(appdata)
            .join("Koyubi")
            .join("dict")
            .join("user-dict.skk");
        dbglog!("User dictionary path: {:?}", path);
        engine.set_user_dictionary(path);
    }
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

        // 辞書読み込み
        load_dictionaries(&mut self.engine.borrow_mut());

        // ユーザー辞書設定
        load_user_dictionary(&mut self.engine.borrow_mut());

        // 候補ウィンドウ作成
        let hinstance = HINSTANCE(globals::dll_instance().0);
        if let Err(e) = self.candidate_window.borrow().create(hinstance) {
            dbglog!("CandidateWindow::create failed: {:?}", e);
        }

        Ok(())
    }

    fn Deactivate(&self) -> windows::core::Result<()> {
        // 候補ウィンドウ破棄
        self.candidate_window.borrow().destroy();

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
        let eat = key_event::should_eat_key(wparam, lparam, &self.engine.borrow());
        dbglog!("OnTestKeyDown: vk=0x{:02X} eat={} mode={:?}", wparam.0, eat, self.engine.borrow().current_mode());
        Ok(BOOL(eat as i32))
    }

    fn OnTestKeyUp(
        &self,
        _pic: windows::core::Ref<'_, ITfContext>,
        _wparam: WPARAM,
        _lparam: LPARAM,
    ) -> windows::core::Result<BOOL> {
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

        // Emacs キーバインド処理（エンジンの前にチェック）
        // Ctrl+H はエンジン経由で処理するため除外（▽モードでの reading 削除等）
        {
            let mut kbd_state = [0u8; 256];
            unsafe {
                if windows::Win32::UI::Input::KeyboardAndMouse::GetKeyboardState(
                    &mut kbd_state,
                )
                .is_err()
                {
                    kbd_state.fill(0);
                }
            }
            let ctrl = kbd_state[VK_CONTROL.0 as usize] & 0x80 != 0;
            let shift = kbd_state[VK_SHIFT.0 as usize] & 0x80 != 0;

            if ctrl && !shift {
                match key_event::emacs_action(vk) {
                    Some(EmacsAction::SimulateKey(target)) => {
                        dbglog!("OnKeyDown: Emacs SimulateKey vk=0x{:02X} -> target=0x{:04X}", vk, target.0);
                        send_simulated_key(target);
                        return Ok(BOOL(1));
                    }
                    Some(EmacsAction::KillLine) => {
                        dbglog!("OnKeyDown: Emacs KillLine");
                        send_kill_line();
                        return Ok(BOOL(1));
                    }
                    None => {} // H or non-Emacs key → fall through to engine
                }
            }
        }

        let key_event = match key_event::to_key_event(wparam, lparam) {
            Some(ev) => {
                dbglog!("OnKeyDown: vk=0x{:02X} -> {:?}", wparam.0, ev);
                ev
            }
            None => {
                dbglog!("OnKeyDown: vk=0x{:02X} -> None (ignored)", wparam.0);
                return Ok(BOOL(0));
            }
        };

        let response = self.engine.borrow_mut().process_key(key_event);
        dbglog!("OnKeyDown: response={:?}", response);

        match response {
            EngineResponse::PassThrough => {
                // Ctrl+H: エンジンが PassThrough を返した場合、VK_BACK をシミュレート
                // (wparam=0x48 は should_eat_key で消費済みなのでアプリには届かない。
                //  代わりに VK_BACK を送信してバックスペースとして機能させる)
                if vk == 0x48 {
                    dbglog!("OnKeyDown: Ctrl+H PassThrough -> simulate VK_BACK");
                    send_simulated_key(VK_BACK);
                    return Ok(BOOL(1));
                }
                return Ok(BOOL(0));
            }
            EngineResponse::Commit(text) => {
                self.do_commit_text(&context, &text)?;
            }
            EngineResponse::UpdateComposition { .. } | EngineResponse::Consumed => {
                // Handled by sync below
            }
        }

        // TSF コンポジション状態をエンジン状態と同期
        let comp_text = self.engine.borrow().composition_text();
        if let Some(text) = comp_text {
            let sink: ITfCompositionSink = self.to_interface();
            self.do_update_composition(&context, &text, sink)?;
        } else if self.composition.borrow().is_some() {
            self.do_end_composition(&context)?;
        }

        // 候補ウィンドウの表示/非表示を同期
        self.sync_candidate_window(&context);

        Ok(BOOL(1))
    }

    fn OnKeyUp(
        &self,
        _pic: windows::core::Ref<'_, ITfContext>,
        _wparam: WPARAM,
        _lparam: LPARAM,
    ) -> windows::core::Result<BOOL> {
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
        *self.composition.borrow_mut() = None;
        self.candidate_window.borrow().hide();
        Ok(())
    }
}
