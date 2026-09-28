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
            title: "内置的 openconnect 没能启动，需要补一份可用的副本".to_string(),
            description: "本程序自带一份 openconnect（带 Wintun 驱动的 Windows 版），正常情况下你不需要装任何东西。\
                走到这里说明那份副本缺失或跑不起来 —— 最常见的原因是解压不完整、\
                缺少配套 DLL，或者被安全软件拦截。请从下方的官方来源取一份带 openconnect.exe 的\
                完整发行包，然后按下面两步交给本程序。"
                .to_string(),
            steps: vec![
                step(
                    "准备一份完整的 openconnect 发行包",
                    "需要的是压缩包里含 openconnect.exe 与同目录 DLL 的那一类（Windows 版通常\
                     由 MinGW64 构建）。只下载单个 exe 是不行的，缺 DLL 会直接启动失败。",
                    "",
                ),
                step(
                    "交给本程序托管",
                    "到「设置」页找到 OpenConnect 这一项：「导入目录」会把 openconnect.exe 和\
                     同目录的 DLL 一起复制到程序自己的数据目录，之后连接就用这份，\
                     不需要管理员权限、也不污染你的安装目录；\
                     也可以直接「手动指定」选中 openconnect.exe 使用。",
                    "",
                ),
            ],
            links: vec![
                link("openconnect 官方文档", "https://www.infradead.org/openconnect/"),
                link(
                    "Windows 安装包说明",
                    "https://www.infradead.org/openconnect/packages.html",
                ),
            ],
        };
    }

    InstallGuide {
        platform: "windows".to_string(),
        title: "连接器正常".to_string(),
        description: "openconnect 已可用，可以开始配置 VPN 了。".to_string(),
        steps: vec![],
        links: vec![],
    }
}

fn macos_guide(openconnect_available: bool) -> InstallGuide {
    let mut steps = vec![step(
        "用 Homebrew 安装 openconnect",
        "Apple Silicon 会装到 /opt/homebrew/bin，Intel 机器在 /usr/local/bin，两个位置本程序都会探测。",
        "brew install openconnect",
    )];
    steps.push(step(
        "确认安装位置",
        "如果 brew 装在了非标准前缀，到「设置」页找到 OpenConnect 这一项，用「手动指定」选一下即可。",
        "which openconnect",
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
                 到「设置」页找到 OpenConnect 这一项，用「手动指定」选一下即可。",
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
    fn windows_without_openconnect_explains_builtin_and_points_to_settings() {
        let guide = build(Os::Windows, false);
        assert!(guide.title.contains("内置"), "实际: {}", guide.title);
        let mut text = format!("{}\n{}", guide.title, guide.description);
        for step in &guide.steps {
            text.push_str(&format!("\n{}\n{}", step.title, step.detail));
        }
        // 用户得知道「内置那份为什么不好使」，才知道要找什么
        assert!(text.contains("DLL"), "缺少 DLL 的说明: {}", text);
        assert!(
            text.contains("设置") && text.contains("导入目录"),
            "应该把用户引导到设置页的导入入口: {}",
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
}
