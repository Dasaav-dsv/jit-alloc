# jit-alloc

Executable memory allocator that operates on a [Write-XOR-Executable](https://en.wikipedia.org/wiki/W%5EX) principle.

## Features

- `no_std` support.
- Separation of metadata from executable pages.
- Deallocation error hardening (double free, invalid pointer, invalid layout etc.)
- Multiple executable memory mapping allocation strategies on Unix-based arches.
- Configurable allocation granularity and minimum allocation alignment.
- Fuzz-tested, target-independent mock tests, miri support.

## Examples

```rust
use core::alloc::Layout;

use jit_alloc::JitAlloc;

let mut alloc = JitAlloc::<u16, 1>::new();

let layout = Layout::new::<[u8; 128]>();
let (rx, rw) = alloc.allocate(layout).unwrap();

unsafe {
    alloc.deallocate(rx, layout).unwrap();
}
```
## License

Licensed under either of

 * Apache License, Version 2.0
   ([LICENSE-APACHE](LICENSE-APACHE) or <http://www.apache.org/licenses/LICENSE-2.0>)
 * MIT license
   ([LICENSE-MIT](LICENSE-MIT) or <http://opensource.org/licenses/MIT>)

at your option.

## Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
