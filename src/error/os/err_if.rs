
pub struct Specialize<T>(pub T);

pub trait ViaPointer {
    #[allow(unused)]
    fn as_debug(&self) -> impl fmt::Debug;
}

pub trait ViaDebug {
    #[allow(unused)]
    fn as_debug(&self) -> impl fmt::Debug;
}

pub trait ViaType {
    #[allow(unused)]
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
            let code = last_os_error();
            cfg_select! {
                debug_assertions => Err($crate::error::OsError::new(
                    code,
                    ::core::format_args!(
                        "{}({:?})",
                        ::core::stringify!($path0$(::$path)*),
                        ($((&&&Specialize($params)).as_debug(),)*)
                    ),
                    file!(),
                    line!(),
                    column!(),
                )),
                _ => Err($crate::error::OsError::new(
                    code,
                    ::core::stringify!($path0$(::$path)*),
                )),
            }
        } else {
            ::core::result::Result::Ok(res)
        }
    }};
}

use core::{any::type_name, fmt};

pub(crate) use os_err_if;
