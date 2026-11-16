use core::{fmt, mem, num::NonZero, slice};

use alloc::{borrow::Cow, fmt::format, vec::Vec};
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
}

impl OsErrorInner {
    pub fn new(code: Option<NonZero<i32>>, args: fmt::Arguments<'_>) -> Self {
        let expr = args
            .as_str()
            .map_or_else(|| Cow::Owned(format(args)), Cow::Borrowed);

        Self::Single(OsErrorSource { code, expr })
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
        let expr = &*self.expr;
        match self.code {
            Some(code) => f.write_fmt(format_args!("in {expr} (error code {code})")),
            None => f.write_fmt(format_args!("in {expr} (error code ?)")),
        }
    }
}
