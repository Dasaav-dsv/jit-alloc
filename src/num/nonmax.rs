use core::{fmt, num::NonZero};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct NonMax<T: HasZero>(T::NonZero);

pub trait HasZero {
    type NonZero;
}

impl HasZero for u32 {
    type NonZero = NonZero<u32>;
}

impl NonMax<u32> {
    #[allow(unused)]
    pub const MIN: Self = Self::new(0).unwrap();

    pub const fn new(value: u32) -> Option<Self> {
        match NonZero::new(value.wrapping_add(1)) {
            Some(nonzero) => Some(Self(nonzero)),
            None => None,
        }
    }

    pub const fn get(self) -> u32 {
        self.0.get().wrapping_sub(1)
    }

    pub const fn as_usize(self) -> usize {
        self.get() as usize
    }
}

impl Default for NonMax<u32> {
    fn default() -> Self {
        const { Self::new(0).unwrap() }
    }
}

impl TryFrom<u32> for NonMax<u32> {
    type Error = core::num::TryFromIntError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        value.wrapping_add(1).try_into().map(Self)
    }
}

impl TryFrom<usize> for NonMax<u32> {
    type Error = core::num::TryFromIntError;

    fn try_from(value: usize) -> Result<Self, Self::Error> {
        <u32>::try_from(value)?.wrapping_add(1).try_into().map(Self)
    }
}

impl fmt::Debug for NonMax<u32> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.get(), f)
    }
}

impl fmt::Display for NonMax<u32> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.get(), f)
    }
}
