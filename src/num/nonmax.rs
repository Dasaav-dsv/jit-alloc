use core::{fmt, hash, num::NonZero};

use crate::num::Unsigned;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct NonMax<T: HasZero>(T::NonZero);

pub trait HasZero: Unsigned + From<Self::NonZero> {
    const NZ_ONE: Self::NonZero;
    const NZ_MAX: Self::NonZero;

    type NonZero: Clone
        + Copy
        + PartialEq
        + Eq
        + PartialOrd
        + Ord
        + hash::Hash
        + TryFrom<Self>
        + TryFrom<NonZero<usize>>;
}

impl<T: HasZero> NonMax<T> {
    #[allow(unused)]
    pub const ZERO: Self = Self(<T as HasZero>::NZ_ONE);

    #[allow(unused)]
    pub const MAX: Self = Self(<T as HasZero>::NZ_MAX);

    pub fn new(value: T) -> Option<Self> {
        let nonzero = T::NonZero::try_from(value.wrapping_add(T::ONE)).ok()?;
        Some(Self(nonzero))
    }

    pub fn get(self) -> T {
        T::from(self.0) - T::ONE
    }
}

impl<T: HasZero> Default for NonMax<T> {
    fn default() -> Self {
        Self::ZERO
    }
}

impl<T: HasZero> TryFrom<usize> for NonMax<T> {
    type Error = <T::NonZero as TryFrom<T>>::Error;

    fn try_from(value: usize) -> Result<Self, Self::Error> {
        match T::try_from(value.wrapping_add(1)) {
            Ok(value) => T::NonZero::try_from(value).map(Self),
            Err(_) => T::NonZero::try_from(T::ZERO).map(Self),
        }
    }
}

impl<T: HasZero> fmt::Debug for NonMax<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.get(), f)
    }
}

impl<T: HasZero> fmt::Display for NonMax<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.get(), f)
    }
}

macro_rules! impl_nonmax {
    ($($t:ty),+ $(,)?) => {
        $(
            impl HasZero for $t {
                const NZ_ONE: Self::NonZero = NonZero::<$t>::MIN;
                const NZ_MAX: Self::NonZero = NonZero::<$t>::MAX;
                type NonZero = NonZero<$t>;
            }
        )+
    };
}

impl_nonmax! { u8, u16, u32, u64, u128, usize }
