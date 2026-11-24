//! Runs a proxy auto-configuration (PAC) script for one URL with CFNetwork, the engine macOS itself
//! uses, so the answer is the one the system would give for that URL.
use crate::platform::Result;
use std::ffi::c_void;
use std::ptr::null;

type CFTypeRef = *const c_void;

#[derive(Debug, PartialEq)]
pub enum Route {
    Direct,
    /// PAC `PROXY`: an HTTP proxy, tunnelling https with CONNECT.
    Http(String, i64),
    /// PAC `SOCKS`.
    Socks(String, i64),
}

pub const FAILED: &str = "PAC 脚本运行出错";
pub const NO_ROUTE: &str = "PAC 脚本没有给出能用的代理";

// kCFStringEncodingUTF8, kCFNumberSInt64Type.
const UTF8: u32 = 0x0800_0100;
const SINT64: isize = 4;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFStringCreateWithBytes(
        alloc: CFTypeRef,
        bytes: *const u8,
        len: isize,
        encoding: u32,
        external: u8,
    ) -> CFTypeRef;
    fn CFURLCreateWithBytes(
        alloc: CFTypeRef,
        bytes: *const u8,
        len: isize,
        encoding: u32,
        base: CFTypeRef,
    ) -> CFTypeRef;
    fn CFArrayGetCount(array: CFTypeRef) -> isize;
    fn CFArrayGetValueAtIndex(array: CFTypeRef, index: isize) -> CFTypeRef;
    fn CFDictionaryGetValue(dictionary: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
    fn CFEqual(a: CFTypeRef, b: CFTypeRef) -> u8;
    fn CFGetTypeID(value: CFTypeRef) -> usize;
    fn CFStringGetTypeID() -> usize;
    fn CFNumberGetTypeID() -> usize;
    fn CFStringGetCString(string: CFTypeRef, buffer: *mut u8, size: isize, encoding: u32) -> u8;
    fn CFNumberGetValue(number: CFTypeRef, kind: isize, value: *mut c_void) -> u8;
    fn CFRelease(value: CFTypeRef);
}

#[link(name = "CFNetwork", kind = "framework")]
unsafe extern "C" {
    static kCFProxyTypeKey: CFTypeRef;
    static kCFProxyHostNameKey: CFTypeRef;
    static kCFProxyPortNumberKey: CFTypeRef;
    static kCFProxyTypeNone: CFTypeRef;
    static kCFProxyTypeHTTP: CFTypeRef;
    static kCFProxyTypeHTTPS: CFTypeRef;
    static kCFProxyTypeSOCKS: CFTypeRef;
    fn CFNetworkCopyProxiesForAutoConfigurationScript(
        script: CFTypeRef,
        url: CFTypeRef,
        error: *mut CFTypeRef,
    ) -> CFTypeRef;
}

/// A Core Foundation object this code created or copied, released on drop.
struct Owned(CFTypeRef);
impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) }
        }
    }
}

/// The first route the script gives for `url`. A fallback such as `PROXY a; DIRECT` is not
/// tried: curl cannot fail over, and going direct when the proxy is down is not ours to guess.
pub fn first_route(script: &str, url: &str) -> Result<Route> {
    unsafe {
        let script = Owned(CFStringCreateWithBytes(
            null(),
            script.as_ptr(),
            script.len() as isize,
            UTF8,
            0,
        ));
        let url = Owned(CFURLCreateWithBytes(
            null(),
            url.as_ptr(),
            url.len() as isize,
            UTF8,
            null(),
        ));
        if script.0.is_null() || url.0.is_null() {
            return Err(FAILED);
        }
        let mut error = null();
        let list = Owned(CFNetworkCopyProxiesForAutoConfigurationScript(
            script.0, url.0, &mut error,
        ));
        drop(Owned(error));
        if list.0.is_null() {
            return Err(FAILED);
        }
        // CFNetwork drops entries macOS cannot use (`SOCKS5`, `HTTPS`, unparseable ones).
        if CFArrayGetCount(list.0) == 0 {
            return Err(NO_ROUTE);
        }
        let entry = CFArrayGetValueAtIndex(list.0, 0);
        let kind = CFDictionaryGetValue(entry, kCFProxyTypeKey);
        if kind.is_null() {
            return Err(NO_ROUTE);
        }
        if CFEqual(kind, kCFProxyTypeNone) != 0 {
            return Ok(Route::Direct);
        }
        let host = string(CFDictionaryGetValue(entry, kCFProxyHostNameKey)).ok_or(NO_ROUTE)?;
        let port = number(CFDictionaryGetValue(entry, kCFProxyPortNumberKey)).ok_or(NO_ROUTE)?;
        if CFEqual(kind, kCFProxyTypeHTTP) != 0 || CFEqual(kind, kCFProxyTypeHTTPS) != 0 {
            Ok(Route::Http(host, port))
        } else if CFEqual(kind, kCFProxyTypeSOCKS) != 0 {
            Ok(Route::Socks(host, port))
        } else {
            Err(NO_ROUTE)
        }
    }
}

unsafe fn string(value: CFTypeRef) -> Option<String> {
    if value.is_null() || unsafe { CFGetTypeID(value) != CFStringGetTypeID() } {
        return None;
    }
    // Host names are at most 253 bytes.
    let mut buffer = [0u8; 256];
    if unsafe { CFStringGetCString(value, buffer.as_mut_ptr(), 256, UTF8) } == 0 {
        return None;
    }
    let end = buffer.iter().position(|b| *b == 0)?;
    String::from_utf8(buffer[..end].to_vec()).ok()
}

unsafe fn number(value: CFTypeRef) -> Option<i64> {
    if value.is_null() || unsafe { CFGetTypeID(value) != CFNumberGetTypeID() } {
        return None;
    }
    let mut out = 0i64;
    let ok = unsafe { CFNumberGetValue(value, SINT64, (&raw mut out).cast()) };
    (ok != 0).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    const API: &str = "https://api.example.com/v1/usage";

    fn run(body: &str, url: &str) -> Result<Route> {
        first_route(
            &format!("function FindProxyForURL(url, host) {{ {body} }}"),
            url,
        )
    }

    #[test]
    fn routes_follow_the_script_for_each_url() {
        assert_eq!(run("return \"DIRECT\";", API), Ok(Route::Direct));
        let proxy = Route::Http("127.0.0.1".into(), 7890);
        assert_eq!(
            run("return \"PROXY 127.0.0.1:7890; DIRECT\";", API),
            Ok(proxy)
        );
        let proxy = Route::Http("127.0.0.1".into(), 7890);
        assert_eq!(
            run("return \"PROXY 127.0.0.1:7890\";", "http://example.com/"),
            Ok(proxy)
        );
        let socks = Route::Socks("127.0.0.1".into(), 7891);
        assert_eq!(run("return \"SOCKS 127.0.0.1:7891\";", API), Ok(socks));
        // macOS ignores SOCKS5 and HTTPS entries; the next usable one is what it would use.
        let socks = Route::Socks("127.0.0.1".into(), 7891);
        assert_eq!(
            run(
                "return \"SOCKS5 127.0.0.1:7891; SOCKS 127.0.0.1:7891\";",
                API
            ),
            Ok(socks)
        );
        // PAC helpers such as dnsDomainIs are available, and the host is the target's.
        let split =
            "return dnsDomainIs(host, \".example.org\") ? \"PROXY proxy.local:3128\" : \"DIRECT\";";
        assert_eq!(run(split, API), Ok(Route::Direct));
        assert_eq!(
            run(split, "https://www.example.org/"),
            Ok(Route::Http("proxy.local".into(), 3128))
        );
    }

    #[test]
    fn scripts_without_a_usable_route_are_errors_not_direct() {
        for body in [
            "return \"\";",
            "return \"SOCKS5 127.0.0.1:7891\";",
            "return \"HTTPS 127.0.0.1:7890\";",
        ] {
            assert_eq!(run(body, API), Err(NO_ROUTE), "{body}");
        }
        assert_eq!(run("throw new Error(\"x\");", API), Err(FAILED));
        assert_eq!(first_route("syntax error(((", API), Err(FAILED));
    }

    #[test]
    fn scripts_run_on_several_threads_at_once() {
        let threads: Vec<_> = (0..4)
            .map(|_| std::thread::spawn(|| run("return \"PROXY 127.0.0.1:7890\";", API)))
            .collect();
        for thread in threads {
            assert_eq!(
                thread.join().unwrap(),
                Ok(Route::Http("127.0.0.1".into(), 7890))
            );
        }
    }
}
