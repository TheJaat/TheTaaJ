//! The fuzz targets' shared part (TheTaaJ K189).
//!
//! Each program in src/bin/ is one target: libFuzzer (LLVM's
//! `libclang_rt.fuzzer`, linked in by tools/fuzz.sh) has the `main`, and
//! calls the target's `LLVMFuzzerTestOneInput` with inputs it makes up -
//! steered by which branches of the code each input reached, which the
//! compiler counts (`-C passes=sancov-module`). An input that makes the
//! target panic - an index out of range, an overflow, an `unwrap` of an
//! `Err` - is a crash: libFuzzer writes it to a file, and it becomes a
//! test (`tools/fuzz.sh` runs every saved input again, `make fuzz-check`).
//!
//! A parser of untrusted input may say "no" to anything; it may not
//! panic, loop for ever or read outside what it was given.

/// `fuzz_target!(|data: &[u8]| { ... })`: the target's body, called
/// with each input.
#[macro_export]
macro_rules! fuzz_target {
    (|$data:ident: &[u8]| $body:block) => {
        #[no_mangle]
        pub extern "C" fn LLVMFuzzerTestOneInput(bytes: *const u8, size: usize) -> i32 {
            let $data: &[u8] = if size == 0 { &[] } else { unsafe { core::slice::from_raw_parts(bytes, size) } };
            $body
            0
        }
    };
}

/// Splits an input into pieces: each is a length byte, then that many
/// bytes (fewer at the end). For targets that take several messages.
pub fn pieces(mut data: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    while let Some((&length, rest)) = data.split_first() {
        let length = (length as usize).min(rest.len());
        out.push(&rest[..length]);
        data = &rest[length..];
    }
    out
}
