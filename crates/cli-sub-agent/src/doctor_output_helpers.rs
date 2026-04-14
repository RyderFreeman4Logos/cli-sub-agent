use super::{
    DoctorEffectiveConfigStatus, render_effective_config_lines,
    render_tool_availability_error_lines,
};

pub(super) fn print_tool_availability_error(error: &str) {
    for line in render_tool_availability_error_lines(error) {
        println!("{line}");
    }
}

pub(super) fn print_effective_config(status: &DoctorEffectiveConfigStatus) {
    for line in render_effective_config_lines(status) {
        println!("{line}");
    }
}
