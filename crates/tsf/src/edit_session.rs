//! TSF EditSession 実装
//!
//! TSF ではテキスト操作は全て ITfEditSession::DoEditSession コールバック内で行う。
//! TF_ES_SYNC | TF_ES_READWRITE で同期実行（OnKeyDown 内なので同期が保証される）。

use std::cell::RefCell;
use std::io::Write as _;
use std::mem::ManuallyDrop;
use std::rc::Rc;

use windows::core::{implement, Interface as _};
use windows::Win32::UI::TextServices::{
    ITfComposition, ITfCompositionSink, ITfContext, ITfContextComposition, ITfEditSession,
    ITfEditSession_Impl, ITfInsertAtSelection, ITfRange, INSERT_TEXT_AT_SELECTION_FLAGS,
    TF_ANCHOR_END, TF_AE_NONE, TF_SELECTION, TF_SELECTIONSTYLE,
};
use windows_core::BOOL;

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

/// 文字列を UTF-16 に変換するヘルパー
fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

/// range を末尾に collapse して selection をセットする（カーソル移動）
unsafe fn set_cursor_to_end_of_range(
    ec: u32,
    context: &ITfContext,
    range: ITfRange,
) -> windows::core::Result<()> {
    range.Collapse(ec, TF_ANCHOR_END)?;
    let selection = TF_SELECTION {
        range: ManuallyDrop::new(Some(range)),
        style: TF_SELECTIONSTYLE {
            ase: TF_AE_NONE,
            fInterimChar: BOOL(0),
        },
    };
    context.SetSelection(ec, &[selection])?;
    Ok(())
}

// =========================================================
// CommitEditSession: 確定テキストを直接挿入（コンポジションなし）
// =========================================================

#[implement(ITfEditSession)]
pub struct CommitEditSession {
    context: ITfContext,
    text: String,
}

impl CommitEditSession {
    pub fn new(context: ITfContext, text: String) -> Self {
        Self { context, text }
    }
}

impl ITfEditSession_Impl for CommitEditSession_Impl {
    fn DoEditSession(&self, ec: u32) -> windows::core::Result<()> {
        dbglog!("CommitEditSession::DoEditSession ec={}", ec);
        unsafe {
            let insert: ITfInsertAtSelection = self.context.cast()?;
            let wide = to_wide(&self.text);
            let range = insert.InsertTextAtSelection(ec, INSERT_TEXT_AT_SELECTION_FLAGS(0), &wide)?;
            set_cursor_to_end_of_range(ec, &self.context, range)?;
            dbglog!("CommitEditSession: done");
        }
        Ok(())
    }
}

// =========================================================
// CommitWithCompositionEditSession: コンポジション範囲を確定テキストに置換して終了
// =========================================================

#[implement(ITfEditSession)]
pub struct CommitWithCompositionEditSession {
    context: ITfContext,
    composition: ITfComposition,
    text: String,
}

impl CommitWithCompositionEditSession {
    pub fn new(context: ITfContext, composition: ITfComposition, text: String) -> Self {
        Self {
            context,
            composition,
            text,
        }
    }
}

impl ITfEditSession_Impl for CommitWithCompositionEditSession_Impl {
    fn DoEditSession(&self, ec: u32) -> windows::core::Result<()> {
        dbglog!("CommitWithCompositionEditSession::DoEditSession ec={}", ec);
        unsafe {
            let range: ITfRange = self.composition.GetRange()?;
            let wide = to_wide(&self.text);
            range.SetText(ec, 0, &wide)?;

            // カーソルを確定テキストの末尾に移動
            range.Collapse(ec, TF_ANCHOR_END)?;
            let selection = TF_SELECTION {
                range: ManuallyDrop::new(Some(range)),
                style: TF_SELECTIONSTYLE {
                    ase: TF_AE_NONE,
                    fInterimChar: BOOL(0),
                },
            };
            self.context.SetSelection(ec, &[selection])?;

            self.composition.EndComposition(ec)?;
            dbglog!("CommitWithCompositionEditSession: done");
        }
        Ok(())
    }
}

// =========================================================
// CompositionEditSession: コンポジションを開始または更新する
// =========================================================

#[implement(ITfEditSession)]
pub struct CompositionEditSession {
    context: ITfContext,
    text: String,
    existing_composition: Option<ITfComposition>,
    sink: ITfCompositionSink,
    /// 同期実行のため、結果の ITfComposition をここに格納する
    result: Rc<RefCell<Option<ITfComposition>>>,
}

impl CompositionEditSession {
    pub fn new(
        context: ITfContext,
        text: String,
        existing_composition: Option<ITfComposition>,
        sink: ITfCompositionSink,
        result: Rc<RefCell<Option<ITfComposition>>>,
    ) -> Self {
        Self {
            context,
            text,
            existing_composition,
            sink,
            result,
        }
    }
}

impl ITfEditSession_Impl for CompositionEditSession_Impl {
    fn DoEditSession(&self, ec: u32) -> windows::core::Result<()> {
        dbglog!("CompositionEditSession::DoEditSession ec={} existing={}", ec, self.existing_composition.is_some());
        let wide = to_wide(&self.text);

        unsafe {
            if let Some(ref comp) = self.existing_composition {
                // 既存コンポジションのテキストを更新
                let range: ITfRange = comp.GetRange()?;
                range.SetText(ec, 0, &wide)?;
                *self.result.borrow_mut() = Some(comp.clone());
                dbglog!("CompositionEditSession: updated existing");
            } else {
                // 新規コンポジション開始
                let insert: ITfInsertAtSelection = self.context.cast()?;
                let range: ITfRange =
                    insert.InsertTextAtSelection(ec, INSERT_TEXT_AT_SELECTION_FLAGS(0), &wide)?;
                dbglog!("CompositionEditSession: inserted text");

                let ctx_comp: ITfContextComposition = self.context.cast()?;
                let composition = ctx_comp.StartComposition(ec, &range, &self.sink)?;
                *self.result.borrow_mut() = Some(composition);
                dbglog!("CompositionEditSession: started composition");
            }
        }
        Ok(())
    }
}

// =========================================================
// EndCompositionEditSession: コンポジションを終了する（キャンセル時）
// =========================================================

#[implement(ITfEditSession)]
pub struct EndCompositionEditSession {
    composition: ITfComposition,
}

impl EndCompositionEditSession {
    pub fn new(composition: ITfComposition) -> Self {
        Self { composition }
    }
}

impl ITfEditSession_Impl for EndCompositionEditSession_Impl {
    fn DoEditSession(&self, ec: u32) -> windows::core::Result<()> {
        dbglog!("EndCompositionEditSession::DoEditSession ec={}", ec);
        unsafe {
            // コンポジション範囲を空にしてから終了
            let range: ITfRange = self.composition.GetRange()?;
            range.SetText(ec, 0, &[])?;
            self.composition.EndComposition(ec)?;
            dbglog!("EndCompositionEditSession: done");
        }
        Ok(())
    }
}
