#![cfg(target_os = "macos")]
//! macOS utun 设备：通过 PF_SYSTEM 控制 socket 打开一个 utun，
//! 读写都是裸 IP 包（内核在包前带 4 字节地址族头，这里负责加/剥）。

use std::ffi::CString;
use std::io;
use std::net::Ipv4Addr;
use std::os::unix::io::RawFd;

use super::platform::TunDevice;

pub const UTUN_CONTROL_NAME: &str = "com.apple.net.utun_control";
const UTUN_OPT_IFNAME: libc::c_int = 2;

pub fn unit_from_name(name: &str) -> Option<u32> {
    name.strip_prefix("utun").and_then(|n| n.parse::<u32>().ok())
}

/// 返回 (规范化地址字符串, 地址族低字节)。后者仅为自测提供稳定断言。
pub fn pickup_sockaddr(ip: &str) -> Option<(String, u8)> {
    let parsed: Ipv4Addr = ip.parse().ok()?;
    Some((parsed.to_string(), (libc::AF_INET & 0xff) as u8))
}

/// 把 fd 设为非阻塞。失败返回错误，由调用方负责关掉 fd。
fn set_nonblocking(fd: RawFd) -> io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub struct Utun {
    fd: RawFd,
    name: String,
    index: u32,
}

impl Utun {
    /// 打开一个系统自动分配的 utun。
    pub fn create() -> io::Result<Utun> {
        let fd = unsafe { libc::socket(libc::AF_SYSTEM, libc::SOCK_DGRAM, libc::SYSPROTO_CONTROL) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // 必须设为非阻塞：否则读空会一直阻塞，卡死 netstack 的 poll 主循环。
        if let Err(e) = set_nonblocking(fd) {
            unsafe { libc::close(fd) };
            return Err(e);
        }

        let mut info: libc::ctl_info = unsafe { std::mem::zeroed() };
        let ctl_name = CString::new(UTUN_CONTROL_NAME).unwrap();
        // libc 里 ctl_name 是定长数组而非指针，按字节拷入（含结尾 NUL）。
        let bytes = ctl_name.as_bytes_with_nul();
        let n = bytes.len().min(info.ctl_name.len());
        for (dst, src) in info.ctl_name.iter_mut().zip(&bytes[..n]) {
            *dst = *src as libc::c_char;
        }
        let rc = unsafe { libc::ioctl(fd, libc::CTLIOCGINFO, &mut info as *mut libc::ctl_info) };
        if rc < 0 {
            unsafe { libc::close(fd) };
            return Err(io::Error::last_os_error());
        }

        let mut addr: libc::sockaddr_ctl = unsafe { std::mem::zeroed() };
        addr.sc_len = std::mem::size_of::<libc::sockaddr_ctl>() as u8;
        addr.sc_family = libc::AF_SYSTEM as u8;
        addr.ss_sysaddr = libc::AF_SYS_CONTROL as u16;
        addr.sc_id = info.ctl_id;
        addr.sc_unit = 0; // 让内核分配

        let rc = unsafe {
            libc::connect(
                fd,
                &addr as *const libc::sockaddr_ctl as *const libc::sockaddr,
                std::mem::size_of::<libc::sockaddr_ctl>() as libc::socklen_t,
            )
        };
        if rc < 0 {
            unsafe { libc::close(fd) };
            return Err(io::Error::last_os_error());
        }

        // 取回内核分配的接口名 utunN
        let mut name_buf = [0u8; 64];
        let mut len: libc::socklen_t = name_buf.len() as libc::socklen_t;
        let rc = unsafe {
            libc::getsockopt(
                fd,
                libc::SYSPROTO_CONTROL,
                UTUN_OPT_IFNAME,
                name_buf.as_mut_ptr() as *mut libc::c_void,
                &mut len,
            )
        };
        if rc < 0 {
            unsafe { libc::close(fd) };
            return Err(io::Error::last_os_error());
        }
        let cstr = unsafe { std::ffi::CStr::from_ptr(name_buf.as_ptr() as *const libc::c_char) };
        let name = cstr.to_string_lossy().to_string();
        // utun 的「单元号」不等于接口索引，必须用内核给的索引。
        let index = unsafe { libc::if_nametoindex(cstr.as_ptr()) };
        if index == 0 {
            unsafe { libc::close(fd) };
            return Err(io::Error::last_os_error());
        }

        Ok(Utun { fd, name, index })
    }
}

impl TunDevice for Utun {
    fn name(&self) -> &str {
        &self.name
    }

    fn index(&self) -> u32 {
        self.index
    }

    fn read_packet(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        // utun 读出的是 4 字节地址族头 + IP 包，这里剥掉头
        let mut tmp = vec![0u8; buf.len() + 4];
        let n = unsafe { libc::read(self.fd, tmp.as_mut_ptr() as *mut libc::c_void, tmp.len()) };
        if n < 0 {
            let err = io::Error::last_os_error();
            // 非阻塞下无数据可读：返回 0 表示「暂无数据」，而非错误
            if err.kind() == io::ErrorKind::WouldBlock {
                return Ok(0);
            }
            return Err(err);
        }
        let n = n as usize;
        if n <= 4 {
            return Ok(0);
        }
        let payload = &tmp[4..n];
        buf[..payload.len()].copy_from_slice(payload);
        Ok(payload.len())
    }

    fn write_packet(&mut self, buf: &[u8]) -> io::Result<usize> {
        // 写出去要补 4 字节地址族头：AF_INET 大端放在最后一位
        let mut tmp = Vec::with_capacity(buf.len() + 4);
        tmp.extend_from_slice(&[0, 0, 0, libc::AF_INET as u8]);
        tmp.extend_from_slice(buf);
        let n = unsafe { libc::write(self.fd, tmp.as_ptr() as *const libc::c_void, tmp.len()) };
        if n < 0 {
            let err = io::Error::last_os_error();
            // 非阻塞下发送缓冲满：返回 0，由上层稍后重试
            if err.kind() == io::ErrorKind::WouldBlock {
                return Ok(0);
            }
            return Err(err);
        }
        Ok((n as usize).saturating_sub(4))
    }
}

impl Drop for Utun {
    fn drop(&mut self) {
        unsafe { libc::close(self.fd) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utun_name_maps_to_unit() {
        assert_eq!(unit_from_name("utun0"), Some(0));
        assert_eq!(unit_from_name("utun7"), Some(7));
        assert_eq!(unit_from_name("en0"), None);
    }

    #[test]
    fn control_socket_path_is_utun_control() {
        assert_eq!(UTUN_CONTROL_NAME, "com.apple.net.utun_control");
    }

    #[test]
    fn pickup_address_is_slash32() {
        let addr = pickup_sockaddr("10.10.0.9").unwrap();
        assert_eq!(addr.0, "10.10.0.9");
        assert_eq!(addr.1, 2);
    }
}
