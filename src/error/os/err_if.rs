#![allow(unused)]

pub struct Specialize<T>(pub T);

pub trait ViaPointer {
    fn as_debug(&self) -> impl fmt::Debug;
}

pub trait ViaDebug {
    fn as_debug(&self) -> impl fmt::Debug;
}

pub trait ViaType {
    fn as_debug(&self) -> impl fmt::Debug;
}

impl<T> ViaPointer for &&Specialize<*const T> {
    fn as_debug(&self) -> impl fmt::Debug {
        if self.0.is_null() { "NULL" } else { "[ptr]" }
    }
}

impl<T> ViaPointer for &&Specialize<*mut T> {
    fn as_debug(&self) -> impl fmt::Debug {
        if self.0.is_null() { "NULL" } else { "[ptr]" }
    }
}

impl<T: fmt::Debug> ViaDebug for &Specialize<T> {
    fn as_debug(&self) -> impl fmt::Debug {
        &self.0
    }
}

impl<T> ViaType for Specialize<T> {
    fn as_debug(&self) -> impl fmt::Debug {
        type_name::<T>()
    }
}

macro_rules! os_err_if {
    ($path0:ident$(::$path:ident)* ($($params:expr),* $(,)?) $($cond:tt)+) => {{
        #[allow(unused)]
        use $crate::error::os::err_if::{Specialize, ViaPointer as _, ViaDebug as _, ViaType as _};
        let res = $path0$(::$path)*($($params,)*);
        if res $($cond)+ {
            let code = $crate::arch::last_os_error();
            Err($crate::error::OsError::new(
                code,
                ::core::format_args!(
                    "{}({:?})",
                    ::core::stringify!($fun),
                    ($((&&&Specialize($params)).as_debug(),)*)
                )
            ))
        } else {
            ::core::result::Result::Ok(res)
        }
    }};
}

use core::{any::type_name, fmt};

pub(crate) use os_err_if;
