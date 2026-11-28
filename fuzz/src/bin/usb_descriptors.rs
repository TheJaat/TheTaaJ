//! USB descriptors (usb/src/descriptor.rs): whatever a device plugged in
//! says about itself.
#![no_main]
fuzz::fuzz_target!(|data: &[u8]| {
    let _ = usb::descriptor::device(data);
    let _ = usb::descriptor::configuration_length(data);
    let _ = usb::descriptor::configuration(data);
});
