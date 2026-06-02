use std::process::{Command, Stdio};
use std::time::Duration;

use super::{ProcessTreeActivity, ProcessTreeStatus};

#[cfg(target_os = "linux")]
#[test]
fn process_tree_activity_reports_cpu_progress_for_busy_child() {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg("while :; do :; done")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn busy child");
    let mut activity = ProcessTreeActivity::new(child.id());

    assert_eq!(activity.observe(), ProcessTreeStatus::AliveIdle);
    std::thread::sleep(Duration::from_millis(80));
    assert_eq!(activity.observe(), ProcessTreeStatus::AliveWithCpuProgress);

    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(target_os = "linux")]
#[test]
fn process_tree_activity_keeps_sleeping_child_idle() {
    let mut child = Command::new("sleep")
        .arg("5")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn sleeping child");
    let mut activity = ProcessTreeActivity::new(child.id());

    assert_eq!(activity.observe(), ProcessTreeStatus::AliveIdle);
    std::thread::sleep(Duration::from_millis(80));
    assert_eq!(activity.observe(), ProcessTreeStatus::AliveIdle);

    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(target_os = "linux")]
#[test]
fn process_tree_activity_reports_dead_after_child_exits() {
    let mut child = Command::new("true").spawn().expect("spawn short child");
    let pid = child.id();
    child.wait().expect("wait short child");

    let mut activity = ProcessTreeActivity::new(pid);
    assert_eq!(activity.observe(), ProcessTreeStatus::Dead);
}
