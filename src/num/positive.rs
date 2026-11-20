use core::{fmt, ops};

use crate::num::Unsigned;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Positive<T: Unsigned>(T);

impl<T: Unsigned> Positive<T> {
    #[allow(unused)]
    pub const MIN: Self = Self(T::MIN);

    #[allow(unused)]
    pub const MAX: Self = Self(T::MAX);

    pub fn new(value: T::Wide) -> Option<Self> {
        value.checked_sub(T::Wide::ONE)?.try_into().ok().map(Self)
    }

    pub fn checked_add(self, rhs: Self) -> Option<Self> {
        self.0.checked_add(rhs.0)?.checked_add(T::ONE).map(Self)
    }

    pub fn checked_sub(self, rhs: Self) -> Option<Self> {
        self.0.checked_sub(rhs.0)?.checked_sub(T::ONE).map(Self)
    }

    pub fn get(self) -> Option<T> {
        self.0.checked_add(T::ONE)
    }

    pub fn as_wide(self) -> T::Wide {
        self.0.as_wide() + T::Wide::ONE
    }
}

impl<T: Unsigned> ops::Add for Positive<T> {
    type Output = Positive<T>;

    #[track_caller]
    fn add(self, rhs: Self) -> Self::Output {
        Self(self.0.strict_add(rhs.0).strict_add(T::ONE))
    }
}

impl<T: Unsigned> ops::Add<T> for Positive<T> {
    type Output = Positive<T>;

    #[track_caller]
    fn add(self, rhs: T) -> Self::Output {
        Self(self.0.strict_add(rhs))
    }
}

impl<T: Unsigned> ops::Sub for Positive<T> {
    type Output = T;

    #[track_caller]
    fn sub(self, rhs: Self) -> Self::Output {
        self.0.strict_sub(rhs.0)
    }
}

impl<T: Unsigned> ops::Sub<T> for Positive<T> {
    type Output = Positive<T>;

    #[track_caller]
    fn sub(self, rhs: T) -> Self::Output {
        Self(self.0.strict_sub(rhs))
    }
}

impl<T: Unsigned> TryFrom<usize> for Positive<T> {
    type Error = <T as TryFrom<usize>>::Error;

    fn try_from(value: usize) -> Result<Self, Self::Error> {
        <T>::try_from(value.wrapping_sub(1)).map(Self)
    }
}

impl<T: Unsigned> fmt::Debug for Positive<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.get(), f)
    }
}

impl<T: Unsigned> fmt::Display for Positive<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.as_wide(), f)
    }
}
