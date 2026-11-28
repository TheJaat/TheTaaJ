//! The power service's AML reader (acpi/src/lib.rs): \_S5 found in the
//! DSDT, the firmware's own byte code.
#![no_main]
fuzz::fuzz_target!(|data: &[u8]| {
    for state in [3u8, 4, 5] {
        let _ = acpi::sleep_type(data, state);
    }
});
