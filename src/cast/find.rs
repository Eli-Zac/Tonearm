//! Finds Cast devices on the network. They announce themselves over mDNS
//! as `_googlecast._tcp`, with their name and model in the TXT record.
//!
//! On Windows the search goes through the system's own DNS-SD service
//! (`DnsServiceBrowse`), not a socket of ours. Windows Firewall lets that
//! service hear mDNS answers on every network, where a socket of Tonearm's
//! would be blocked, or ask first.

use super::Device;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How often the search starts over, to see devices that came on.
const REFRESH: Duration = Duration::from_secs(15);
/// A device not heard from for this long has gone.
const FORGET: Duration = Duration::from_secs(70);

pub struct Finder {
    devices: Arc<Mutex<Vec<(Device, Instant)>>>,
    /// Dropping it ends the search thread.
    _stop: mpsc::Sender<()>,
}

impl Finder {
    /// Starts searching. `changed` runs on the search thread whenever the
    /// list changes.
    pub fn start(changed: impl Fn() + Send + 'static) -> Finder {
        let devices = Arc::new(Mutex::new(Vec::new()));
        let (stop, stopped) = mpsc::channel::<()>();
        let list = devices.clone();
        std::thread::Builder::new()
            .name("cast-find".into())
            .spawn(move || run(list, stopped, changed))
            .expect("cannot start the cast search");
        Finder {
            devices,
            _stop: stop,
        }
    }

    /// The devices found, by name.
    pub fn devices(&self) -> Vec<Device> {
        let mut found: Vec<Device> = self
            .devices
            .lock()
            .unwrap()
            .iter()
            .map(|(d, _)| d.clone())
            .collect();
        found.sort_by_key(|d| d.name.to_lowercase());
        found
    }
}

fn run(list: Arc<Mutex<Vec<(Device, Instant)>>>, stop: mpsc::Receiver<()>, changed: impl Fn()) {
    loop {
        let (tx, rx) = mpsc::channel::<Device>();
        let search = platform::Search::start(tx);
        if let Err(e) = &search {
            eprintln!("cast: cannot search the network: {e}");
        }
        let until = Instant::now() + REFRESH;
        loop {
            let wait = until.saturating_duration_since(Instant::now());
            let found = match rx.recv_timeout(wait.min(Duration::from_secs(1))) {
                Ok(device) => Some(device),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => None,
            };
            if matches!(stop.try_recv(), Err(mpsc::TryRecvError::Disconnected)) {
                return;
            }
            let mut devices = list.lock().unwrap();
            let before: Vec<Device> = devices.iter().map(|(d, _)| d.clone()).collect();
            if let Some(device) = found {
                devices.retain(|(d, _)| d.id != device.id);
                devices.push((device, Instant::now()));
            }
            devices.retain(|(_, seen)| seen.elapsed() < FORGET);
            let after: Vec<Device> = devices.iter().map(|(d, _)| d.clone()).collect();
            drop(devices);
            let same = before.len() == after.len() && before.iter().all(|d| after.contains(d));
            if !same {
                changed();
            }
            if Instant::now() >= until {
                break;
            }
        }
        drop(search);
    }
}

/// A device from its TXT record, address and port.
fn device(txt: &[(String, String)], ip: std::net::IpAddr, port: u16) -> Option<Device> {
    let get = |key: &str| {
        txt.iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.clone())
    };
    let model = get("md").unwrap_or_default();
    // Bit 5 of the capabilities marks a speaker group.
    let caps: u32 = get("ca").and_then(|c| c.parse().ok()).unwrap_or(0);
    Some(Device {
        id: get("id")?,
        name: get("fn").filter(|n| !n.is_empty()).unwrap_or_else(|| model.clone()),
        group: caps & 32 != 0 || model.eq_ignore_ascii_case("Google Cast Group"),
        // Bit 0 is video out: a TV or a Chromecast in one.
        screen: caps & 1 != 0,
        model,
        addr: std::net::SocketAddr::new(ip, port),
    })
}

#[cfg(windows)]
mod platform {
    use super::Device;
    use std::net::{IpAddr, Ipv4Addr};
    use std::sync::mpsc::Sender;
    use std::sync::Mutex;
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::NetworkManagement::Dns::*;

    const SERVICE: &str = "_googlecast._tcp.local";
    /// What DnsServiceBrowse and DnsServiceResolve answer when they have
    /// started. ERROR_SUCCESS would mean they finished at once.
    const DNS_REQUEST_PENDING: i32 = 9506;

    /// Where the callbacks, which run on a system thread, send what they
    /// find. Only one search runs at a time.
    static SINK: Mutex<Option<Sender<Device>>> = Mutex::new(None);

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    pub struct Search {
        cancel: Box<DNS_SERVICE_CANCEL>,
        _name: Vec<u16>,
    }

    impl Search {
        pub fn start(sink: Sender<Device>) -> Result<Search, String> {
            *SINK.lock().unwrap() = Some(sink);
            let name = wide(SERVICE);
            let mut cancel = Box::<DNS_SERVICE_CANCEL>::default();
            let request = DNS_SERVICE_BROWSE_REQUEST {
                Version: DNS_QUERY_REQUEST_VERSION1.0,
                InterfaceIndex: 0,
                QueryName: PCWSTR(name.as_ptr()),
                Anonymous: DNS_SERVICE_BROWSE_REQUEST_0 {
                    pBrowseCallback: Some(browsed),
                },
                pQueryContext: std::ptr::null_mut(),
            };
            let status = unsafe { DnsServiceBrowse(&request, &mut *cancel) };
            if status != DNS_REQUEST_PENDING {
                return Err(format!("DnsServiceBrowse answered {status}"));
            }
            Ok(Search {
                cancel,
                _name: name,
            })
        }
    }

    impl Drop for Search {
        fn drop(&mut self) {
            unsafe {
                DnsServiceBrowseCancel(&*self.cancel);
            }
        }
    }

    /// One answer to the search: the names of devices, to resolve.
    unsafe extern "system" fn browsed(
        status: u32,
        _context: *const core::ffi::c_void,
        records: *const DNS_RECORDW,
    ) {
        if records.is_null() {
            return;
        }
        if status == 0 {
            let mut record = records;
            while !record.is_null() {
                let r = &*record;
                // A time to live of zero is a device saying goodbye. It is
                // forgotten when it stops answering.
                if r.wType == DNS_TYPE_PTR.0 && r.dwTtl > 0 {
                    if let Ok(name) = r.Data.PTR.pNameHost.to_string() {
                        resolve(&name);
                    }
                }
                record = r.pNext;
            }
        }
        DnsFree(Some(records as *const _), DnsFreeRecordList);
    }

    /// A resolve in flight. Freed by its callback, which always runs once.
    struct Resolving {
        name: Vec<u16>,
        cancel: DNS_SERVICE_CANCEL,
    }

    fn resolve(name: &str) {
        let pending = Box::into_raw(Box::new(Resolving {
            name: wide(name),
            cancel: DNS_SERVICE_CANCEL::default(),
        }));
        unsafe {
            let request = DNS_SERVICE_RESOLVE_REQUEST {
                Version: DNS_QUERY_REQUEST_VERSION1.0,
                InterfaceIndex: 0,
                QueryName: PWSTR((*pending).name.as_mut_ptr()),
                pResolveCompletionCallback: Some(resolved),
                pQueryContext: pending as *mut _,
            };
            if DnsServiceResolve(&request, &mut (*pending).cancel) != DNS_REQUEST_PENDING {
                drop(Box::from_raw(pending));
            }
        }
    }

    unsafe extern "system" fn resolved(
        status: u32,
        context: *const core::ffi::c_void,
        instance: *const DNS_SERVICE_INSTANCE,
    ) {
        drop(Box::from_raw(context as *mut Resolving));
        if instance.is_null() {
            return;
        }
        let i = &*instance;
        if status == 0 && !i.ip4Address.is_null() {
            let mut txt = Vec::new();
            // A key with no value has a null one.
            let text = |p: PWSTR| {
                if p.is_null() {
                    String::new()
                } else {
                    p.to_string().unwrap_or_default()
                }
            };
            for n in 0..i.dwPropertyCount as usize {
                txt.push((text(*i.keys.add(n)), text(*i.values.add(n))));
            }
            // Stored as the address's four bytes in order.
            let ip = IpAddr::V4(Ipv4Addr::from((*i.ip4Address).to_ne_bytes()));
            if let Some(device) = super::device(&txt, ip, i.wPort) {
                if let Some(sink) = SINK.lock().unwrap().as_ref() {
                    let _ = sink.send(device);
                }
            }
        }
        DnsServiceFreeInstance(instance);
    }
}

#[cfg(not(windows))]
mod platform {
    use super::Device;
    use std::sync::mpsc::Sender;

    /// Other systems have their own DNS-SD service to ask (Android's
    /// NsdManager), not written yet.
    pub struct Search;

    impl Search {
        pub fn start(_sink: Sender<Device>) -> Result<Search, String> {
            Err("not written for this system yet".into())
        }
    }
}
