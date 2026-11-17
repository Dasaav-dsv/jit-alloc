use core::num::NonZero;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[repr(transparent)]
pub struct NonMax<T: Zeroable>(T::NonZero);

pub trait Zeroable {
    type NonZero;
}

macro_rules! impl_nonmax {
    ($($t:ty),+ $(,)?) => {
        $(
            impl Zeroable for $t {
                type NonZero = NonZero<$t>;
            }

            impl NonMax<$t> {
                pub const fn new(value: $t) -> Option<Self> {
                    match NonZero::new(value.wrapping_add(1)) {
                        Some(nonzero) => Some(Self(nonzero)),
                        None => None,
                    }
                }

                pub const fn get(self) -> $t {
                    self.0.get().wrapping_sub(1)
                }
            }

            impl Default for NonMax<$t> {
                fn default() -> Self {
                    const { Self::new(0).unwrap() }
                }
            }
        )+
    };
}

impl_nonmax! { u16, u32 }
