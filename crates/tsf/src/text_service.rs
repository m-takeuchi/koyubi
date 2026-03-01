//! Koyubi SKK Text Service — ITfTextInputProcessor + ITfKeyEventSink + ITfCompositionSink
//!
//! TSF のメインエントリポイント。キー入力をエンジンに渡し、
//! 結果に応じてテキスト挿入やコンポジション操作を行う。

use std::cell::{Cell, RefCell};
use std::io::Write as _;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};

use koyubi_engine::composer::SkkEngine;
use koyubi_engine::config::Config;
use koyubi_engine::dict::Dictionary;
use koyubi_engine::{EngineResponse, InputMode};

use windows::Win32::System::LibraryLoader::GetModuleFileNameW;

/// デバッグログをファイルに書き出す
///
/// 環境変数 KOYUBI_DEBUG にファイルパスを設定すると有効になる。
/// 例: set KOYUBI_DEBUG=C:\koyubi\debug.log
/// 未設定の場合は何も出力しない（パフォーマンス影響なし）。
macro_rules! dbglog {
    ($($arg:tt)*) => {{
        use std::sync::OnceLock;
        static LOG_PATH: OnceLock<Option<String>> = OnceLock::new();
        let path = LOG_PATH.get_or_init(|| std::env::var("KOYUBI_DEBUG").ok());
        if let Some(ref path) = path {
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
            {
                use std::time::SystemTime;
                let now = SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap_or_default();
                let secs = now.as_secs() % 86400; // 時刻部分のみ
                let millis = now.subsec_millis();
                let _ = write!(f, "{:02}:{:02}:{:02}.{:03} ",
                    secs / 3600, (secs % 3600) / 60, secs % 60, millis);
                let _ = writeln!(f, $($arg)*);
            }
        }
    }};
}

use windows::Win32::Foundation::{HINSTANCE, LPARAM, RECT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetKeyboardState, SendInput, INPUT, INPUT_KEYBOARD, KEYBDINPUT,
    KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, VIRTUAL_KEY, VK_BACK, VK_CAPITAL, VK_CONTROL, VK_DELETE,
    VK_END, VK_MENU, VK_SHIFT, VK_SPACE,
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
    thumb_shift_held: Cell<bool>,
    caps_ctrl_held: Cell<bool>,
    dicts_loaded: Cell<bool>,
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
            thumb_shift_held: Cell::new(false),
            caps_ctrl_held: Cell::new(false),
            dicts_loaded: Cell::new(false),
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
        is_ctrl_h: bool,
        comp_sink: ITfCompositionSink,
    ) -> windows::core::Result<BOOL> {
        match response {
            EngineResponse::PassThrough => {
                // Ctrl+H: エンジンが PassThrough を返した場合、VK_BACK をシミュレート
                if is_ctrl_h {
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

        // Ctrl+H は to_key_event で Key::Backspace に変換される。
        // PassThrough 時に VK_BACK をシミュレートする必要があるため検出する。
        let is_ctrl_h = vk == 0x48 && matches!(key_event.key, koyubi_engine::Key::Backspace);

        let response = self.engine.borrow_mut().process_key(key_event);
        dbglog!("process_key_normal: response={:?}", response);

        self.handle_engine_response(context, response, is_ctrl_h, comp_sink)
    }

    /// 言語バーボタンにモード変更を通知する
    fn notify_mode_change(&self) {
        if let Some(ref button) = *self.lang_bar_button.borrow() {
            let mode = self.engine.borrow().current_mode();
            let impl_ref: &LangBarButton = unsafe { button.as_impl() };
            impl_ref.update_mode(mode);
        }
    }

    /// バックグラウンドでロードされた辞書をエンジンに追加する（ノンブロッキング）
    ///
    /// OnceLock::get() で辞書の準備状況をチェックし、
    /// ロード完了していればエンジンに追加する。ブロックしない。
    fn ensure_dictionaries_loaded(&self) {
        if self.dicts_loaded.get() {
            return;
        }
        if let Some(dicts) = CACHED_SYSTEM_DICTS.get() {
            for dict in dicts {
                self.engine.borrow_mut().add_dictionary(Arc::clone(dict));
            }
            self.dicts_loaded.set(true);
            dbglog!("Dictionaries attached to engine ({} dicts)", dicts.len());
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

/// プロセス内で辞書を共有するためのキャッシュ
///
/// TSF DLL はテキスト入力を使うすべてのプロセスに読み込まれるため、
/// 辞書のロード（4MB+ のファイル読み込み・パース）をプロセスあたり1回に抑える。
/// バックグラウンドスレッドで初期化し、UI スレッドをブロックしない。
static CACHED_SYSTEM_DICTS: OnceLock<Vec<Arc<Dictionary>>> = OnceLock::new();

/// 辞書のバックグラウンドプリロードを開始する
///
/// 別スレッドで辞書をロードし、CACHED_SYSTEM_DICTS に格納する。
/// Activate からの呼び出しで UI スレッドをブロックしない。
fn start_dict_preload(config: &Config) {
    if CACHED_SYSTEM_DICTS.get().is_some() {
        return; // 既にロード済み
    }
    let config = config.clone();
    std::thread::spawn(move || {
        CACHED_SYSTEM_DICTS.get_or_init(|| {
            load_dictionaries_from_disk(&config)
        });
        dbglog!("Dictionary preload completed in background thread");
    });
}

/// 辞書をディスクから読み込む（初回のみ呼ばれる）
fn load_dictionaries_from_disk(config: &Config) -> Vec<Arc<Dictionary>> {
    let mut result = Vec::new();

    // config で明示的にパスが指定されている場合はそれを使用
    if !config.system_dict_paths.is_empty() {
        for path_str in &config.system_dict_paths {
            let path = PathBuf::from(path_str);
            match Dictionary::load(&path) {
                Ok(dict) => {
                    dbglog!(
                        "Dictionary loaded (config): {:?} ({} entries)",
                        path,
                        dict.entry_count()
                    );
                    result.push(Arc::new(dict));
                }
                Err(e) => {
                    dbglog!("Dictionary not found (config): {:?} ({:?})", path, e);
                }
            }
        }
        return result;
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

    // 4. C:\Program Files\Koyubi\dict\ (インストーラ配置用)
    search_paths.push(PathBuf::from(r"C:\Program Files\Koyubi\dict\SKK-JISYO.L"));

    for path in &search_paths {
        match Dictionary::load(path) {
            Ok(dict) => {
                dbglog!(
                    "Dictionary loaded: {:?} ({} entries)",
                    path,
                    dict.entry_count()
                );
                result.push(Arc::new(dict));
                return result; // 最初に見つかった辞書を使用
            }
            Err(e) => {
                dbglog!("Dictionary not found: {:?} ({:?})", path, e);
            }
        }
    }
    result
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
/// Ctrl を一時的に離してからキーを送信する。
/// Ctrl がまだ物理的に押されている場合のみ再度押す（stuck 防止）。
fn send_simulated_key(vk: VIRTUAL_KEY) {
    let ctrl_held = unsafe { GetAsyncKeyState(VK_CONTROL.0 as i32) } as u16 & 0x8000 != 0;
    let inputs = [
        ki(VK_CONTROL, KEYEVENTF_KEYUP),      // Ctrl を離す
        ki_tagged(vk, KEYBD_EVENT_FLAGS(0)),   // キー押下（タグ付き: IME 再処理防止）
        ki_tagged(vk, KEYEVENTF_KEYUP),        // キー離す
        ki(VK_CONTROL, KEYBD_EVENT_FLAGS(0)),  // Ctrl を再度押す
    ];
    let count = if ctrl_held { 4 } else { 3 };
    unsafe {
        SendInput(&inputs[..count], std::mem::size_of::<INPUT>() as i32);
    }
}

/// 行末まで削除（Ctrl+K）: Shift+End で選択 → Delete で削除
///
/// Ctrl を離す前に Shift を押し始めることで、Ctrl→無修飾→Shift の遷移を避ける。
/// （Windows が Ctrl UP + Shift DOWN を IME 切り替えシーケンスと誤認するのを防止）
/// Ctrl がまだ物理的に押されている場合のみ再度押す（stuck 防止）。
fn send_kill_line() {
    let ctrl_held = unsafe { GetAsyncKeyState(VK_CONTROL.0 as i32) } as u16 & 0x8000 != 0;
    let inputs = [
        ki(VK_SHIFT, KEYBD_EVENT_FLAGS(0)),          // Shift 押す（Ctrl+Shift 状態）
        ki(VK_CONTROL, KEYEVENTF_KEYUP),             // Ctrl を離す（Shift のみ）
        ki_tagged(VK_END, KEYBD_EVENT_FLAGS(0)),     // End 押す（Shift+End = 行末まで選択）
        ki_tagged(VK_END, KEYEVENTF_KEYUP),          // End 離す
        ki(VK_SHIFT, KEYEVENTF_KEYUP),               // Shift 離す
        ki_tagged(VK_DELETE, KEYBD_EVENT_FLAGS(0)),  // Delete 押す
        ki_tagged(VK_DELETE, KEYEVENTF_KEYUP),       // Delete 離す
        ki(VK_CONTROL, KEYBD_EVENT_FLAGS(0)),         // Ctrl を再度押す
    ];
    let count = if ctrl_held { 8 } else { 7 };
    unsafe {
        SendInput(&inputs[..count], std::mem::size_of::<INPUT>() as i32);
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

/// 単純なキー押下/離し（Ctrl 操作なし）
///
/// CapsLock→Ctrl の Emacs バインディング用。物理 Ctrl が押されていないため、
/// Ctrl のリリース/再押下は不要。
fn send_key_only(vk: VIRTUAL_KEY) {
    let inputs = [
        ki(vk, KEYBD_EVENT_FLAGS(0)),  // キー押下
        ki(vk, KEYEVENTF_KEYUP),       // キー離す
    ];
    unsafe {
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

/// 行末まで削除（Ctrl 操作なし、CapsLock→Ctrl 用）
///
/// 物理 Ctrl が押されていないので、Ctrl のリリース/再押下は不要。
fn send_kill_line_no_ctrl() {
    let inputs = [
        ki(VK_SHIFT, KEYBD_EVENT_FLAGS(0)),  // Shift 押す
        ki(VK_END, KEYBD_EVENT_FLAGS(0)),     // End 押す（Shift+End = 行末まで選択）
        ki(VK_END, KEYEVENTF_KEYUP),         // End 離す
        ki(VK_SHIFT, KEYEVENTF_KEYUP),       // Shift 離す
        ki(VK_DELETE, KEYBD_EVENT_FLAGS(0)),  // Delete 押す
        ki(VK_DELETE, KEYEVENTF_KEYUP),      // Delete 離す
    ];
    unsafe {
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

/// Ctrl+key を SendInput で注入する（CapsLock→Ctrl の非 IME コンボ用）
///
/// 物理 Ctrl が押されていないので、Ctrl を押してキーを送信し、Ctrl を離す。
fn send_ctrl_key(vk: VIRTUAL_KEY) {
    let inputs = [
        ki(VK_CONTROL, KEYBD_EVENT_FLAGS(0)), // Ctrl 押す
        ki(vk, KEYBD_EVENT_FLAGS(0)),          // キー押下
        ki(vk, KEYEVENTF_KEYUP),              // キー離す
        ki(VK_CONTROL, KEYEVENTF_KEYUP),      // Ctrl 離す
    ];
    unsafe {
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

/// Thumb Shift 対象キー判定（無変換/変換/カナ）
fn is_thumb_shift_key(vk: u16) -> bool {
    vk == 0x1D || vk == 0x1C || vk == 0x15
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
        *self.engine.borrow_mut() = SkkEngine::new(config.clone());
        self.dicts_loaded.set(false);

        // 辞書をバックグラウンドでプリロード（UI スレッドをブロックしない）
        start_dict_preload(&config);

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
        self.thumb_shift_held.set(false);
        self.caps_ctrl_held.set(false);

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

        // SendInput で注入したキーはスルー（SandS / CapsLock→Ctrl 共通）
        if unsafe { GetMessageExtraInfo() }.0 as usize == SANDS_INJECTED {
            dbglog!("OnTestKeyDown: vk=0x{:02X} SANDS_INJECTED -> pass", vk);
            return Ok(BOOL(0));
        }

        // CapsLock→Ctrl: VK_CAPITAL 自体を消費
        let caps_ctrl_enabled = self.engine.borrow().config().caps_ctrl_enabled;
        if caps_ctrl_enabled && vk == VK_CAPITAL.0 {
            dbglog!("OnTestKeyDown: CapsLock -> eat");
            return Ok(BOOL(1));
        }

        // CapsLock→Ctrl が押されているとき、非修飾キーを消費
        if caps_ctrl_enabled && self.caps_ctrl_held.get() {
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
                    dbglog!("OnTestKeyDown: vk=0x{:02X} caps_ctrl -> eat", vk);
                    return Ok(BOOL(1));
                }
            }
        }

        // Thumb Shift: 無変換/変換/カナキー自体を消費
        let thumb_shift_enabled = self.engine.borrow().config().thumb_shift_enabled;
        if thumb_shift_enabled && is_thumb_shift_key(vk) {
            dbglog!("OnTestKeyDown: thumb shift key 0x{:02X} -> eat", vk);
            return Ok(BOOL(1));
        }

        // Thumb Shift が押されているとき、非修飾キーを消費
        if thumb_shift_enabled && self.thumb_shift_held.get() {
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
                    dbglog!("OnTestKeyDown: vk=0x{:02X} thumb_shift -> eat", vk);
                    return Ok(BOOL(1));
                }
            }
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

        // CapsLock up → 消費
        if self.engine.borrow().config().caps_ctrl_enabled && vk == VK_CAPITAL.0 {
            dbglog!("OnTestKeyUp: CapsLock -> eat");
            return Ok(BOOL(1));
        }

        // Thumb Shift キー up → 消費
        if self.engine.borrow().config().thumb_shift_enabled && is_thumb_shift_key(vk) {
            dbglog!("OnTestKeyUp: thumb shift key 0x{:02X} -> eat", vk);
            return Ok(BOOL(1));
        }

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

        // バックグラウンドでロードされた辞書を取り込む（ノンブロッキング）
        self.ensure_dictionaries_loaded();

        // SendInput で注入したキーはスルー（SandS / CapsLock→Ctrl 共通）
        if unsafe { GetMessageExtraInfo() }.0 as usize == SANDS_INJECTED {
            dbglog!("OnKeyDown: vk=0x{:02X} SANDS_INJECTED -> pass", vk);
            return Ok(BOOL(0));
        }

        // CapsLock→Ctrl: VK_CAPITAL → caps_ctrl_held = true
        let caps_ctrl_enabled = self.engine.borrow().config().caps_ctrl_enabled;
        if caps_ctrl_enabled && vk == VK_CAPITAL.0 {
            self.caps_ctrl_held.set(true);
            dbglog!("OnKeyDown: CapsLock -> caps_ctrl_held=true");
            return Ok(BOOL(1));
        }

        // CapsLock→Ctrl が押されているとき、Ctrl 付きで処理
        if caps_ctrl_enabled && self.caps_ctrl_held.get() {
            if vk != VK_SHIFT.0 && vk != VK_CONTROL.0 && vk != VK_MENU.0 {
                let mut kbd_state = [0u8; 256];
                unsafe {
                    if GetKeyboardState(&mut kbd_state).is_err() {
                        kbd_state.fill(0);
                    }
                }
                let physical_ctrl = kbd_state[VK_CONTROL.0 as usize] & 0x80 != 0;
                let alt = kbd_state[VK_MENU.0 as usize] & 0x80 != 0;
                if !physical_ctrl && !alt {
                    dbglog!("OnKeyDown: vk=0x{:02X} caps_ctrl -> ctrl", vk);

                    // Emacs バインディング処理（Ctrl+H 以外）
                    if self.engine.borrow().config().emacs_bindings_enabled {
                        let shift = kbd_state[VK_SHIFT.0 as usize] & 0x80 != 0;
                        if !shift {
                            match key_event::emacs_action(vk) {
                                Some(EmacsAction::SimulateKey(target)) => {
                                    dbglog!("OnKeyDown: caps_ctrl Emacs SimulateKey -> 0x{:04X}", target.0);
                                    send_key_only(target);
                                    return Ok(BOOL(1));
                                }
                                Some(EmacsAction::KillLine) => {
                                    dbglog!("OnKeyDown: caps_ctrl Emacs KillLine");
                                    send_kill_line_no_ctrl();
                                    return Ok(BOOL(1));
                                }
                                None => {}
                            }
                        }
                    }

                    // エンジンに Ctrl 強制で渡す
                    let key_event = match key_event::to_key_event_with_forced_ctrl(wparam, lparam) {
                        Some(ev) => {
                            dbglog!("OnKeyDown: caps_ctrl forced ctrl -> {:?}", ev);
                            ev
                        }
                        None => {
                            dbglog!("OnKeyDown: caps_ctrl forced ctrl -> None (ignored)");
                            return Ok(BOOL(0));
                        }
                    };

                    let response = self.engine.borrow_mut().process_key(key_event);
                    dbglog!("OnKeyDown: caps_ctrl response={:?}", response);

                    // PassThrough の特殊処理
                    if let EngineResponse::PassThrough = &response {
                        // Ctrl+H → Backspace
                        if vk == 0x48 {
                            dbglog!("OnKeyDown: caps_ctrl Ctrl+H PassThrough -> send_key_only(VK_BACK)");
                            send_key_only(VK_BACK);
                            return Ok(BOOL(1));
                        }
                        // その他 → Ctrl+key をアプリに届ける（Ctrl+C/V/Z 等）
                        dbglog!("OnKeyDown: caps_ctrl PassThrough -> send_ctrl_key(0x{:02X})", vk);
                        send_ctrl_key(VIRTUAL_KEY(vk));
                        return Ok(BOOL(1));
                    }

                    let sink: ITfCompositionSink = self.to_interface();
                    return self.handle_engine_response(&context, response, false, sink);
                }
            }
        }

        // Thumb Shift: キー自体 → thumb_shift_held = true
        let thumb_shift_enabled = self.engine.borrow().config().thumb_shift_enabled;
        if thumb_shift_enabled && is_thumb_shift_key(vk) {
            self.thumb_shift_held.set(true);
            dbglog!("OnKeyDown: thumb shift key 0x{:02X} -> held=true", vk);
            return Ok(BOOL(1));
        }

        // Thumb Shift が押されているとき、非修飾キーを Shift 付きで処理
        if thumb_shift_enabled && self.thumb_shift_held.get() {
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
                    dbglog!("OnKeyDown: vk=0x{:02X} thumb_shift -> shift", vk);

                    // Ascii モード → SendInput で Shift+key を注入
                    if self.engine.borrow().current_mode() == InputMode::Ascii {
                        dbglog!("OnKeyDown: thumb_shift Ascii -> send_shifted_key(0x{:02X})", vk);
                        send_shifted_key(VIRTUAL_KEY(vk));
                        return Ok(BOOL(1));
                    }

                    // 非 Ascii → Shift 強制でエンジンに渡す
                    let key_event = match key_event::to_key_event_with_forced_shift(wparam, lparam) {
                        Some(ev) => {
                            dbglog!("OnKeyDown: thumb_shift forced shift -> {:?}", ev);
                            ev
                        }
                        None => {
                            dbglog!("OnKeyDown: thumb_shift forced shift -> None (ignored)");
                            return Ok(BOOL(0));
                        }
                    };

                    let response = self.engine.borrow_mut().process_key(key_event);
                    dbglog!("OnKeyDown: thumb_shift response={:?}", response);
                    let sink: ITfCompositionSink = self.to_interface();
                    return self.handle_engine_response(&context, response, false, sink);
                }
            }
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
                    return self.handle_engine_response(&context, response, false, sink);
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

        // CapsLock up → caps_ctrl_held=false
        if self.engine.borrow().config().caps_ctrl_enabled && vk == VK_CAPITAL.0 {
            self.caps_ctrl_held.set(false);
            dbglog!("OnKeyUp: CapsLock -> caps_ctrl_held=false");
            return Ok(BOOL(1));
        }

        // Thumb Shift キー up → held=false
        if self.engine.borrow().config().thumb_shift_enabled && is_thumb_shift_key(vk) {
            self.thumb_shift_held.set(false);
            dbglog!("OnKeyUp: thumb shift key 0x{:02X} -> held=false", vk);
            return Ok(BOOL(1));
        }

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
                    return self.handle_engine_response(&context, response, false, sink);
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
        self.thumb_shift_held.set(false);
        self.caps_ctrl_held.set(false);
        *self.composition.borrow_mut() = None;
        self.candidate_window.borrow().hide();
        Ok(())
    }
}
