//! 非 macOS / Windows 平台的占位实现：保持门面接口存在，不做任何操作。

use super::super::models::InstalledApp;
use super::RunningProcess;

pub(super) fn detect(_app: &InstalledApp) -> Vec<RunningProcess> {
    Vec::new()
}

pub(super) fn terminate(_app: &InstalledApp, _processes: &[RunningProcess]) {}

pub(super) fn kill(_processes: &[RunningProcess]) {}
