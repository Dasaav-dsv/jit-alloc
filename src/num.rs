use core::{fmt, hash::Hash, ops};

pub mod nonmax;
pub mod positive;

mod private {
    pub trait Sealed {}
}

pub trait Unsigned:
    Sized
    + Copy
    + Default
    + PartialEq
    + Eq
    + PartialOrd
    + Ord
    + Hash
    + ops::Add<Output = Self>
    + ops::Sub<Output = Self>
    + ops::Mul<Output = Self>
    + ops::Div<Output = Self>
    + TryFrom<u8>
    + TryFrom<u16>
    + TryFrom<u32>
    + TryFrom<u64>
    + TryFrom<u128>
    + TryFrom<usize>
    + TryFrom<i8>
    + TryFrom<i16>
    + TryFrom<i32>
    + TryFrom<i64>
    + TryFrom<i128>
    + TryFrom<isize>
    + TryFrom<Self::Wide>
    + fmt::Debug
    + fmt::Display
    + private::Sealed
{
    const ZERO: Self;
    const ONE: Self;
    const MAX: Self;
    const BITS: u32;

    type Wide: Unsigned;
    type Signed: ops::Neg<Output = Self::Signed>;

    fn wrapping_add(self, rhs: Self) -> Self;

    fn wrapping_sub(self, rhs: Self) -> Self;

    fn checked_add(self, rhs: Self) -> Option<Self>;

    fn checked_sub(self, rhs: Self) -> Option<Self>;

    fn strict_add(self, rhs: Self) -> Self;

    fn strict_sub(self, rhs: Self) -> Self;

    fn checked_add_signed(self, rhs: Self::Signed) -> Option<Self>;

    fn checked_sub_signed(self, rhs: Self::Signed) -> Option<Self>;

    fn as_wide(self) -> Self::Wide;

    fn as_signed(self) -> Self::Signed;

    fn as_usize(self) -> usize;
}

macro_rules! impl_unsigned {
    ($($t:ty { wide: $w:ty, signed: $s:ty $(,)?}),+ $(,)?) => {
        $(
            impl Unsigned for $t {
                const ZERO: Self = <$t>::MIN;
                const MAX: Self = <$t>::MAX;
                const ONE: Self = 1;
                const BITS: u32 = <$t>::BITS;

                type Wide = $w;
                type Signed = $s;

                fn wrapping_add(self, rhs: Self) -> Self {
                    <$t>::wrapping_add(self, rhs)
                }

                fn wrapping_sub(self, rhs: Self) -> Self {
                    <$t>::wrapping_sub(self, rhs)
                }

                fn checked_add(self, rhs: Self) -> Option<Self> {
                    <$t>::checked_add(self, rhs)
                }

                fn checked_sub(self, rhs: Self) -> Option<Self> {
                    <$t>::checked_sub(self, rhs)
                }

                #[track_caller]
                fn strict_add(self, rhs: Self) -> Self {
                    <$t>::strict_add(self, rhs)
                }

                #[track_caller]
                fn strict_sub(self, rhs: Self) -> Self {
                    <$t>::strict_sub(self, rhs)
                }

                fn checked_add_signed(self, rhs: Self::Signed) -> Option<Self> {
                    <$t>::checked_add_signed(self, rhs)
                }

                fn checked_sub_signed(self, rhs: Self::Signed) -> Option<Self> {
                    <$t>::checked_sub_signed(self, rhs)
                }

                fn as_wide(self) -> Self::Wide {
                    self as $w
                }

                fn as_signed(self) -> Self::Signed {
                    self as $s
                }

                fn as_usize(self) -> usize {
                    self as usize
                }
            }
            impl private::Sealed for $t {}
        )+
    };
}

impl_unsigned! {
    u8 { wide: u16, signed: i8 },
    u16 { wide: u32, signed: i16 },
    u32 { wide: u64, signed: i32 },
    u64 { wide: u128, signed: i64 },
    u128 { wide: u128, signed: i128 },
}

#[cfg(target_pointer_width = "16")]
impl_unsigned! { usize { wide: u32, signed: isize } }

#[cfg(target_pointer_width = "32")]
impl_unsigned! { usize { wide: u64, signed: isize } }

#[cfg(target_pointer_width = "64")]
impl_unsigned! { usize { wide: u128, signed: isize } }
