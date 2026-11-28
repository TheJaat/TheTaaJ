//! The kernel's ACPI tables (kernel/kernel64/src/acpi.rs): RSDP, RSDT or
//! XSDT, MADT and FADT, as the firmware left them in memory. The input
//! is that memory, from address 0; the RSDP is at 0.
#![no_main]
use kernel64::acpi::{read_table, Acpi, Fadt, Madt, PhysicalMemory};

struct Memory<'a>(&'a [u8]);

impl PhysicalMemory for Memory<'_> {
    fn read(&self, address: u64, buffer: &mut [u8]) -> bool {
        let Ok(start) = usize::try_from(address) else { return false };
        match start.checked_add(buffer.len()).and_then(|end| self.0.get(start..end)) {
            Some(bytes) => {
                buffer.copy_from_slice(bytes);
                true
            }
            None => false,
        }
    }
}

fuzz::fuzz_target!(|data: &[u8]| {
    let memory = Memory(data);
    if let Ok(table) = read_table(&memory, 0) {
        let _ = Madt::parse(&table).map(|madt| (0..16).map(|irq| madt.isa_route(irq)).count());
        let _ = Fadt::parse(&table);
    }
    let Ok(acpi) = Acpi::new(&memory, 0) else { return 0 };
    for signature in acpi.signatures(&memory) {
        if let Ok(table) = acpi.find(&memory, &signature) {
            let _ = Madt::parse(&table);
            let _ = Fadt::parse(&table);
        }
    }
});
