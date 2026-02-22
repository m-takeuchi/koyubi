//! Windows 仮想キーコード → エンジン KeyEvent 変換
//!
//! TSF の ITfKeyEventSink が受け取る WPARAM/LPARAM から
//! koyubi-engine の KeyEvent に変換する。

use koyubi_engine::{CompositionState, InputMode, Key, KeyEvent};
use koyubi_engine::composer::SkkEngine;

use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyboardState, ToUnicode, VIRTUAL_KEY,
    VK_BACK, VK_CONTROL, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE, VK_HOME, VK_LEFT, VK_MENU,
    VK_RETURN, VK_RIGHT, VK_SHIFT, VK_SPACE, VK_TAB, VK_UP,
};

/// キーボードの修飾キー状態で指定キーが押されているか
fn is_key_down(key_state: &[u8; 256], vk: VIRTUAL_KEY) -> bool {
    key_state[vk.0 as usize] & 0x80 != 0
}

/// WPARAM からスキャンコードを取得（LPARAM のビット 16-23）
fn scan_code(lparam: LPARAM) -> u32 {
    ((lparam.0 as u32) >> 16) & 0xFF
}

/// エンジンにバックスペースで処理する対象があるか判定する。
///
/// Direct + ローマ字ペンディングなし → 処理対象なし（アプリに渡す）
/// PreComposition / Registration → 処理対象あり
/// Conversion → 処理対象なし（アプリに渡す）
fn has_backspace_target(engine: &SkkEngine) -> bool {
    match engine.composition_state() {
        CompositionState::Direct => engine.has_pending_romaji(),
        CompositionState::PreComposition { .. } | CompositionState::Registration { .. } => true,
        CompositionState::Conversion { .. } => false,
    }
}

/// Emacs キーバインドのアクション
pub enum EmacsAction {
    /// 単一キーの SendInput シミュレーション
    SimulateKey(VIRTUAL_KEY),
    /// 行末まで削除（Shift+End → Delete）
    KillLine,
}

/// VK コードから Emacs アクションを返す。
///
/// Ctrl+H は除外（エンジン経由で処理するため）。
pub fn emacs_action(vk: u16) -> Option<EmacsAction> {
    match vk {
        0x46 => Some(EmacsAction::SimulateKey(VK_RIGHT)), // Ctrl+F → Right
        0x42 => Some(EmacsAction::SimulateKey(VK_LEFT)),  // Ctrl+B → Left
        0x41 => Some(EmacsAction::SimulateKey(VK_HOME)),  // Ctrl+A → Home
        0x45 => Some(EmacsAction::SimulateKey(VK_END)),   // Ctrl+E → End
        0x4E => Some(EmacsAction::SimulateKey(VK_DOWN)),  // Ctrl+N → Down
        0x50 => Some(EmacsAction::SimulateKey(VK_UP)),    // Ctrl+P → Up
        0x44 => Some(EmacsAction::SimulateKey(VK_DELETE)), // Ctrl+D → Delete
        0x4B => Some(EmacsAction::KillLine),              // Ctrl+K → Kill line
        _ => None,
    }
}

/// Windows VK コードからエンジンの KeyEvent に変換する。
///
/// 修飾キー単体（Shift, Ctrl, Alt）の場合は None を返す。
/// Ctrl+H は Key::Backspace に変換する（Emacs キーバインド）。
pub fn to_key_event(wparam: WPARAM, lparam: LPARAM) -> Option<KeyEvent> {
    let vk = wparam.0 as u16;

    // 修飾キー単体は無視
    if vk == VK_SHIFT.0 || vk == VK_CONTROL.0 || vk == VK_MENU.0 {
        return None;
    }

    let mut kbd_state = [0u8; 256];
    unsafe {
        if GetKeyboardState(&mut kbd_state).is_err() {
            kbd_state.fill(0);
        }
    }

    let shift = is_key_down(&kbd_state, VK_SHIFT);
    let ctrl = is_key_down(&kbd_state, VK_CONTROL);
    let alt = is_key_down(&kbd_state, VK_MENU);

    // Ctrl+H → Backspace に変換（ctrl フラグは落とす）
    if ctrl && vk == 0x48 {
        return Some(KeyEvent {
            key: Key::Backspace,
            shift,
            ctrl: false,
            alt,
        });
    }

    // 非文字キーの直接マッピング
    let key = match vk {
        v if v == VK_RETURN.0 => Key::Enter,
        v if v == VK_SPACE.0 => Key::Space,
        v if v == VK_BACK.0 => Key::Backspace,
        v if v == VK_ESCAPE.0 => Key::Escape,
        v if v == VK_TAB.0 => Key::Tab,
        _ => {
            // 文字キー: ToUnicode で文字を取得
            // Ctrl 押下時は kbd_state から Ctrl をクリアして、
            // Ctrl+J が '\n' ではなく 'j' + ctrl: true になるようにする
            let mut state_for_tounicode = kbd_state;
            if ctrl {
                state_for_tounicode[VK_CONTROL.0 as usize] = 0;
                // 左右 Ctrl もクリア
                state_for_tounicode[0xA2] = 0; // VK_LCONTROL
                state_for_tounicode[0xA3] = 0; // VK_RCONTROL
            }

            let sc = scan_code(lparam);
            let mut buf = [0u16; 4];
            let result = unsafe {
                ToUnicode(vk as u32, sc, Some(&state_for_tounicode), &mut buf, 0)
            };

            if result == 1 {
                if let Some(ch) = char::from_u32(buf[0] as u32) {
                    Key::Char(ch)
                } else {
                    return None;
                }
            } else {
                return None;
            }
        }
    };

    Some(KeyEvent {
        key,
        shift,
        ctrl,
        alt,
    })
}

/// キーを消費するかどうかの事前判定（OnTestKeyDown 用）。
///
/// エンジンの process_key() を呼ばずに、キーを食べるか判定する。
pub fn should_eat_key(wparam: WPARAM, _lparam: LPARAM, engine: &SkkEngine) -> bool {
    let vk = wparam.0 as u16;

    // 修飾キー単体は食べない
    if vk == VK_SHIFT.0 || vk == VK_CONTROL.0 || vk == VK_MENU.0 {
        return false;
    }

    let mut kbd_state = [0u8; 256];
    unsafe {
        if GetKeyboardState(&mut kbd_state).is_err() {
            kbd_state.fill(0);
        }
    }

    let ctrl = is_key_down(&kbd_state, VK_CONTROL);
    let shift = is_key_down(&kbd_state, VK_SHIFT);

    // IME トグルキーは常に消費
    if ctrl && vk == VK_SPACE.0 {
        return true; // Ctrl-Space
    }
    if ctrl && vk == 0x4A {
        return true; // Ctrl-J (VK_J = 0x4A)
    }
    if ctrl && vk == 0xBA {
        return true; // Ctrl-; (VK_OEM_1 = 0xBA)
    }

    // Emacs キーバインド: 全モードで消費
    if ctrl && !shift {
        if vk == 0x48 {
            return true; // Ctrl+H
        }
        if emacs_action(vk).is_some() {
            return true;
        }
    }

    match engine.current_mode() {
        InputMode::Ascii => false,
        InputMode::Hiragana | InputMode::Katakana => {
            // Ctrl が押されている場合: 明示的に処理するキーのみ消費
            if ctrl {
                // Ctrl-G: コンポジション中のみ消費（キャンセル）
                if vk == 0x47 {
                    return !matches!(engine.composition_state(), CompositionState::Direct);
                }
                // それ以外の Ctrl 組み合わせはアプリに渡す
                return false;
            }
            // アルファベットキー (A-Z)
            if (0x41..=0x5A).contains(&vk) {
                return true;
            }
            // 数字キー (1-9) — ▼モードで候補選択に使う
            if (0x31..=0x39).contains(&vk) {
                if let CompositionState::Conversion { .. } = engine.composition_state() {
                    return true;
                }
            }
            // VK_BACK: 処理対象があるときのみ消費
            if vk == VK_BACK.0 {
                return has_backspace_target(engine);
            }
            // Space, Enter, Escape, Tab
            if vk == VK_SPACE.0
                || vk == VK_RETURN.0
                || vk == VK_ESCAPE.0
                || vk == VK_TAB.0
            {
                return true;
            }
            // OEM キー（記号）
            if (0xBA..=0xE4).contains(&vk) {
                return true;
            }
            false
        }
        InputMode::ZenkakuAscii => {
            // Ctrl が押されている場合
            if ctrl {
                return false;
            }
            // アルファベットキー (A-Z)
            if (0x41..=0x5A).contains(&vk) {
                return true;
            }
            // 数字キー (0-9)
            if (0x30..=0x39).contains(&vk) {
                return true;
            }
            // Space
            if vk == VK_SPACE.0 {
                return true;
            }
            // OEM キー（記号）
            if (0xBA..=0xE4).contains(&vk) {
                return true;
            }
            false
        }
        _ => false,
    }
}
