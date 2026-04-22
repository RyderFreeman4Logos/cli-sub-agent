use sysinfo::System;

pub(super) fn print_resource_status() {
    let mut sys = System::new();
    sys.refresh_memory();

    let available_memory_bytes = sys.available_memory();
    let free_swap_bytes = sys.free_swap();
    let total_free = available_memory_bytes.saturating_add(free_swap_bytes);

    println!(
        "Available Memory: {} (physical {} + swap {})",
        format_bytes(total_free),
        format_bytes(available_memory_bytes),
        format_bytes(free_swap_bytes),
    );
}

pub(super) fn format_bytes(bytes: u64) -> String {
    let gb = bytes as f64 / 1024.0 / 1024.0 / 1024.0;
    format!("{gb:.1} GB")
}
