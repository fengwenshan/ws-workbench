//! 会话目录：主进程与提权 worker 之间的通信通道
//!
//! 为什么不用管道：Windows 上通过 UAC/计划任务提权启动的进程**无法继承父进程的
//! 标准句柄**，`Start-Process -Verb RunAs` 也拿不到子进程输出。所以两边改为
//! 读写同一个目录里的文件：
//!
//! ```text
//! <app_data>/vpn/
//!   request.json    主进程写入的连接请求，worker 读取
//!   status.json     worker 原子覆盖写入的实时状态，主进程轮询
//!   worker.log      worker 追加的日志，主进程按偏移量增量读取
//!   stop.flag       主进程写入即请求停止，worker 轮询到后自行清理退出
//!   script/         生成的 vpnc-script 与脚本日志
//! ```
//!
//! 目录选在用户自己的 AppData 下：普通权限的主进程可写，以同一用户提权运行的
//! worker 同样可写，不需要放宽任何 ACL。

use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::vpn::models::{LearnReport, VpnRequest, VpnStatus};

pub const REQUEST_FILE: &str = "request.json";
pub const STATUS_FILE: &str = "status.json";
pub const LOG_FILE: &str = "worker.log";
pub const SCRIPT_LOG_FILE: &str = "script.log";
pub const STOP_FILE: &str = "stop.flag";
/// 学习模式报告，由 worker 每轮采样后覆盖写入
pub const LEARN_FILE: &str = "learn.json";
/// 主进程写入即请求清空学习报告（worker 在守护循环里轮询到后自己清并复位）
pub const LEARN_CLEAR_FILE: &str = "learn.clear";
pub const SCRIPT_DIR: &str = "script";
/// 每次会话的运行标识，用来判断日志文件是不是被换过
pub const RUN_ID_FILE: &str = "run.id";

/// 日志读取游标。
///
/// 只记偏移量是不够的：新会话会删掉旧日志重开一份，而新文件完全可能和旧文件
/// 一样长，单看长度会误判成「没有新内容」。
///
/// 这里不用文件创建时间做区分——Windows 有「文件系统隧道」特性，
/// 同名文件在删除后短时间内重建会**继承原来的创建时间**，靠它判断并不可靠。
/// 改成每次会话写入一个随机运行标识，标识变了就从头读。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LogCursor {
    pub offset: u64,
    pub run_id: String,
}

#[derive(Debug, Clone)]
pub struct VpnSession {
    pub dir: PathBuf,
}

impl VpnSession {
    pub fn new(dir: PathBuf) -> Self {
        VpnSession { dir }
    }

    pub fn ensure(&self) -> Result<(), String> {
        fs::create_dir_all(&self.dir)
            .map_err(|e| format!("创建会话目录失败 {}: {}", self.dir.display(), e))?;
        fs::create_dir_all(self.script_dir())
            .map_err(|e| format!("创建脚本目录失败: {}", e))?;
        Ok(())
    }

    pub fn request_path(&self) -> PathBuf {
        self.dir.join(REQUEST_FILE)
    }

    pub fn status_path(&self) -> PathBuf {
        self.dir.join(STATUS_FILE)
    }

    pub fn log_path(&self) -> PathBuf {
        self.dir.join(LOG_FILE)
    }

    pub fn script_log_path(&self) -> PathBuf {
        self.dir.join(SCRIPT_LOG_FILE)
    }

    pub fn stop_path(&self) -> PathBuf {
        self.dir.join(STOP_FILE)
    }

    pub fn script_dir(&self) -> PathBuf {
        self.dir.join(SCRIPT_DIR)
    }

    pub fn learn_path(&self) -> PathBuf {
        self.dir.join(LEARN_FILE)
    }

    pub fn learn_clear_path(&self) -> PathBuf {
        self.dir.join(LEARN_CLEAR_FILE)
    }

    /// 读取当前的运行标识
    pub fn run_id(&self) -> String {
        fs::read_to_string(self.dir.join(RUN_ID_FILE))
            .map(|text| text.trim().to_string())
            .unwrap_or_default()
    }

    /// 准备一次新会话：换一个运行标识，并清掉上一轮的残留文件
    pub fn prepare(&self) -> Result<(), String> {
        self.ensure()?;
        self.clear_stop();
        self.clear_learn_clear_flag();
        let _ = fs::write(
            self.dir.join(RUN_ID_FILE),
            uuid::Uuid::new_v4().to_string(),
        );
        for path in [
            self.status_path(),
            self.log_path(),
            self.script_log_path(),
            self.learn_path(),
        ] {
            if path.exists() {
                let _ = fs::remove_file(path);
            }
        }
        Ok(())
    }

    /// 原子写入：先写临时文件再改名，避免主进程读到写了一半的 JSON
    fn write_atomic(path: &Path, content: &str) -> Result<(), String> {
        let tmp = path.with_extension("tmp");
        {
            let mut file = fs::File::create(&tmp)
                .map_err(|e| format!("写入 {} 失败: {}", tmp.display(), e))?;
            file.write_all(content.as_bytes())
                .map_err(|e| format!("写入 {} 失败: {}", tmp.display(), e))?;
            file.flush()
                .map_err(|e| format!("刷新 {} 失败: {}", tmp.display(), e))?;
        }
        // Rust 的 fs::rename 在 Windows 上使用 MOVEFILE_REPLACE_EXISTING，可以直接覆盖
        fs::rename(&tmp, path).map_err(|e| {
            let _ = fs::remove_file(&tmp);
            format!("替换 {} 失败: {}", path.display(), e)
        })
    }

    pub fn write_request(&self, request: &VpnRequest) -> Result<(), String> {
        self.ensure()?;
        let json = serde_json::to_string_pretty(request)
            .map_err(|e| format!("序列化连接请求失败: {}", e))?;
        Self::write_atomic(&self.request_path(), &json)
    }

    pub fn read_request(&self) -> Result<VpnRequest, String> {
        let content = fs::read_to_string(self.request_path())
            .map_err(|e| format!("读取连接请求失败: {}", e))?;
        serde_json::from_str(&content).map_err(|e| format!("解析连接请求失败: {}", e))
    }

    pub fn delete_request(&self) {
        let _ = fs::remove_file(self.request_path());
    }

    pub fn write_status(&self, status: &VpnStatus) -> Result<(), String> {
        let json = serde_json::to_string_pretty(status)
            .map_err(|e| format!("序列化状态失败: {}", e))?;
        Self::write_atomic(&self.status_path(), &json)
    }

    pub fn read_status(&self) -> Option<VpnStatus> {
        let content = fs::read_to_string(self.status_path()).ok()?;
        serde_json::from_str(&content).ok()
    }

    pub fn append_log(&self, line: &str) {
        if let Ok(mut file) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.log_path())
        {
            let _ = writeln!(file, "{}", line);
        }
    }

    /// 从指定位置读取新增日志，返回 (新增行, 新游标)。
    /// 游标用借用传入，调用方后续还要拿它做比较，不需要被移走。
    pub fn read_log(&self, cursor: &LogCursor) -> (Vec<String>, LogCursor) {
        let current_run = self.run_id();
        let mut file = match fs::File::open(self.log_path()) {
            Ok(file) => file,
            Err(_) => {
                // 日志文件还不存在，但运行标识要跟着更新，免得下次误判成换了会话
                return (
                    Vec::new(),
                    LogCursor {
                        offset: 0,
                        run_id: current_run,
                    },
                )
            }
        };
        let len = file.metadata().map(|m| m.len()).unwrap_or(0);

        // 换了会话（运行标识变了），或者文件比记录的偏移量还短，
        // 都说明不是在原来那份文件上追加，必须从头读
        let start = if cursor.run_id != current_run || len < cursor.offset {
            0
        } else {
            cursor.offset
        };

        if file.seek(SeekFrom::Start(start)).is_err() {
            return (Vec::new(), cursor.clone());
        }
        let mut buffer = String::new();
        if file.read_to_string(&mut buffer).is_err() {
            return (Vec::new(), cursor.clone());
        }

        let next = LogCursor {
            offset: start.saturating_add(buffer.len() as u64),
            run_id: current_run,
        };
        let lines = buffer
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| line.to_string())
            .collect();
        (lines, next)
    }

    /// 读日志尾部若干行，用于界面首次打开时补历史
    pub fn read_log_tail(&self, max_lines: usize) -> Vec<String> {
        let content = match fs::read_to_string(self.log_path()) {
            Ok(content) => content,
            Err(_) => return Vec::new(),
        };
        let all: Vec<String> = content
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| line.to_string())
            .collect();
        let start = all.len().saturating_sub(max_lines);
        all[start..].to_vec()
    }

    pub fn request_stop(&self) {
        let _ = self.ensure();
        let _ = fs::write(self.stop_path(), b"stop");
    }

    pub fn is_stop_requested(&self) -> bool {
        self.stop_path().exists()
    }

    pub fn clear_stop(&self) {
        let _ = fs::remove_file(self.stop_path());
    }

    // ======================= 学习模式 =======================

    pub fn write_learn(&self, report: &LearnReport) -> Result<(), String> {
        let json = serde_json::to_string_pretty(report)
            .map_err(|e| format!("序列化学习报告失败: {}", e))?;
        Self::write_atomic(&self.learn_path(), &json)
    }

    pub fn read_learn(&self) -> Option<LearnReport> {
        let content = fs::read_to_string(self.learn_path()).ok()?;
        serde_json::from_str(&content).ok()
    }

    /// 清空报告：主进程自己不删文件，只插一面旗。
    ///
    /// worker 的报告是攒在内存里的，直接把 learn.json 删掉，15 秒后下一轮采样
    /// 又会把全量写回来，界面上看起来就是「点了清空没反应」。所以这里只留标记，
    /// 由 worker 自己复位内存并删文件。
    pub fn request_learn_clear(&self) {
        let _ = self.ensure();
        let _ = fs::write(self.learn_clear_path(), b"clear");
    }

    pub fn is_learn_clear_requested(&self) -> bool {
        self.learn_clear_path().exists()
    }

    pub fn clear_learn_clear_flag(&self) {
        let _ = fs::remove_file(self.learn_clear_path());
    }

    /// worker 复位报告时使用：清标记并删掉落盘的报告
    pub fn reset_learn(&self) {
        let _ = fs::remove_file(self.learn_path());
        self.clear_learn_clear_flag();
    }

    /// 连接结束后清掉明文密码
    pub fn scrub_request(&self) {
        self.delete_request();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vpn::models::ConnectorKind;

    fn temp_session(name: &str) -> VpnSession {
        let dir = std::env::temp_dir().join(format!(
            "ws-vpn-test-{}-{}",
            name,
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        let session = VpnSession::new(dir);
        session.ensure().unwrap();
        session
    }

    fn sample_request() -> VpnRequest {
        VpnRequest {
            profile_id: "p1".into(),
            profile_name: "公司 VPN".into(),
            server: "vpn.example.com:1218".into(),
            username: "13800138000".into(),
            password: "secret".into(),
            protocol: "anyconnect".into(),
            connector: ConnectorKind::Auto,
            connector_path: r"C:\app\openconnect\openconnect.exe".into(),
            rules: vec![crate::vpn::models::VpnRule {
                id: "r1".into(),
                kind: "IP-CIDR".into(),
                value: "198.51.100.0/24".into(),
                policy: "VPN".into(),
                no_resolve: true,
                remark: String::new(),
                enabled: true,
            }],
            default_policy: "DIRECT".into(),
            fake_ip_range: "198.18.0.0/16".into(),
            fake_ip_filter: vec![],
            learn: false,
            extra_args: vec![],
            script_path: r"C:\session\script\vpnc-script-win.js".into(),
            hook_exe: r"C:\app\ws-workbench.exe".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
        }
    }

    #[test]
    fn request_roundtrip() {
        let session = temp_session("req");
        session.write_request(&sample_request()).unwrap();
        let back = session.read_request().unwrap();
        assert_eq!(back.server, "vpn.example.com:1218");
        assert_eq!(back.default_policy, "DIRECT");
        assert_eq!(back.rules.len(), 1);
        assert_eq!(back.rules[0].value, "198.51.100.0/24");
        // 规则的 kind/policy 必须能被引擎解析，否则运行期会被静默丢弃
        assert!(crate::vpn::rule_engine::RuleKind::parse(&back.rules[0].kind).is_ok());
        assert!(crate::vpn::rule_engine::Policy::parse(&back.rules[0].policy).is_ok());
        let _ = fs::remove_dir_all(&session.dir);
    }

    #[test]
    fn status_roundtrip() {
        let session = temp_session("status");
        let mut status = VpnStatus::default();
        status.phase = crate::vpn::models::VpnPhase::Connected;
        status.vpn_ip = "198.51.100.39".into();
        session.write_status(&status).unwrap();
        let back = session.read_status().unwrap();
        assert_eq!(back.phase, crate::vpn::models::VpnPhase::Connected);
        assert_eq!(back.vpn_ip, "198.51.100.39");
        let _ = fs::remove_dir_all(&session.dir);
    }

    #[test]
    fn status_write_is_atomic_and_leaves_no_tmp() {
        let session = temp_session("atomic");
        let status = VpnStatus::default();
        session.write_status(&status).unwrap();
        session.write_status(&status).unwrap();
        assert!(session.status_path().exists());
        assert!(!session.status_path().with_extension("tmp").exists());
        let _ = fs::remove_dir_all(&session.dir);
    }

    #[test]
    fn read_log_returns_only_new_lines() {
        let session = temp_session("log");
        session.append_log("第一行");
        let (lines, cursor) = session.read_log(&LogCursor::default());
        assert_eq!(lines, vec!["第一行".to_string()]);

        session.append_log("第二行");
        let (lines, cursor2) = session.read_log(&cursor);
        assert_eq!(lines, vec!["第二行".to_string()]);
        assert!(cursor2.offset > cursor.offset);

        // 没有新内容时不重复推送
        let (lines, _) = session.read_log(&cursor2);
        assert!(lines.is_empty());
        let _ = fs::remove_dir_all(&session.dir);
    }

    #[test]
    fn read_log_recovers_when_file_shrinks() {
        let session = temp_session("shrink");
        session.append_log("很长的一行内容用来占位");
        let (_, cursor) = session.read_log(&LogCursor::default());
        fs::remove_file(session.log_path()).unwrap();
        session.append_log("新");
        // 文件比记录的偏移量短，应该从 0 重新读
        let (lines, _) = session.read_log(&cursor);
        assert_eq!(lines, vec!["新".to_string()]);
        let _ = fs::remove_dir_all(&session.dir);
    }

    #[test]
    fn read_log_restarts_when_session_is_recreated() {
        let session = temp_session("rerun");
        session.prepare().unwrap();
        session.append_log("旧的");
        let (_, cursor) = session.read_log(&LogCursor::default());

        // 新会话：运行标识换掉、日志重开，而且两份文件刚好一样长，
        // 只看长度是发现不了的，必须靠运行标识
        session.prepare().unwrap();
        session.append_log("新的");
        assert_eq!(session.log_path().metadata().unwrap().len(), cursor.offset);

        let (lines, _) = session.read_log(&cursor);
        assert_eq!(lines, vec!["新的".to_string()]);
        let _ = fs::remove_dir_all(&session.dir);
    }

    #[test]
    fn run_id_changes_on_each_session() {
        let session = temp_session("runid");
        session.prepare().unwrap();
        let first = session.run_id();
        assert!(!first.is_empty());
        session.prepare().unwrap();
        assert_ne!(first, session.run_id());
        let _ = fs::remove_dir_all(&session.dir);
    }

    #[test]
    fn read_log_without_file_keeps_run_id_in_sync() {
        let session = temp_session("nofile");
        session.prepare().unwrap();
        let (lines, cursor) = session.read_log(&LogCursor::default());
        assert!(lines.is_empty());
        // 即使还没有日志文件，游标也要带上当前运行标识
        assert_eq!(cursor.run_id, session.run_id());
        let _ = fs::remove_dir_all(&session.dir);
    }

    #[test]
    fn read_log_tail_keeps_last_lines() {
        let session = temp_session("tail");
        for index in 0..10 {
            session.append_log(&format!("第 {} 行", index));
        }
        let tail = session.read_log_tail(3);
        assert_eq!(tail, vec!["第 7 行", "第 8 行", "第 9 行"]);
        // 行数不足时全给
        assert_eq!(session.read_log_tail(99).len(), 10);
        let _ = fs::remove_dir_all(&session.dir);
    }

    #[test]
    fn stop_flag_lifecycle() {
        let session = temp_session("stop");
        assert!(!session.is_stop_requested());
        session.request_stop();
        assert!(session.is_stop_requested());
        session.clear_stop();
        assert!(!session.is_stop_requested());
        let _ = fs::remove_dir_all(&session.dir);
    }

    #[test]
    fn prepare_clears_previous_run() {
        let session = temp_session("prepare");
        session.write_status(&VpnStatus::default()).unwrap();
        session.append_log("上一轮日志");
        session.request_stop();
        session.prepare().unwrap();
        assert!(session.read_status().is_none());
        assert!(!session.is_stop_requested());
        assert!(session.script_dir().exists());
        let _ = fs::remove_dir_all(&session.dir);
    }

    #[test]
    fn learn_report_roundtrip_and_clear_flag() {
        let session = temp_session("learn");
        let mut report = LearnReport::default();
        report.enabled = true;
        report.scans = 3;
        session.write_learn(&report).unwrap();
        let back = session.read_learn().unwrap();
        assert!(back.enabled);
        assert_eq!(back.scans, 3);
        // 原子替换不能在目录里留下 .tmp
        assert!(!session.learn_path().with_extension("tmp").exists());

        // 清空请求走旗标文件：主进程不直接删报告，避免 worker 下一轮又写回来
        session.request_learn_clear();
        assert!(session.is_learn_clear_requested());
        assert!(session.read_learn().is_some());
        session.reset_learn();
        assert!(!session.is_learn_clear_requested());
        assert!(session.read_learn().is_none());
        let _ = fs::remove_dir_all(&session.dir);
    }

    #[test]
    fn prepare_clears_previous_learn_report() {
        let session = temp_session("learn-prepare");
        session.write_learn(&LearnReport::default()).unwrap();
        session.request_learn_clear();
        session.prepare().unwrap();
        assert!(session.read_learn().is_none());
        assert!(!session.is_learn_clear_requested());
        let _ = fs::remove_dir_all(&session.dir);
    }
}
