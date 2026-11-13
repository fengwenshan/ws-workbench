//! 卸载执行：启动卸载程序（可选提权）、超时与退出码解析、没有卸载命令时的强制移除。
//!
//! 进程启动走 PowerShell 的 `Start-Process`（`crate::winps`），所以 `needs_elevation`
//! 就是 `-Verb RunAs`；脚本拼装与退出码解析的细节见 `start_process_script`。

use crate::cleaner::models::{CleanerProgress, InstalledApp, UninstallOutcome};

use super::parse::{
    parse_uninstall_command, uninstall_string_of, Hive, UninstallCommand, UninstallEntry,
};
use super::registry::{open_entry_key, parse_app_id, read_string};

/// 生成 `Start-Process` 提权/普通启动脚本
fn start_process_script(command: &UninstallCommand) -> String {
    let quote = |value: &str| format!("'{}'", value.replace('\'', "''"));
    // `Start-Process -ArgumentList @()` 会被 PowerShell 参数校验直接拒绝
    // （Cannot validate argument on parameter 'ArgumentList'），
    // 没有参数时必须整段省略；否则无参数卸载程序根本启动不了。
    let argument_list = if command.args.is_empty() {
        String::new()
    } else {
        let args = command
            .args
            .iter()
            .map(|arg| quote(arg))
            .collect::<Vec<_>>()
            .join(",");
        format!(" -ArgumentList @({})", args)
    };
    format!(
        "$p = Start-Process -FilePath {}{} -Wait -PassThru{}; \
         if ($null -ne $p) {{ $p.ExitCode }} else {{ 0 }}",
        quote(&command.program),
        argument_list,
        if command.needs_elevation {
            " -Verb RunAs"
        } else {
            ""
        }
    )
}

#[cfg(test)]
mod script_tests {
    use super::*;

    #[test]
    fn omits_empty_argument_list() {
        let script = start_process_script(&UninstallCommand {
            program: r"C:\Program Files\Foo\uninst.exe".to_string(),
            args: Vec::new(),
            needs_elevation: false,
        });
        assert!(!script.contains("-ArgumentList"));
        assert!(script.contains(r"'C:\Program Files\Foo\uninst.exe'"));
    }

    #[test]
    fn keeps_arguments_and_elevation() {
        let script = start_process_script(&UninstallCommand {
            program: r"C:\Foo\uninst.exe".to_string(),
            args: vec!["/S".to_string(), r"/D=C:\Foo Bar".to_string()],
            needs_elevation: true,
        });
        assert!(script.contains(r"-ArgumentList @('/S','/D=C:\Foo Bar')"));
        assert!(script.contains("-Verb RunAs"));
    }
}

/// 卸载（设计 4.2 的 `cleaner_uninstall_app`）。
///
/// 没有 `UninstallString` 的条目走「强制移除」：删掉注册表条目后由前端继续
/// 残留清理。HKLM 条目需要一次 UAC，HKCU 条目普通权限即可。
// TODO(platform-verify:windows): 真实 MSI 应用卸载（msiexec /I→/X）与 UAC 取消的结果映射未验；验证步骤：装一个 MSI 应用从卸载窗口执行一次，再对一个 HKLM 条目在 UAC 弹窗点「否」；期望：MSI 真正卸载完成且 exit_code=0，取消授权返回 ok=false、message 为「已取消授权，未执行卸载」，绝不误报成功；exe 卸载器的 0/非 0 退出码映射与「无卸载命令→强制移除」已由 windows_smoke::windows_uninstall_outcomes_smoke 覆盖。
pub fn uninstall(
    app_id: &str,
    hint: Option<&InstalledApp>,
    emit: &dyn Fn(CleanerProgress),
) -> Result<UninstallOutcome, String> {
    let (hive, key_path) = parse_app_id(app_id)?;
    let key = open_entry_key(hive, key_path)?;
    let display_name = {
        let name = read_string(&key, "DisplayName");
        if name.trim().is_empty() {
            hint.map(|app| app.name.clone()).unwrap_or_default()
        } else {
            name
        }
    };
    let entry = UninstallEntry {
        display_name: display_name.clone(),
        uninstall_string: read_string(&key, "UninstallString"),
        quiet_uninstall_string: read_string(&key, "QuietUninstallString"),
        ..Default::default()
    };
    let raw = uninstall_string_of(&entry).to_string();

    if raw.trim().is_empty() {
        // 强制移除：只删注册表条目（设计 4.5 明确不清理 COM/服务/驱动）
        emit(CleanerProgress::new(
            "uninstall",
            "running",
            0,
            1,
            "该应用没有卸载命令，正在移除注册表条目…",
        ));
        crate::cleaner::trash_guard::delete_registry_key(hive.token(), key_path)?;
        emit(CleanerProgress::new(
            "uninstall",
            "done",
            1,
            1,
            "已移除注册表条目",
        ));
        return Ok(UninstallOutcome {
            ok: true,
            command: format!("删除注册表条目: {}|{}", hive.token(), key_path),
            exit_code: None,
            elevated: false,
            message: "该应用没有卸载命令，已移除注册表条目；请继续清理残留文件".to_string(),
        });
    }

    let needs_elevation = hive != Hive::Hkcu;
    let command = parse_uninstall_command(&raw, needs_elevation)
        .ok_or_else(|| "无法解析卸载命令".to_string())?;
    emit(CleanerProgress::new(
        "uninstall",
        "running",
        0,
        1,
        format!(
            "正在运行卸载程序{}{}…",
            command.display(),
            if needs_elevation {
                "（需要管理员授权）"
            } else {
                ""
            }
        ),
    ));

    let script = start_process_script(&command);
    // winps 把「进程不能正常退出」直接映射成 Err（字符串里带 stderr），
    // 这里用 ok 标记区分「PowerShell 自己失败」和「脚本跑完但卸载器非 0」
    let (text, ok) =
        match crate::winps::run_with_timeout_ms(&script, std::time::Duration::from_secs(15 * 60)) {
            Ok(text) => (text, true),
            Err(error) => (error, false),
        };
    emit(CleanerProgress::new(
        "uninstall",
        "done",
        1,
        1,
        "卸载程序已结束",
    ));

    let command_text = command.display();
    if !ok {
        let lowered = text.to_ascii_lowercase();
        let canceled =
            lowered.contains("cancel") || text.contains("已取消") || text.contains("1223");
        let message = if text.contains("超时") {
            "卸载程序超时（15 分钟）".to_string()
        } else if canceled {
            "已取消授权，未执行卸载".to_string()
        } else {
            format!("启动卸载程序失败: {}", text.trim())
        };
        return Ok(UninstallOutcome {
            ok: false,
            command: command_text,
            exit_code: None,
            elevated: needs_elevation,
            message,
        });
    }

    let exit_code = text
        .lines()
        .rev()
        .find_map(|line| line.trim().parse::<i32>().ok());
    let (ok, message) = match exit_code {
        Some(0) => (true, "卸载程序已完成".to_string()),
        Some(code) => (false, format!("卸载程序退出码 {}，可能未完成卸载", code)),
        None => (true, "卸载程序已启动并结束（未取到退出码）".to_string()),
    };
    Ok(UninstallOutcome {
        ok,
        command: command_text,
        exit_code,
        elevated: needs_elevation,
        message,
    })
}
