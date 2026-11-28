//! The device tree (kernel/kernel64/src/devicetree.rs): what the
//! firmware or the loader hands the AArch64 kernel - read before
//! anything else, and walked in full.
#![no_main]
use kernel64::devicetree::{Machine, Tree};
fuzz::fuzz_target!(|data: &[u8]| {
    let Ok(tree) = Tree::new(data) else { return 0 };
    let _ = tree.reserved().count();
    for node in tree.nodes() {
        let _ = node.base_name();
        for (name, value) in node.properties() {
            let _ = (name, value.len());
        }
        let _ = node.reg().count();
        let _ = node.numbers("interrupts").count();
        let _ = node.texts("compatible").count();
        let _ = node.enabled();
        let _ = node.children().count();
    }
    let _ = tree.find("/chosen");
    let _ = tree.compatible("arm,pl011");
    let _ = tree.by_phandle(1);
    let _ = Machine::read(&tree);
});
