use std::ffi::c_void;

use windows::Win32::System::Com::{IClassFactory, IClassFactory_Impl};
use windows::Win32::Foundation::CLASS_E_NOAGGREGATION;
use windows::core::{GUID, IUnknown, Interface as _, implement};
use windows_core::BOOL;

use crate::globals;
use crate::text_service::TextService;

/// COM クラスファクトリ — TextService オブジェクトを生成する
#[implement(IClassFactory)]
pub struct ClassFactory;

impl ClassFactory {
    pub fn new() -> Self {
        Self
    }
}

impl IClassFactory_Impl for ClassFactory_Impl {
    fn CreateInstance(
        &self,
        punkouter: windows::core::Ref<'_, IUnknown>,
        riid: *const GUID,
        ppvobject: *mut *mut c_void,
    ) -> windows::core::Result<()> {
        unsafe {
            if ppvobject.is_null() {
                return Err(windows::core::Error::from_hresult(
                    windows::Win32::Foundation::E_POINTER,
                ));
            }
            *ppvobject = std::ptr::null_mut();

            // COM aggregation は未サポート
            if punkouter.is_some() {
                return Err(windows::core::Error::from_hresult(CLASS_E_NOAGGREGATION));
            }

            let service: IUnknown = TextService::new().into();
            service.query(&*riid, ppvobject).ok()
        }
    }

    fn LockServer(&self, flock: BOOL) -> windows::core::Result<()> {
        if flock.as_bool() {
            globals::inc_ref_count();
        } else {
            globals::dec_ref_count();
        }
        Ok(())
    }
}
