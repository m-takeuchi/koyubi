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

use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::TextServices::{
    ITfComposition, ITfCompositionSink, ITfCompositionSink_Impl, ITfContext, ITfEditSession,
    ITfKeyEventSink, ITfKeyEventSink_Impl, ITfKeystrokeMgr, ITfTextInputProcessor,
    ITfTextInputProcessor_Impl, ITfThreadMgr, TF_ES_READWRITE, TF_ES_SYNC,
};
use windows::core::{implement, Interface as _};
use windows_core::{BOOL, GUID, IUnknownImpl as _};

use crate::edit_session::{
    CommitEditSession, CommitWithCompositionEditSession, CompositionEditSession,
    EndCompositionEditSession,
};
use crate::globals;
use crate::key_event;

/// Koyubi SKK Text Service
#[implement(ITfTextInputProcessor, ITfKeyEventSink, ITfCompositionSink)]
pub struct TextService {
    thread_mgr: RefCell<Option<ITfThreadMgr>>,
    client_id: Cell<u32>,
    engine: RefCell<SkkEngine>,
    composition: RefCell<Option<ITfComposition>>,
}

impl TextService {
    pub fn new() -> Self {
        globals::inc_ref_count();
        Self {
            thread_mgr: RefCell::new(None),
            client_id: Cell::new(0),
            engine: RefCell::new(SkkEngine::new()),
            composition: RefCell::new(None),
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

        Ok(())
    }

    fn Deactivate(&self) -> windows::core::Result<()> {
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
            EngineResponse::PassThrough => return Ok(BOOL(0)),
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
        Ok(())
    }
}
