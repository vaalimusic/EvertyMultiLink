use std::net::Ipv4Addr;

#[cfg(windows)]
pub fn lan_ipv4() -> Option<Ipv4Addr> {
    use windows_sys::Win32::{
        Foundation::ERROR_BUFFER_OVERFLOW,
        NetworkManagement::{IpHelper::*, Ndis::IfOperStatusUp},
        Networking::WinSock::{AF_INET, SOCKADDR_IN},
    };
    let mut size = 16_384u32;
    for _ in 0..3 {
        // The Win32 linked list contains pointers and needs native alignment.
        let mut storage = vec![0usize; (size as usize).div_ceil(size_of::<usize>())];
        let head = storage.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        let result = unsafe {
            GetAdaptersAddresses(
                AF_INET.into(),
                GAA_FLAG_INCLUDE_GATEWAYS,
                std::ptr::null(),
                head,
                &mut size,
            )
        };
        if result == ERROR_BUFFER_OVERFLOW {
            continue;
        }
        if result != 0 {
            return None;
        }
        let mut next = head;
        let mut candidates = Vec::new();
        while let Some(adapter) = unsafe { next.as_ref() } {
            next = adapter.Next;
            if adapter.OperStatus != IfOperStatusUp
                || !matches!(adapter.IfType, 6 | 71)
                || adapter.PhysicalAddressLength != 6
            {
                continue;
            }
            let mut address = adapter.FirstUnicastAddress;
            while let Some(unicast) = unsafe { address.as_ref() } {
                address = unicast.Next;
                let sockaddr = unicast.Address.lpSockaddr;
                if sockaddr.is_null()
                    || unicast.Address.iSockaddrLength < size_of::<SOCKADDR_IN>() as i32
                    || unsafe { (*sockaddr).sa_family } != AF_INET
                {
                    continue;
                }
                let addr = unsafe { &*sockaddr.cast::<SOCKADDR_IN>() };
                let ip = Ipv4Addr::from(unsafe { addr.sin_addr.S_un.S_addr }.to_ne_bytes());
                if ip.is_private() {
                    candidates.push((
                        adapter.FirstGatewayAddress.is_null(),
                        adapter.Ipv4Metric,
                        ip,
                    ));
                }
            }
        }
        candidates.sort_unstable();
        return candidates.first().map(|entry| entry.2);
    }
    None
}

#[cfg(target_os = "macos")]
pub fn lan_ipv4() -> Option<Ipv4Addr> {
    use std::ffi::CStr;
    let mut head = std::ptr::null_mut();
    if unsafe { libc::getifaddrs(&mut head) } != 0 {
        return None;
    }
    let mut next = head;
    let mut candidates = Vec::new();
    while let Some(interface) = unsafe { next.as_ref() } {
        next = interface.ifa_next;
        if interface.ifa_name.is_null()
            || interface.ifa_addr.is_null()
            || interface.ifa_flags & libc::IFF_UP as u32 == 0
            || unsafe { (*interface.ifa_addr).sa_family as i32 } != libc::AF_INET
        {
            continue;
        }
        let name = unsafe { CStr::from_ptr(interface.ifa_name) }.to_bytes();
        let address = unsafe { &*interface.ifa_addr.cast::<libc::sockaddr_in>() };
        let ip = Ipv4Addr::from(address.sin_addr.s_addr.to_ne_bytes());
        if name.starts_with(b"en") && ip.is_private() {
            candidates.push((name != b"en0", ip));
        }
    }
    unsafe { libc::freeifaddrs(head) };
    candidates.sort_unstable();
    candidates.first().map(|entry| entry.1)
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn lan_ipv4() -> Option<Ipv4Addr> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:9").ok()?;
    match socket.local_addr().ok()?.ip() {
        std::net::IpAddr::V4(ip) if ip.is_private() => Some(ip),
        _ => None,
    }
}
