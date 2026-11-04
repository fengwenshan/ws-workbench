#![cfg(target_os = "macos")]
//! 进程名反查：`sysctl net.inet.tcp.pcblist_n` / `net.inet.udp.pcblist_n`
//! 拿本机 socket 表，按本地端口匹配到 PID，再用 `proc_pidpath` 取进程名。
//!
//! pcblist 的二进制布局随内核版本变化；`parse_pcblist` 用记录头里的长度字段
//! 游走，与字段偏移无关；字段偏移集中为常量，运行期需用 `lsof` 交叉校验。

use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::net::Ipv4Addr;

use super::platform::{FlowKey, ProcLookup, Transport};

/// 单条 socket 记录里我们关心的字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PcbRecord {
    /// 本地端口（主机序）
    pub lport: u16,
    /// 远端地址（主机序 IPv4）
    pub faddr: Ipv4Addr,
    pub pid: u32,
}

// 字段偏移（xudpcb_n / xtcpcb_n 内的 xinpcb_n 头部）。随内核版本可能变化，
// 落地时用 `lsof -nP -iTCP` 交叉校验；不一致时只调这三个常量。
const OFF_LPORT: usize = 0x34;
const OFF_FADDR: usize = 0x38;
const OFF_PID: usize = 0x58;

/// 从 `proc_pidpath` 输出（NUL 结尾的绝对路径）取末段可执行名。
pub fn proc_pidpath_to_name(out: &str) -> Option<String> {
    let trimmed = out.trim_end_matches('\0').trim();
    if trimmed.is_empty() {
        return None;
    }
    trimmed.rsplit('/').next().map(|s| s.to_string())
}

pub struct NameCache {
    cap: usize,
    map: HashMap<u32, String>,
}

impl NameCache {
    pub fn new(cap: usize) -> Self {
        NameCache { cap, map: HashMap::new() }
    }
    pub fn get(&self, pid: u32) -> Option<String> {
        self.map.get(&pid).cloned()
    }
    pub fn put(&mut self, pid: u32, name: &str) {
        if self.map.len() >= self.cap {
            if let Some(k) = self.map.keys().next().copied() {
                self.map.remove(&k);
            }
        }
        self.map.insert(pid, name.to_string());
    }
}

/// 按 `xig_len`/`xig_count` 游走记录，抽取字段。
pub fn parse_pcblist(buf: &[u8]) -> Vec<PcbRecord> {
    if buf.len() < 16 {
        return Vec::new();
    }
    let header_len = u32::from_ne_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
    let count = u32::from_ne_bytes([buf[4], buf[5], buf[6], buf[7]]) as usize;
    if header_len < 16 || header_len > buf.len() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut pos = header_len;
    for _ in 0..count {
        if pos + 4 > buf.len() {
            break;
        }
        let rec_len =
            u32::from_ne_bytes([buf[pos], buf[pos + 1], buf[pos + 2], buf[pos + 3]]) as usize;
        if rec_len == 0 || pos + rec_len > buf.len() {
            break;
        }
        if let Some(rec) = parse_one_record(&buf[pos..pos + rec_len]) {
            out.push(rec);
        }
        pos += rec_len;
    }
    out
}

fn parse_one_record(rec: &[u8]) -> Option<PcbRecord> {
    if rec.len() < OFF_PID + 4 {
        return None;
    }
    let lport_net = u16::from_ne_bytes(rec[OFF_LPORT..OFF_LPORT + 2].try_into().ok()?);
    let faddr_net = u32::from_ne_bytes(rec[OFF_FADDR..OFF_FADDR + 4].try_into().ok()?);
    let pid = u32::from_ne_bytes(rec[OFF_PID..OFF_PID + 4].try_into().ok()?);
    Some(PcbRecord {
        lport: u16::from_be(lport_net),
        faddr: Ipv4Addr::from(u32::from_be(faddr_net)),
        pid,
    })
}

pub struct SysctlProcLookup {
    names: NameCache,
}

impl SysctlProcLookup {
    pub fn new() -> Self {
        SysctlProcLookup { names: NameCache::new(256) }
    }
}

impl ProcLookup for SysctlProcLookup {
    fn pid_for(&mut self, key: &FlowKey) -> Option<u32> {
        let buf = read_pcblist(key.transport);
        let want_port = key.local.port();
        parse_pcblist(&buf)
            .into_iter()
            .find(|r| r.lport == want_port)
            .map(|r| r.pid)
    }

    fn name_for_pid(&mut self, pid: u32) -> Option<String> {
        if let Some(n) = self.names.get(pid) {
            return Some(n);
        }
        let path = pid_path(pid)?;
        let name = proc_pidpath_to_name(&path)?;
        self.names.put(pid, &name);
        Some(name)
    }
}

fn read_pcblist(transport: Transport) -> Vec<u8> {
    let name = match transport {
        Transport::Tcp => "net.inet.tcp.pcblist_n",
        Transport::Udp => "net.inet.udp.pcblist_n",
    };
    let cname = match CString::new(name) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let mut len: usize = 0;
    let rc = unsafe {
        libc::sysctlbyname(cname.as_ptr(), std::ptr::null_mut(), &mut len, std::ptr::null_mut(), 0)
    };
    if rc != 0 || len == 0 {
        return Vec::new();
    }
    let mut buf = vec![0u8; len];
    let rc = unsafe {
        libc::sysctlbyname(
            cname.as_ptr(),
            buf.as_mut_ptr() as *mut libc::c_void,
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 {
        return Vec::new();
    }
    buf.truncate(len);
    buf
}

fn pid_path(pid: u32) -> Option<String> {
    const PROC_PIDPATHINFO_MAXSIZE: usize = 4096;
    let mut buf = vec![0u8; PROC_PIDPATHINFO_MAXSIZE];
    let rc = unsafe {
        libc::proc_pidpath(
            pid as libc::c_int,
            buf.as_mut_ptr() as *mut libc::c_void,
            buf.len() as u32,
        )
    };
    if rc <= 0 {
        return None;
    }
    let cstr = unsafe { CStr::from_ptr(buf.as_ptr() as *const libc::c_char) };
    Some(cstr.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_name_from_pidpath() {
        assert_eq!(
            proc_pidpath_to_name("/usr/sbin/mDNSResponder\0").as_deref(),
            Some("mDNSResponder")
        );
        assert_eq!(proc_pidpath_to_name(""), None);
    }

    #[test]
    fn cache_round_trips() {
        let mut c = NameCache::new(2);
        c.put(1, "A");
        c.put(2, "B");
        c.put(3, "C"); // 触发淘汰
        assert_eq!(c.get(3).as_deref(), Some("C"));
        assert!(c.get(1).is_none() || c.get(2).is_none());
    }

    fn synth_record(lport: u16, faddr: Ipv4Addr, pid: u32, total_len: usize) -> Vec<u8> {
        let mut rec = vec![0u8; total_len];
        rec[0..4].copy_from_slice(&(total_len as u32).to_ne_bytes());
        rec[OFF_LPORT..OFF_LPORT + 2].copy_from_slice(&lport.to_be_bytes());
        rec[OFF_FADDR..OFF_FADDR + 4].copy_from_slice(&u32::from(faddr).to_be_bytes());
        rec[OFF_PID..OFF_PID + 4].copy_from_slice(&pid.to_ne_bytes());
        rec
    }

    #[test]
    fn walks_pcblist_and_extracts_fields() {
        let rec = synth_record(8080, "127.0.0.1".parse().unwrap(), 4321, 0x70);
        let mut buf = vec![0u8; 16];
        buf[0..4].copy_from_slice(&16u32.to_ne_bytes()); // xig_len = 16
        buf[4..8].copy_from_slice(&1u32.to_ne_bytes()); // xig_count = 1
        buf.extend_from_slice(&rec);
        let recs = parse_pcblist(&buf);
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].lport, 8080);
        assert_eq!(recs[0].faddr, Ipv4Addr::new(127, 0, 0, 1));
        assert_eq!(recs[0].pid, 4321);
    }
}
