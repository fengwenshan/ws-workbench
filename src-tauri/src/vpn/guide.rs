//! openconnect 不可用时展示的安装引导
//!
//! 这里刻意给「可复制的命令」和「官方下载地址」，不写任何臆测的步骤。
//! 三个平台各自有分支 —— 不要写成「不是 Windows 就当 macOS」，
//! 那样 Linux 用户会看到 brew 的安装命令。

use crate::platform::Os;
use crate::vpn::models::{GuideLink, GuideStep, InstallGuide};

fn link(label: &str, url: &str) -> GuideLink {
    GuideLink {
        label: label.to_string(),
        url: url.to_string(),
    }
}

fn step(title: &str, detail: &str, command: &str) -> GuideStep {
    GuideStep {
        title: title.to_string(),
        detail: detail.to_string(),
        command: command.to_string(),
    }
}

/// 根据平台与缺失原因生成引导内容
pub fn build(os: Os, openconnect_available: bool) -> InstallGuide {
    match os {
        Os::Windows => windows_guide(openconnect_available),
        Os::Macos => macos_guide(openconnect_available),
        Os::Linux => linux_guide(openconnect_available),
    }
}

fn windows_guide(openconnect_available: bool) -> InstallGuide {
    if !openconnect_available {
        return InstallGuide {
            platform: "windows".to_string(),
            title: "安装包内置的 openconnect 缺失或无法启动".to_string(),
            description: "Windows 版固定使用安装包自带的 openconnect，不再读取注册表、\
                常见安装目录或 PATH，也不会导入外部的 openconnect。走到这里说明\
                resources/apps/openconnect 里的副本不完整 —— 最常见的是解压不完整、\
                缺少同目录 DLL，或被安全软件拦截。"
                .to_string(),
            steps: vec![
                step(
                    "确认安装目录完整",
                    "检查安装目录下的 resources\\apps\\openconnect 是否同时存在\
                     openconnect.exe 与 libopenconnect-5.dll、wintun.dll 等配套 DLL。",
                    "",
                ),
                step(
                    "重新安装本程序",
                    "用官方安装包覆盖安装一次；如果杀毒软件报过拦截，先把安装目录加入白名单。",
                    "",
                ),
            ],
            links: vec![],
        };
    }

    InstallGuide {
        platform: "windows".to_string(),
        title: "连接器正常（安装包内置）".to_string(),
        description: "openconnect 来自安装包内置副本，可以开始配置 VPN 了。".to_string(),
        steps: vec![],
        links: vec![],
    }
}

fn macos_guide(openconnect_available: bool) -> InstallGuide {
    let mut steps = vec![step(
        "用 Homebrew 安装 openconnect",
        "必须装在标准前缀：Apple Silicon 是 /opt/homebrew，Intel 机器是 /usr/local。\
         本程序只自动识别这两个位置，装到自定义前缀（例如 ~/hb）不会被当成连接器。",
        "brew install openconnect",
    )];
    steps.push(step(
        "确认安装位置",
        "brew --prefix 应输出 /opt/homebrew（Intel 机器是 /usr/local），\
         which openconnect 应指向 <前缀>/bin/openconnect。\
         已经装在其它前缀里的，用下面「选择安装目录」选中 openconnect 所在的目录（里面有 bin/openconnect），不需要重装。",
        "brew --prefix && which openconnect",
    ));

    InstallGuide {
        platform: "macos".to_string(),
        title: if openconnect_available {
            "连接器正常".to_string()
        } else {
            "未找到 openconnect，请先安装".to_string()
        },
        description: "macOS 上创建 utun 接口需要 root 权限，本程序会在连接时\
            弹出一次系统授权框（与 Windows 的 UAC 等价）。"
            .to_string(),
        steps,
        links: vec![
            link("Homebrew openconnect 页面", "https://formulae.brew.sh/formula/openconnect"),
            link("openconnect 官方文档", "https://www.infradead.org/openconnect/"),
        ],
    }
}

fn linux_guide(openconnect_available: bool) -> InstallGuide {
    InstallGuide {
        platform: "linux".to_string(),
        title: if openconnect_available {
            "连接器正常".to_string()
        } else {
            "未找到 openconnect，请先安装".to_string()
        },
        description: "Linux 下创建隧道并修改路由需要 root 权限或 CAP_NET_ADMIN 能力，\
            本程序会提示你以相应权限运行。"
            .to_string(),
        steps: vec![
            step(
                "从发行版仓库安装 openconnect",
                "Debian/Ubuntu 与 RHEL 系的名字一致，装完在 /usr/bin/openconnect。",
                "sudo apt install openconnect",
            ),
            step(
                "确认安装位置",
                "如果装在了非标准前缀（例如 /usr/local 或自定义目录），\
                 用下面「选择安装目录」选中 openconnect 所在的目录（里面有 bin/openconnect），不需要重装。",
                "which openconnect",
            ),
        ],
        links: vec![
            link(
                "openconnect 官方文档",
                "https://www.infradead.org/openconnect/",
            ),
            link(
                "各发行版安装包说明",
                "https://www.infradead.org/openconnect/packages.html",
            ),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_without_openconnect_explains_builtin_and_reinstall() {
        let guide = build(Os::Windows, false);
        assert!(guide.title.contains("内置"), "实际: {}", guide.title);
        let mut text = format!("{}\n{}", guide.title, guide.description);
        for step in &guide.steps {
            text.push_str(&format!("\n{}\n{}", step.title, step.detail));
        }
        // 用户得知道「内置那份为什么不好使」，才知道要找什么
        assert!(text.contains("DLL"), "缺少 DLL 的说明: {}", text);
        assert!(
            text.contains(r"resources\apps\openconnect") && text.contains("重新安装"),
            "应该把用户引导到安装目录并建议覆盖安装: {}",
            text
        );
        assert!(!guide.links.iter().any(|item| item.url.contains("openconnect-vpn.net")));
    }

    #[test]
    fn windows_ok_guide_is_empty() {
        let guide = build(Os::Windows, true);
        assert!(guide.steps.is_empty());
        assert!(guide.links.is_empty());
    }

    #[test]
    fn macos_guide_gives_brew_command() {
        let guide = build(Os::Macos, false);
        assert!(guide
            .steps
            .iter()
            .any(|step| step.command.contains("brew install openconnect")));
        assert!(guide.description.contains("授权"));
    }

    #[test]
    fn macos_guide_reports_ok_state() {
        let guide = build(Os::Macos, true);
        assert_eq!(guide.title, "连接器正常");
    }

    /// 事实性回归：引导里不能再出现第二个通道或图形界面客户端。
    /// 现在只有 openconnect 一条路，任何「备选 / 降级」的说法都会误导用户。
    #[test]
    fn no_guide_mentions_a_second_channel() {
        for os in [Os::Windows, Os::Macos, Os::Linux] {
            for available in [true, false] {
                let guide = build(os, available);
                let mut text = format!("{}\n{}", guide.title, guide.description);
                for step in &guide.steps {
                    text.push_str(&format!("\n{}\n{}", step.title, step.detail));
                }
                for item in &guide.links {
                    text.push_str(&format!("\n{}", item.label));
                }
                for forbidden in ["Cisco", "AnyConnect", "Secure Client", "GUI"] {
                    assert!(
                        !text.contains(forbidden),
                        "{:?} 引导里残留了「{}」: {}",
                        os,
                        forbidden,
                        text
                    );
                }
            }
        }
    }

    /// 关键回归点：Linux 不能拿到 brew 命令
    #[test]
    fn linux_guide_does_not_mention_homebrew() {
        let guide = build(Os::Linux, false);
        assert_eq!(guide.platform, "linux");
        assert!(
            !guide
                .steps
                .iter()
                .any(|step| step.command.contains("brew")),
            "Linux 引导里不该出现 Homebrew"
        );
        assert!(guide
            .steps
            .iter()
            .any(|step| step.command.contains("apt install openconnect")));
    }

    #[test]
    fn every_platform_gets_a_distinct_guide() {
        let platforms = [Os::Windows, Os::Macos, Os::Linux];
        let names: Vec<String> = platforms
            .iter()
            .map(|os| build(*os, false).platform)
            .collect();
        assert_eq!(names, vec!["windows", "macos", "linux"]);
    }

    /// brew 装在自定义前缀（例如 ~/hb）时程序探测不到，引导必须先说清「必须装到标准前缀」，
    /// 并把兜底指向真实存在的「选择安装目录」，不能再提已经删掉的「设置页手动指定」。
    #[test]
    fn macos_guide_requires_standard_prefix_and_real_fallback() {
        let guide = build(Os::Macos, false);
        let mut text = format!("{}\n{}", guide.title, guide.description);
        for step in &guide.steps {
            text.push_str(&format!("\n{}\n{}", step.title, step.detail));
        }

        assert!(text.contains("/opt/homebrew"), "缺少标准前缀地址: {}", text);
        assert!(text.contains("自定义前缀"), "要点明自定义前缀不被识别: {}", text);
        assert!(text.contains("选择安装目录"), "兜底入口名称要和界面按钮一致: {}", text);
        assert!(!text.contains("设置"), "设置页里没有这个入口，不能再引导过去: {}", text);
    }

    /// Linux 引导同样不能指向不存在的设置项
    #[test]
    fn linux_guide_points_at_real_fallback() {
        let guide = build(Os::Linux, false);
        let mut text = String::new();
        for step in &guide.steps {
            text.push_str(&format!("\n{}\n{}", step.title, step.detail));
        }
        assert!(text.contains("选择安装目录"), "兜底入口名称要和界面按钮一致: {}", text);
        assert!(!text.contains("设置"), "设置页里没有这个入口: {}", text);
    }
}
