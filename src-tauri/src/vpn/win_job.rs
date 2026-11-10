//! Windows：把 openconnect 子进程绑到一个「worker 一死就跟着死」的作业对象上。
//!
//! 正常断开时 worker 会自己 `kill()` openconnect，这条路没问题。真正的事故是
//! worker 被强杀：任务管理器「结束任务」、崩溃、或者用户把整组 `ws-workbench.exe`
//! 一起结束 —— 这时没有任何清理代码会执行，而 Windows 默认也不会替死掉的父进程
//! 结束子进程，openconnect 变成孤儿继续跑，隧道和路由就留在后台（Wi-Fi 看似
//! 正常，但访问公司内网仍走那条已经没人管的隧道）。
//!
//! 给子进程挂一个带 `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` 的作业对象后，worker
//! 进程一结束句柄就关闭，系统会把作业里的进程一起结束，隧道随之断开。

#![cfg(target_os = "windows")]

use std::ffi::c_void;

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE};

/// 随 worker 一起消亡的作业对象。
///
/// 句柄用 `usize` 存：`HANDLE` 本身不是 `Send`，而 worker 可能被移到工作线程里。
pub(crate) struct KillOnCloseJob {
    handle: usize,
}

impl KillOnCloseJob {
    /// 把 `child`（openconnect）放进作业对象。失败时返回原因，调用方记日志后照常连接。
    pub(crate) fn attach(child: &std::process::Child) -> Result<Self, String> {
        // SAFETY: 这里的句柄都只在本函数内创建、检查和关闭；传入的指针指向栈上
        // 的作业对象配置结构，调用期间一直有效。
        unsafe {
            let job =
                CreateJobObjectW(None, None).map_err(|err| format!("创建作业对象失败: {err}"))?;

            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
            .map_err(|err| format!("设置作业对象失败: {err}"))?;

            let process = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, false, child.id())
                .map_err(|err| format!("打开 openconnect 进程失败: {err}"))?;
            let assigned = AssignProcessToJobObject(job, process);
            let _ = CloseHandle(process);
            if let Err(err) = assigned {
                let _ = CloseHandle(job);
                return Err(format!("把 openconnect 交给作业对象失败: {err}"));
            }

            Ok(Self {
                handle: job.0 as usize,
            })
        }
    }
}

impl Drop for KillOnCloseJob {
    fn drop(&mut self) {
        // SAFETY: handle 由 attach 创建、只在这里关闭一次。
        unsafe {
            let _ = CloseHandle(HANDLE(self.handle as *mut c_void));
        }
    }
}
