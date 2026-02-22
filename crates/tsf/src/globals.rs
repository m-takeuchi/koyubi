use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};

use windows::Win32::Foundation::HMODULE;
use windows::core::GUID;

/// DLL インスタンスハンドル（HMODULE として保持）
static DLL_INSTANCE: AtomicIsize = AtomicIsize::new(0);

/// COM オブジェクト参照カウント（DllCanUnloadNow 判定用）
static DLL_REF_COUNT: AtomicU32 = AtomicU32::new(0);

/// CLSID for Koyubi Text Service
pub const CLSID_KOYUBI_TEXT_SERVICE: GUID =
    GUID::from_u128(0xa7b3c4d5_e6f7_4890_ab12_cd34ef567890);

/// Profile GUID for Koyubi's Japanese language profile
pub const GUID_KOYUBI_PROFILE: GUID =
    GUID::from_u128(0xb8c4d5e6_f708_4901_bc23_de45f0678901);

/// LangBar ボタン GUID
pub const GUID_LANGBAR_ITEM_BUTTON: GUID =
    GUID::from_u128(0xc9d5e6f7_0819_4a12_cd34_ef5678901234);

/// 日本語 LANGID (0x0411)
pub const LANGID_JA: u16 = 0x0411;

pub fn set_dll_instance(h: HMODULE) {
    DLL_INSTANCE.store(h.0 as isize, Ordering::Relaxed);
}

pub fn dll_instance() -> HMODULE {
    HMODULE(DLL_INSTANCE.load(Ordering::Relaxed) as *mut _)
}

pub fn inc_ref_count() {
    DLL_REF_COUNT.fetch_add(1, Ordering::Relaxed);
}

pub fn dec_ref_count() {
    DLL_REF_COUNT.fetch_sub(1, Ordering::Relaxed);
}

pub fn ref_count() -> u32 {
    DLL_REF_COUNT.load(Ordering::Relaxed)
}
