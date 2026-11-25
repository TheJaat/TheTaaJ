//! What a program may ask about its own loading: dl_iterate_phdr,
//! getauxval, and dlfcn.h's dlopen, dlsym and the rest.
//!
//! Every program here is one static file, loaded whole by the kernel
//! where its linker put it: there is no dynamic linker, no shared
//! object, nothing loaded later. So:
//!
//! - **dl_iterate_phdr** calls back once, for the program itself: its
//!   program headers (a copy, from the kernel's PROGRAM_HEADERS - the
//!   kernel keeps what the ELF file said), at load bias 0, its
//!   thread-local block. That is what an unwinder (Rust's backtraces,
//!   C++'s libunwind) and Rust's std use it for: finding
//!   PT_GNU_EH_FRAME and PT_TLS.
//! - **getauxval** answers what Linux's auxiliary vector would have:
//!   the headers, the page size, 16 random bytes; 0 and ENOENT for the
//!   rest. The kernel passes no auxiliary vector; libc makes these up
//!   from what it can ask.
//! - **dlopen** opens nothing but the program itself (a null name), and
//!   **dlsym** finds nothing in it - the symbol table is not loaded.
//!   Callers that look up a function "if it is there" (Rust's std does,
//!   for newer Linux calls) then do without it, which is what they are
//!   written to do.

use alloc::boxed::Box;
use alloc::vec;
use core::ffi::{c_char, c_int, c_ulong, c_void};
use core::ptr;

use crate::errno::{ENOENT, errno};
use core::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};

/// One ELF program header's size, as the kernel copies them.
const PHENT: usize = 56;

/// The program's headers, copied once: (address, count).
static HEADERS: AtomicPtr<u8> = AtomicPtr::new(ptr::null_mut());
static HEADER_COUNT: AtomicUsize = AtomicUsize::new(0);

/// The copy of the program headers, made at the first call. Two
/// threads asking at once may both make one; one is kept (the other
/// leaks: a few hundred bytes, once).
fn headers() -> (*const u8, usize) {
    let made = HEADERS.load(Ordering::Acquire);
    if !made.is_null() {
        return (made, HEADER_COUNT.load(Ordering::Relaxed));
    }
    let total = taaj::program_headers(&mut []).unwrap_or(0);
    let mut copy = vec![0u8; total.max(PHENT)];
    let total = taaj::program_headers(&mut copy).unwrap_or(0).min(copy.len());
    let count = total / PHENT;
    let copy = Box::leak(copy.into_boxed_slice()).as_mut_ptr();
    HEADER_COUNT.store(count, Ordering::Relaxed);
    match HEADERS.compare_exchange(ptr::null_mut(), copy, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => (copy, count),
        Err(first) => (first, count),
    }
}

/// link.h's struct dl_phdr_info, as glibc lays it out.
#[repr(C)]
pub struct DlPhdrInfo {
    dlpi_addr: usize,
    dlpi_name: *const c_char,
    dlpi_phdr: *const c_void,
    dlpi_phnum: u16,
    dlpi_adds: u64,
    dlpi_subs: u64,
    dlpi_tls_modid: usize,
    dlpi_tls_data: *mut c_void,
}

type Callback = unsafe extern "C" fn(*mut DlPhdrInfo, usize, *mut c_void) -> c_int;

const PT_TLS: u32 = 7;

/// `dl_iterate_phdr(callback, data)`: calls `callback` for each loaded
/// object - here, only the program - with its headers; stops early if
/// it returns non-zero, and returns what it last returned.
///
/// # Safety
/// `callback` must be a function of that type.
#[no_mangle]
pub unsafe extern "C" fn dl_iterate_phdr(callback: Option<Callback>, data: *mut c_void) -> c_int {
    let Some(callback) = callback else { return 0 };
    let (phdr, count) = headers();
    // SAFETY: `count` headers of PHENT bytes each at `phdr`.
    let has_tls = (0..count).any(|n| unsafe { phdr.add(n * PHENT).cast::<u32>().read_unaligned() } == PT_TLS);
    let mut info = DlPhdrInfo {
        dlpi_addr: 0,
        dlpi_name: c"".as_ptr(),
        dlpi_phdr: phdr.cast(),
        dlpi_phnum: count as u16,
        dlpi_adds: 1,
        dlpi_subs: 0,
        dlpi_tls_modid: usize::from(has_tls),
        dlpi_tls_data: if has_tls { crate::threads::tls_block().cast() } else { ptr::null_mut() },
    };
    // SAFETY: the caller's promise; `info` lives through the call.
    unsafe { callback(&mut info, core::mem::size_of::<DlPhdrInfo>(), data) }
}

// sys/auxv.h's names, Linux's numbers.
const AT_PHDR: c_ulong = 3;
const AT_PHENT: c_ulong = 4;
const AT_PHNUM: c_ulong = 5;
const AT_PAGESZ: c_ulong = 6;
const AT_BASE: c_ulong = 7;
const AT_FLAGS: c_ulong = 8;
const AT_UID: c_ulong = 11;
const AT_EUID: c_ulong = 12;
const AT_GID: c_ulong = 13;
const AT_EGID: c_ulong = 14;
const AT_HWCAP: c_ulong = 16;
const AT_CLKTCK: c_ulong = 17;
const AT_SECURE: c_ulong = 23;
const AT_RANDOM: c_ulong = 25;
const AT_HWCAP2: c_ulong = 26;

/// AT_RANDOM's 16 bytes: made once, at the first ask.
static RANDOM: AtomicPtr<u8> = AtomicPtr::new(ptr::null_mut());

fn random_bytes() -> *mut u8 {
    let made = RANDOM.load(Ordering::Acquire);
    if !made.is_null() {
        return made;
    }
    let mut bytes = Box::new([0u8; 16]);
    let _ = taaj::random(&mut bytes[..]);
    let bytes = Box::leak(bytes).as_mut_ptr();
    match RANDOM.compare_exchange(ptr::null_mut(), bytes, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => bytes,
        Err(first) => first,
    }
}

/// `getauxval(type)`: what the auxiliary vector would say about `type`.
/// The program's headers, their size and number; the page size; the
/// clock's ticks per second; 16 random bytes; 0 for the ids, the flags,
/// the hardware capabilities (not looked at yet), the interpreter's
/// base (there is none) and AT_SECURE. Any other type: 0 and ENOENT.
#[no_mangle]
pub extern "C" fn getauxval(kind: c_ulong) -> c_ulong {
    match kind {
        AT_PHDR => headers().0 as c_ulong,
        AT_PHENT => PHENT as c_ulong,
        AT_PHNUM => headers().1 as c_ulong,
        AT_PAGESZ => 4096,
        AT_CLKTCK => abi::TICKS_PER_SECOND as c_ulong,
        AT_RANDOM => random_bytes() as c_ulong,
        AT_BASE | AT_FLAGS | AT_UID | AT_EUID | AT_GID | AT_EGID | AT_HWCAP | AT_HWCAP2 | AT_SECURE => 0,
        _ => {
            errno.store(ENOENT, Ordering::Relaxed);
            0
        }
    }
}

/// What dlerror says next: set by a failed dlopen or dlsym, cleared by
/// reading it.
static LAST: AtomicPtr<c_char> = AtomicPtr::new(ptr::null_mut());

fn failed(why: &'static core::ffi::CStr) {
    LAST.store(why.as_ptr().cast_mut(), Ordering::Relaxed);
}

/// The handle dlopen(NULL) gives: the program itself. Any non-null
/// pointer would do; this one is recognisable.
static SELF: u8 = 0;

/// `dlopen(name, flags)`: with a null name, a handle for the program
/// itself; anything else fails - there are no shared objects to load.
///
/// # Safety
/// `name` null or a string.
#[no_mangle]
pub unsafe extern "C" fn dlopen(name: *const c_char, _flags: c_int) -> *mut c_void {
    if name.is_null() {
        return ptr::addr_of!(SELF).cast_mut().cast();
    }
    failed(c"dlopen: no shared objects on TheTaaJ - every program is linked whole");
    ptr::null_mut()
}

/// `dlsym(handle, name)`: always null - the program's symbol table is
/// not kept in memory. dlerror says so.
///
/// # Safety
/// Nothing is read.
#[no_mangle]
pub unsafe extern "C" fn dlsym(_handle: *mut c_void, _name: *const c_char) -> *mut c_void {
    failed(c"dlsym: symbol not found (programs keep no symbol table at run time)");
    ptr::null_mut()
}

/// `dlclose(handle)`: nothing to unload. 0.
#[no_mangle]
pub extern "C" fn dlclose(_handle: *mut c_void) -> c_int {
    0
}

/// `dlerror()`: what the last failed dlopen or dlsym said, once; null
/// if nothing failed since the last ask.
#[no_mangle]
pub extern "C" fn dlerror() -> *mut c_char {
    LAST.swap(ptr::null_mut(), Ordering::Relaxed)
}

/// dlfcn.h's Dl_info.
#[repr(C)]
pub struct DlInfo {
    dli_fname: *const c_char,
    dli_fbase: *mut c_void,
    dli_sname: *const c_char,
    dli_saddr: *mut c_void,
}

/// `dladdr(address, info)`: 0 - which symbol an address is in is not
/// known without the symbol table.
///
/// # Safety
/// Nothing is written.
#[no_mangle]
pub unsafe extern "C" fn dladdr(_address: *const c_void, _info: *mut DlInfo) -> c_int {
    0
}
