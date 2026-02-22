use windows::core::{Result, PCWSTR};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CLASSES_ROOT, REG_SZ,
    RegCloseKey, RegCreateKeyW, RegDeleteKeyW, RegSetValueExW,
};
use windows::Win32::UI::TextServices::{
    CLSID_TF_CategoryMgr, CLSID_TF_InputProcessorProfiles, GUID_TFCAT_TIP_KEYBOARD,
    ITfCategoryMgr, ITfInputProcessorProfiles,
};

use crate::globals::{
    self, CLSID_KOYUBI_TEXT_SERVICE, GUID_KOYUBI_PROFILE, LANGID_JA,
};

use std::io::Write as _;

const DISPLAY_NAME: &str = "Koyubi SKK";

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

/// DLL のファイルパスを取得する
fn dll_path() -> Result<Vec<u16>> {
    let mut buf = vec![0u16; 260];
    let len = unsafe {
        windows::Win32::System::LibraryLoader::GetModuleFileNameW(
            Some(globals::dll_instance()),
            &mut buf,
        )
    };
    if len == 0 {
        return Err(windows::core::Error::from_win32());
    }
    buf.truncate(len as usize);
    Ok(buf)
}

/// TSF Text Service の登録（DllRegisterServer から呼ばれる）
pub fn register() -> Result<()> {
    dbglog!("register: start");
    let dll_path = dll_path()?;
    dbglog!("register: dll_path ok");

    // COM InprocServer32 レジストリ登録
    register_com_server(&dll_path)?;
    dbglog!("register: com_server ok");

    // ITfInputProcessorProfiles による登録
    let profiles: ITfInputProcessorProfiles = unsafe {
        CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER)?
    };
    dbglog!("register: profiles created");

    unsafe {
        profiles.Register(&CLSID_KOYUBI_TEXT_SERVICE)?;
    }
    dbglog!("register: Register ok");

    // null 終端付き（Windows API が null 終端を期待する）
    let desc: Vec<u16> = DISPLAY_NAME.encode_utf16().chain(std::iter::once(0)).collect();
    let icon_path: Vec<u16> = dll_path.clone();

    let result = unsafe {
        profiles.AddLanguageProfile(
            &CLSID_KOYUBI_TEXT_SERVICE,
            LANGID_JA,
            &GUID_KOYUBI_PROFILE,
            &desc,
            &icon_path,
            0,
        )
    };
    match &result {
        Ok(()) => dbglog!("register: AddLanguageProfile ok"),
        Err(e) => dbglog!("register: AddLanguageProfile failed: {:?}", e),
    }
    result?;

    // ITfCategoryMgr でキーボードカテゴリに登録
    let category_mgr: ITfCategoryMgr = unsafe {
        CoCreateInstance(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER)?
    };
    dbglog!("register: category_mgr created");

    unsafe {
        category_mgr.RegisterCategory(
            &CLSID_KOYUBI_TEXT_SERVICE,
            &GUID_TFCAT_TIP_KEYBOARD,
            &CLSID_KOYUBI_TEXT_SERVICE,
        )?;
    }
    dbglog!("register: RegisterCategory ok");

    Ok(())
}

/// TSF Text Service の登録解除（DllUnregisterServer から呼ばれる）
pub fn unregister() -> Result<()> {
    // ITfCategoryMgr からカテゴリ削除
    if let Ok(category_mgr) = unsafe {
        CoCreateInstance::<_, ITfCategoryMgr>(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER)
    } {
        let _ = unsafe {
            category_mgr.UnregisterCategory(
                &CLSID_KOYUBI_TEXT_SERVICE,
                &GUID_TFCAT_TIP_KEYBOARD,
                &CLSID_KOYUBI_TEXT_SERVICE,
            )
        };
    }

    // ITfInputProcessorProfiles から登録解除
    if let Ok(profiles) = unsafe {
        CoCreateInstance::<_, ITfInputProcessorProfiles>(
            &CLSID_TF_InputProcessorProfiles,
            None,
            CLSCTX_INPROC_SERVER,
        )
    } {
        let _ = unsafe { profiles.Unregister(&CLSID_KOYUBI_TEXT_SERVICE) };
    }

    // COM InprocServer32 レジストリ削除
    let _ = unregister_com_server();

    Ok(())
}

/// GUID を "{XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}" 形式の文字列に変換
fn guid_to_string(guid: &windows::core::GUID) -> String {
    format!(
        "{{{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
        guid.data1,
        guid.data2,
        guid.data3,
        guid.data4[0],
        guid.data4[1],
        guid.data4[2],
        guid.data4[3],
        guid.data4[4],
        guid.data4[5],
        guid.data4[6],
        guid.data4[7],
    )
}

/// Null終端付き UTF-16 文字列を作成
fn to_wide_null(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// HKCR\CLSID\{CLSID}\InprocServer32 にDLLパスとスレッドモデルを書く
fn register_com_server(dll_path: &[u16]) -> Result<()> {
    let clsid_str = guid_to_string(&CLSID_KOYUBI_TEXT_SERVICE);

    unsafe {
        // CLSID キーを作成
        let clsid_key_path = to_wide_null(&format!("CLSID\\{clsid_str}"));
        let mut hkey = HKEY::default();
        let status = RegCreateKeyW(
            HKEY_CLASSES_ROOT,
            PCWSTR(clsid_key_path.as_ptr()),
            &mut hkey,
        );
        if status.is_err() {
            return Err(windows::core::Error::from_hresult(
                windows::core::HRESULT(status.0 as i32),
            ));
        }
        let _ = RegCloseKey(hkey);

        // InprocServer32 サブキーを作成
        let inproc_key_path = to_wide_null(&format!("CLSID\\{clsid_str}\\InprocServer32"));
        let status = RegCreateKeyW(
            HKEY_CLASSES_ROOT,
            PCWSTR(inproc_key_path.as_ptr()),
            &mut hkey,
        );
        if status.is_err() {
            return Err(windows::core::Error::from_hresult(
                windows::core::HRESULT(status.0 as i32),
            ));
        }

        // デフォルト値に DLL パスを設定
        // dll_path は null 終端なしなので、終端を追加
        let mut dll_path_null: Vec<u16> = dll_path.to_vec();
        dll_path_null.push(0);
        let status = RegSetValueExW(
            hkey,
            PCWSTR::null(),
            None,
            REG_SZ,
            Some(std::slice::from_raw_parts(
                dll_path_null.as_ptr() as *const u8,
                dll_path_null.len() * 2,
            )),
        );
        if status.is_err() {
            let _ = RegCloseKey(hkey);
            return Err(windows::core::Error::from_hresult(
                windows::core::HRESULT(status.0 as i32),
            ));
        }

        // ThreadingModel = "Apartment"
        let threading_model = to_wide_null("Apartment");
        let value_name = to_wide_null("ThreadingModel");
        let status = RegSetValueExW(
            hkey,
            PCWSTR(value_name.as_ptr()),
            None,
            REG_SZ,
            Some(std::slice::from_raw_parts(
                threading_model.as_ptr() as *const u8,
                threading_model.len() * 2,
            )),
        );
        let _ = RegCloseKey(hkey);
        if status.is_err() {
            return Err(windows::core::Error::from_hresult(
                windows::core::HRESULT(status.0 as i32),
            ));
        }
    }

    Ok(())
}

/// HKCR\CLSID\{CLSID} を削除
fn unregister_com_server() -> Result<()> {
    let clsid_str = guid_to_string(&CLSID_KOYUBI_TEXT_SERVICE);

    unsafe {
        // まず InprocServer32 サブキーを削除
        let subkey = to_wide_null(&format!("CLSID\\{clsid_str}\\InprocServer32"));
        let _ = RegDeleteKeyW(HKEY_CLASSES_ROOT, PCWSTR(subkey.as_ptr()));

        // 親キーを削除
        let key = to_wide_null(&format!("CLSID\\{clsid_str}"));
        let _ = RegDeleteKeyW(HKEY_CLASSES_ROOT, PCWSTR(key.as_ptr()));
    }

    Ok(())
}
