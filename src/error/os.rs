use core::{fmt, mem, num::NonZero, slice};

use alloc::{borrow::Cow, string::ToString, vec::Vec};
use thiserror::Error;

pub(crate) mod err_if;

#[derive(Error, Debug)]
pub enum OsErrorInner {
    Single(#[from] OsErrorSource),
    Multiple(Vec<OsErrorSource>),
}

#[derive(Clone, Error, Debug)]
pub struct OsErrorSource {
    code: Option<NonZero<i32>>,
    expr: Cow<'static, str>,

    #[cfg(debug_assertions)]
    file: &'static str,
    #[cfg(debug_assertions)]
    line: u32,
    #[cfg(debug_assertions)]
    column: u32,
}

impl OsErrorInner {
    #[cfg(not(debug_assertions))]
    pub fn new(code: Option<NonZero<i32>>, expr: &'static str) -> Self {
        Self::Single(OsErrorSource {
            code,
            expr: Cow::Borrowed(expr),
        })
    }

    #[cfg(debug_assertions)]
    pub fn new(
        code: Option<NonZero<i32>>,
        args: fmt::Arguments<'_>,
        file: &'static str,
        line: u32,
        column: u32,
    ) -> Self {
        let expr = args
            .as_str()
            .map_or_else(|| Cow::Owned(alloc::fmt::format(args)), Cow::Borrowed);

        Self::Single(OsErrorSource {
            code,
            expr,
            file,
            line,
            column,
        })
    }

    pub fn chain(&mut self, other: Self) {
        self.as_sources_vec_mut()
            .clone_from_slice(other.as_sources());
    }

    fn as_sources(&self) -> &[OsErrorSource] {
        match self {
            Self::Single(inner) => slice::from_ref(inner),
            Self::Multiple(vec_inner) => vec_inner,
        }
    }

    fn as_sources_vec_mut(&mut self) -> &mut Vec<OsErrorSource> {
        if let Self::Multiple(sources) = self {
            return sources;
        }

        let taken = mem::replace(self, OsErrorInner::Multiple(Vec::new()));
        let (Self::Multiple(sources), Self::Single(source)) = (self, taken) else {
            unreachable!();
        };

        sources.push(source);

        sources
    }
}

impl fmt::Display for OsErrorInner {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> fmt::Result {
        match self.as_sources() {
            [] => f.write_str("OS error: no source available"),
            [source] => f.write_fmt(format_args!("OS error: {source}")),
            [first, many @ ..] => {
                f.write_str("OS error (most recent first): ")?;
                for source in many.iter().rev() {
                    f.write_fmt(format_args!("{source}; "))?
                }
                f.write_fmt(format_args!("{first}"))
            }
        }
    }
}

impl fmt::Display for OsErrorSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let code = match self.code {
            Some(code) => Cow::Owned(code.to_string()),
            None => Cow::Borrowed("?"),
        };

        #[cfg(not(debug_assertions))]
        f.write_fmt(format_args!(
            "in {expr} (error code {code})",
            expr = self.expr,
        ))?;

        #[cfg(debug_assertions)]
        f.write_fmt(format_args!(
            "in {expr} (error code {code}) at {}:{}:{}",
            self.file,
            self.line,
            self.column,
            expr = self.expr,
        ))?;

        Ok(())
    }
}
