use core::{
    fmt,
    num::{self, NonZero},
    ops,
};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Positive<T>(T);

impl Positive<u16> {
    #[allow(unused)]
    pub const MIN: Self = Self(0);

    #[allow(unused)]
    pub const MAX: Self = Self(u16::MAX);

    pub const fn new(value: u32) -> Option<Self> {
        match value.checked_sub(1) {
            Some(value @ 0..65536) => Some(Self(value as u16)),
            _ => None,
        }
    }

    pub const fn checked_add(self, rhs: Self) -> Option<Self> {
        match self.0.checked_add(rhs.0) {
            Some(value) => match value.checked_add(1) {
                Some(value) => Some(Self(value)),
                None => None,
            },
            None => None,
        }
    }

    pub const fn checked_sub(self, rhs: Self) -> Option<Self> {
        match self.0.checked_sub(rhs.0) {
            Some(value) => match value.checked_sub(1) {
                Some(value) => Some(Self(value)),
                None => None,
            },
            None => None,
        }
    }

    pub const fn get(self) -> u32 {
        self.0 as u32 + 1
    }
}

impl ops::Add for Positive<u16> {
    type Output = Positive<u16>;

    fn add(self, rhs: Self) -> Self::Output {
        Self(self.0.strict_add(rhs.0).strict_add(1))
    }
}

impl ops::Add<u16> for Positive<u16> {
    type Output = Positive<u16>;

    fn add(self, rhs: u16) -> Self::Output {
        Self(self.0.strict_add(rhs))
    }
}

impl ops::Sub for Positive<u16> {
    type Output = u16;

    fn sub(self, rhs: Self) -> Self::Output {
        self.0.strict_sub(rhs.0)
    }
}

impl ops::Sub<u16> for Positive<u16> {
    type Output = Positive<u16>;

    fn sub(self, rhs: u16) -> Self::Output {
        Self(self.0.strict_sub(rhs))
    }
}

impl TryFrom<u16> for Positive<u16> {
    type Error = num::TryFromIntError;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        NonZero::try_from(value).map(|value| Self(value.get() - 1))
    }
}

impl TryFrom<Positive<u16>> for u16 {
    type Error = num::TryFromIntError;

    fn try_from(value: Positive<u16>) -> Result<Self, Self::Error> {
        u16::try_from(value.get())
    }
}

impl TryFrom<usize> for Positive<u16> {
    type Error = num::TryFromIntError;

    fn try_from(value: usize) -> Result<Self, Self::Error> {
        u16::try_from(value.wrapping_sub(1)).map(Self)
    }
}

impl fmt::Debug for Positive<u16> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.get(), f)
    }
}

impl fmt::Display for Positive<u16> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.get(), f)
    }
}
