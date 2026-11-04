//! 捕获用 TUN 设备。
//!
//! - macOS：通过 PF_SYSTEM 控制 socket 打开一个 utun，读写裸 IP 包
//!   （内核在包前带 4 字节地址族头，这里负责加/剥）。
//! - Windows：调用安装包内置的 `wintun.dll` 建一个独立的 Wintun 网卡，
//!   与 openconnect 自己那张隧道网卡互不干扰。
//!
//! 两个实现都只暴露 [`TunDevice`] 的三个方法，`netstack` 不感知平台差异。

use std::io;

use super::platform::TunDevice;

/// 建一张捕获网卡。
///
/// `connector_path` 是内置 openconnect.exe 的路径，Windows 从它所在目录加载
/// `wintun.dll`；macOS 忽略这个参数。
pub fn create_capture(connector_path: &str) -> io::Result<Box<dyn TunDevice>> {
    #[cfg(target_os = "macos")]
    {
        let _ = connector_path;
        Ok(Box::new(mac::Utun::create()?))
    }
    #[cfg(windows)]
    {
        Ok(Box::new(win::WintunDevice::create(connector_path)?))
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = connector_path;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "当前平台没有捕获网卡实现",
        ))
    }
}

// ============================== macOS ==============================

#[cfg(target_os = "macos")]
mod mac {
    use std::ffi::CString;
    use std::io;
    use std::net::Ipv4Addr;
    use std::os::unix::io::RawFd;

    use super::TunDevice;

    pub const UTUN_CONTROL_NAME: &str = "com.apple.net.utun_control";
    const UTUN_OPT_IFNAME: libc::c_int = 2;

    pub fn unit_from_name(name: &str) -> Option<u32> {
        name.strip_prefix("utun")
            .and_then(|n| n.parse::<u32>().ok())
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
            let fd =
                unsafe { libc::socket(libc::AF_SYSTEM, libc::SOCK_DGRAM, libc::SYSPROTO_CONTROL) };
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
            let cstr =
                unsafe { std::ffi::CStr::from_ptr(name_buf.as_ptr() as *const libc::c_char) };
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
            let n =
                unsafe { libc::read(self.fd, tmp.as_mut_ptr() as *mut libc::c_void, tmp.len()) };
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
            let n =
                unsafe { libc::write(self.fd, tmp.as_ptr() as *const libc::c_void, tmp.len()) };
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
            let addr = pickup_sockaddr("10.8.0.9").unwrap();
            assert_eq!(addr.0, "10.8.0.9");
            assert_eq!(addr.1, 2);
        }
    }
}

#[cfg(target_os = "macos")]
pub use mac::{pickup_sockaddr, unit_from_name, Utun, UTUN_CONTROL_NAME};

// ============================== Windows ==============================

#[cfg(windows)]
mod win {
    use std::ffi::c_void;
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};

    use super::TunDevice;

    type Handle = isize;

    const ERROR_NO_MORE_ITEMS: u32 = 259;
    const WINTUN_CAPACITY: u32 = 0x40_0000;

    #[link(name = "kernel32")]
    extern "system" {
        fn LoadLibraryW(name: *const u16) -> Handle;
        fn GetProcAddress(module: Handle, name: *const u8) -> *const c_void;
        fn FreeLibrary(module: Handle) -> i32;
        fn GetLastError() -> u32;
    }

    #[link(name = "iphlpapi")]
    extern "system" {
        /// NET_LUID 是 64 位值；返回 0 表示成功。
        fn ConvertInterfaceLuidToIndex(luid: *const u64, index: *mut u32) -> u32;
    }

    type CreateAdapter =
        unsafe extern "system" fn(*const u16, *const u16, *const c_void) -> Handle;
    type CloseAdapter = unsafe extern "system" fn(Handle);
    type GetAdapterLuid = unsafe extern "system" fn(Handle, *mut u64);
    type StartSession = unsafe extern "system" fn(Handle, u32) -> Handle;
    type EndSession = unsafe extern "system" fn(Handle);
    type ReceivePacket = unsafe extern "system" fn(Handle, *mut u32) -> *mut u8;
    type ReleaseReceivePacket = unsafe extern "system" fn(Handle, *const u8);
    type AllocateSendPacket = unsafe extern "system" fn(Handle, u32) -> *mut u8;
    type SendPacket = unsafe extern "system" fn(Handle, *const u8);

    struct Api {
        module: Handle,
        create: CreateAdapter,
        close: CloseAdapter,
        get_luid: GetAdapterLuid,
        start: StartSession,
        end: EndSession,
        receive: ReceivePacket,
        release: ReleaseReceivePacket,
        allocate: AllocateSendPacket,
        send: SendPacket,
    }

    unsafe fn symbol<T>(module: Handle, name: &[u8]) -> io::Result<T> {
        let ptr = GetProcAddress(module, name.as_ptr());
        if ptr.is_null() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "wintun.dll 缺少导出函数 {}",
                    String::from_utf8_lossy(&name[..name.len().saturating_sub(1)])
                ),
            ));
        }
        Ok(std::mem::transmute_copy(&ptr))
    }

    fn wide(text: &str) -> Vec<u16> {
        std::ffi::OsStr::new(text)
            .encode_wide()
            .chain(Some(0))
            .collect()
    }

    /// 从内置 openconnect 目录加载 wintun.dll 并绑定全部入口。
    fn load_api(connector_path: &str) -> io::Result<Api> {
        let connector = PathBuf::from(connector_path);
        let dir = connector.parent().unwrap_or_else(|| Path::new("."));
        let dll = dir.join("wintun.dll");
        if !dll.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("找不到内置 wintun.dll: {}", dll.display()),
            ));
        }
        let module = unsafe { LoadLibraryW(wide(&dll.to_string_lossy()).as_ptr()) };
        if module == 0 {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!(
                    "加载 {} 失败（Win32 错误 {}）",
                    dll.display(),
                    unsafe { GetLastError() }
                ),
            ));
        }
        let build = || -> io::Result<Api> {
            unsafe {
                Ok(Api {
                    module,
                    create: symbol(module, b"WintunCreateAdapter\0")?,
                    close: symbol(module, b"WintunCloseAdapter\0")?,
                    get_luid: symbol(module, b"WintunGetAdapterLUID\0")?,
                    start: symbol(module, b"WintunStartSession\0")?,
                    end: symbol(module, b"WintunEndSession\0")?,
                    receive: symbol(module, b"WintunReceivePacket\0")?,
                    release: symbol(module, b"WintunReleaseReceivePacket\0")?,
                    allocate: symbol(module, b"WintunAllocateSendPacket\0")?,
                    send: symbol(module, b"WintunSendPacket\0")?,
                })
            }
        };
        match build() {
            Ok(api) => Ok(api),
            Err(e) => {
                unsafe { FreeLibrary(module) };
                Err(e)
            }
        }
    }

    pub struct WintunDevice {
        api: Api,
        adapter: Handle,
        session: Handle,
        name: String,
        index: u32,
        closed: bool,
    }

    // Wintun 句柄只在本线程/本进程内使用；worker 是单线程轮询。
    unsafe impl Send for WintunDevice {}

    impl WintunDevice {
        pub fn create(connector_path: &str) -> io::Result<WintunDevice> {
            let api = load_api(connector_path)?;
            let name = format!("WSWB-Capture-{}", std::process::id());
            let name_w = wide(&name);
            let tunnel_w = wide("WSWorkbench");

            let adapter =
                unsafe { (api.create)(name_w.as_ptr(), tunnel_w.as_ptr(), std::ptr::null()) };
            if adapter == 0 {
                let code = unsafe { GetLastError() };
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    format!(
                        "创建捕获网卡失败（Win32 错误 {}）。需要管理员权限，\
                         并确认 wintun.dll 未被安全软件拦截",
                        code
                    ),
                ));
            }

            let mut luid: u64 = 0;
            unsafe { (api.get_luid)(adapter, &mut luid) };
            let mut index = 0u32;
            let rc = unsafe { ConvertInterfaceLuidToIndex(&luid, &mut index) };
            if rc != 0 || index == 0 {
                unsafe { (api.close)(adapter) };
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    format!("取捕获网卡接口索引失败（错误 {}）", rc),
                ));
            }

            let session = unsafe { (api.start)(adapter, WINTUN_CAPACITY) };
            if session == 0 {
                let code = unsafe { GetLastError() };
                unsafe { (api.close)(adapter) };
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    format!("启动 Wintun 会话失败（Win32 错误 {}）", code),
                ));
            }

            Ok(WintunDevice {
                api,
                adapter,
                session,
                name,
                index,
                closed: false,
            })
        }

        fn close_handles(&mut self) {
            if self.closed {
                return;
            }
            self.closed = true;
            unsafe {
                (self.api.end)(self.session);
                (self.api.close)(self.adapter);
                FreeLibrary(self.api.module);
            }
        }
    }

    impl TunDevice for WintunDevice {
        fn name(&self) -> &str {
            &self.name
        }

        fn index(&self) -> u32 {
            self.index
        }

        fn read_packet(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let mut size = 0u32;
            let packet = unsafe { (self.api.receive)(self.session, &mut size) };
            if packet.is_null() {
                let code = unsafe { GetLastError() };
                if code == ERROR_NO_MORE_ITEMS {
                    return Ok(0);
                }
                return Err(io::Error::from_raw_os_error(code as i32));
            }
            let len = (size as usize).min(buf.len());
            unsafe {
                std::ptr::copy_nonoverlapping(packet, buf.as_mut_ptr(), len);
                (self.api.release)(self.session, packet);
            }
            Ok(len)
        }

        fn write_packet(&mut self, buf: &[u8]) -> io::Result<usize> {
            if buf.is_empty() {
                return Ok(0);
            }
            let packet = unsafe { (self.api.allocate)(self.session, buf.len() as u32) };
            if packet.is_null() {
                // 发送环满：交给上层下一轮重试，不当作错误
                return Ok(0);
            }
            unsafe {
                std::ptr::copy_nonoverlapping(buf.as_ptr(), packet, buf.len());
                (self.api.send)(self.session, packet);
            }
            Ok(buf.len())
        }
    }

    impl Drop for WintunDevice {
        fn drop(&mut self) {
            self.close_handles();
        }
    }
}

#[cfg(windows)]
pub use win::WintunDevice;
