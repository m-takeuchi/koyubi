mod class_factory;
mod edit_session;
mod globals;
mod key_event;
mod register;
mod text_service;

use std::ffi::c_void;
use std::panic;

use windows::Win32::Foundation::{HMODULE, S_FALSE, S_OK, E_FAIL, CLASS_E_CLASSNOTAVAILABLE};
use windows::Win32::System::LibraryLoader::DisableThreadLibraryCalls;
use windows::Win32::System::SystemServices::DLL_PROCESS_ATTACH;
use windows::core::{GUID, HRESULT, Interface as _};
use windows_core::BOOL;

use class_factory::ClassFactory;
use globals::CLSID_KOYUBI_TEXT_SERVICE;

#[no_mangle]
extern "system" fn DllMain(hinst: HMODULE, reason: u32, _reserved: *mut c_void) -> BOOL {
    if reason == DLL_PROCESS_ATTACH {
        globals::set_dll_instance(hinst);
        unsafe {
            let _ = DisableThreadLibraryCalls(hinst);
        }
    }
    BOOL(1)
}

#[no_mangle]
extern "system" fn DllGetClassObject(
    rclsid: *const GUID,
    riid: *const GUID,
    ppv: *mut *mut c_void,
) -> HRESULT {
    let result = panic::catch_unwind(|| unsafe {
        if ppv.is_null() {
            return E_FAIL;
        }
        *ppv = std::ptr::null_mut();

        if rclsid.is_null() || riid.is_null() {
            return E_FAIL;
        }

        if *rclsid != CLSID_KOYUBI_TEXT_SERVICE {
            return CLASS_E_CLASSNOTAVAILABLE;
        }

        let factory = ClassFactory::new();
        let unknown: windows::core::IUnknown = factory.into();
        unknown.query(&*riid, ppv)
    });

    match result {
        Ok(hr) => hr,
        Err(_) => E_FAIL,
    }
}

#[no_mangle]
extern "system" fn DllCanUnloadNow() -> HRESULT {
    if globals::ref_count() == 0 {
        S_OK
    } else {
        S_FALSE
    }
}

#[no_mangle]
extern "system" fn DllRegisterServer() -> HRESULT {
    let result = panic::catch_unwind(|| match register::register() {
        Ok(()) => S_OK,
        Err(e) => e.code(),
    });

    match result {
        Ok(hr) => hr,
        Err(_) => E_FAIL,
    }
}

#[no_mangle]
extern "system" fn DllUnregisterServer() -> HRESULT {
    let result = panic::catch_unwind(|| match register::unregister() {
        Ok(()) => S_OK,
        Err(e) => e.code(),
    });

    match result {
        Ok(hr) => hr,
        Err(_) => E_FAIL,
    }
}
