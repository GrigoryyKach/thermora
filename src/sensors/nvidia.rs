//! NVIDIA through NVML (official and documented) plus the NvAPI thermal sensors (undocumented, Windows only).
//! Both are loaded dynamically: when the library is missing every reading is "not reported", never a fake 0.

use crate::model::{GpuLive, Rd, Why};
use crate::specs::{Component, Field, Group};
use nvml_wrapper::enum_wrappers::device::{Clock, TemperatureSensor};
use nvml_wrapper::Nvml;

pub struct Nvidia {
    nvml: Nvml,
    #[cfg(windows)]
    nvapi: Option<nvapi::NvApi>,
}

fn rd<T, E>(r: Result<T, E>) -> Rd<T> {
    r.map_err(|_| Why::NotReported)
}

impl Nvidia {
    /// `None` when there is no NVIDIA driver (the library cannot be loaded).
    pub fn new() -> Option<Self> {
        let nvml = Nvml::init().ok()?;
        if nvml.device_count().unwrap_or(0) == 0 {
            return None;
        }
        Some(Nvidia {
            nvml,
            #[cfg(windows)]
            nvapi: nvapi::NvApi::load(),
        })
    }

    pub fn live(&self) -> Option<GpuLive> {
        let d = self.nvml.device_by_index(0).ok()?;
        let mem = d.memory_info().ok();
        #[cfg_attr(not(windows), allow(unused_mut))]
        let mut g = GpuLive {
            name: d.name().unwrap_or_default().trim_start_matches("NVIDIA ").to_string(),
            load: d.utilization_rates().map(|u| u.gpu as f32).unwrap_or(0.0),
            temp: Some(rd(d.temperature(TemperatureSensor::Gpu)).map(|t| t as f32)),
            hotspot: Some(Err(Why::NotReported)),
            mem_temp: Some(Err(Why::NotReported)),
            vram_used: mem.as_ref().map(|m| m.used).unwrap_or(0),
            vram_total: mem.as_ref().map(|m| m.total).unwrap_or(0),
            clock_mhz: Some(rd(d.clock_info(Clock::Graphics)).map(|c| c as f32)),
            power: Some(rd(d.power_usage()).map(|p| p as f32 / 1000.0)),
            power_limit: Some(rd(d.enforced_power_limit()).map(|p| p as f32 / 1000.0)),
            fan: Some(rd(d.fan_speed(0)).map(|f| f as f32)),
        };
        #[cfg(windows)]
        if let Some(api) = &self.nvapi {
            if let Ok(t) = api.thermal_sensors() {
                let hot = t[1] as f32 / 256.0;
                if hot > 1.0 {
                    g.hotspot = Some(Ok(hot));
                }
                // Memory junction: index 7 on RTX 40, 9 on older cards (LibreHardwareMonitor NvidiaGpu.cs).
                // Zero means the board does not report it right now.
                let mem = t[7] as f32 / 256.0;
                if mem > 1.0 {
                    g.mem_temp = Some(Ok(mem));
                }
            }
        }
        Some(g)
    }

    pub fn specs(&self) -> Vec<Component> {
        let mut out = Vec::new();
        let driver = self.nvml.sys_driver_version().unwrap_or_default();
        for i in 0..self.nvml.device_count().unwrap_or(0) {
            // Never calls uuid() or serial(): privacy rule.
            let Ok(d) = self.nvml.device_by_index(i) else { continue };
            let name = d.name().unwrap_or_default();
            let vram = d.memory_info().map(|m| crate::model::fmt_bytes(m.total)).unwrap_or_default();
            let mhz = |r: Result<u32, _>| r.map(|c: u32| format!("{c} MHz")).unwrap_or_default();
            let limit = match (d.enforced_power_limit(), d.power_management_limit_default()) {
                (Ok(a), _) => format!("{} W", a / 1000),
                (_, Ok(b)) => format!("{} W", b / 1000),
                _ => String::new(),
            };
            let pcie = match (d.current_pcie_link_gen(), d.current_pcie_link_width(), d.max_pcie_link_gen(), d.max_pcie_link_width()) {
                (Ok(g), Ok(w), Ok(mg), Ok(mw)) => format!("Gen{g} x{w} (max Gen{mg} x{mw})"),
                _ => String::new(),
            };
            out.push(
                Component::new(Group::Gpu, name.clone())
                    .sub(vram.clone())
                    .row(Field::Manufacturer, "NVIDIA")
                    .row(Field::Model, name.trim_start_matches("NVIDIA ").to_string())
                    .row(Field::Vram, vram)
                    .row(Field::Driver, driver.clone())
                    .row(Field::MaxCoreClock, mhz(d.max_clock_info(Clock::Graphics)))
                    .row(Field::MaxMemClock, mhz(d.max_clock_info(Clock::Memory)))
                    .row(Field::PowerLimit, limit)
                    .row(Field::PcieLink, pcie),
            );
        }
        out
    }
}

#[cfg(windows)]
mod nvapi {
    //! NvAPI is only reachable through `nvapi_QueryInterface(id)`; the ids and the sensor struct below are not in
    //! NVIDIA's public headers. Source of the ids and the layout: LibreHardwareMonitor `Interop/NvApi.cs`
    //! (MPL-2.0; read only, nothing copied). Fails safe: any non-zero status means the row shows a dash. It is only
    //! ever called with the exact struct version below and nothing is ever written through NvAPI.
    use libloading::Library;
    use std::ffi::c_void;

    pub struct NvApi {
        _lib: Library,
        gpu: *mut c_void,
        get_sensors: unsafe extern "C" fn(*mut c_void, *mut Sensors) -> i32,
        mask: u32,
    }

    // SAFETY: the handle and function pointers are plain values; the struct is only used by the collector thread.
    unsafe impl Send for NvApi {}

    #[repr(C)]
    struct Sensors {
        version: u32,
        mask: u32,
        reserved: [i32; 8],
        temps: [i32; 32],
    }

    type Query = unsafe extern "C" fn(u32) -> *const c_void;

    impl NvApi {
        pub fn load() -> Option<NvApi> {
            // SAFETY: the interface ids are the ones LibreHardwareMonitor uses; function pointers are only
            // transmuted to the signatures documented there, and all calls check the returned status.
            unsafe {
                let lib = Library::new("nvapi64.dll").ok()?;
                let q: libloading::Symbol<Query> = lib.get(b"nvapi_QueryInterface\0").ok()?;
                let (init, enum_gpus, sensors) = (q(0x0150E828), q(0xE5AC921F), q(0x65FE3AAD));
                if init.is_null() || enum_gpus.is_null() || sensors.is_null() {
                    return None;
                }
                let init: unsafe extern "C" fn() -> i32 = std::mem::transmute(init);
                if init() != 0 {
                    return None;
                }
                let enum_gpus: unsafe extern "C" fn(*mut *mut c_void, *mut i32) -> i32 = std::mem::transmute(enum_gpus);
                let mut handles = [std::ptr::null_mut::<c_void>(); 64];
                let mut n = 0i32;
                if enum_gpus(handles.as_mut_ptr(), &mut n) != 0 || n < 1 {
                    return None;
                }
                let get_sensors: unsafe extern "C" fn(*mut c_void, *mut Sensors) -> i32 = std::mem::transmute(sensors);
                let mut api = NvApi { _lib: lib, gpu: handles[0], get_sensors, mask: 0 };
                // Same probing as LibreHardwareMonitor: grow the mask bit by bit until the call fails.
                let mut s = Sensors { version: (std::mem::size_of::<Sensors>() as u32) | (2 << 16), mask: 0, reserved: [0; 8], temps: [0; 32] };
                for bit in 0..32 {
                    s.mask = 1u32 << bit;
                    if (api.get_sensors)(api.gpu, &mut s) != 0 {
                        break;
                    }
                    api.mask = (1u32 << bit) | ((1u32 << bit) - 1);
                }
                (api.mask != 0).then_some(api)
            }
        }

        pub fn thermal_sensors(&self) -> Result<[i32; 32], i32> {
            let mut s = Sensors { version: (std::mem::size_of::<Sensors>() as u32) | (2 << 16), mask: self.mask, reserved: [0; 8], temps: [0; 32] };
            // SAFETY: valid handle and a struct of the version we pass.
            let st = unsafe { (self.get_sensors)(self.gpu, &mut s) };
            if st == 0 { Ok(s.temps) } else { Err(st) }
        }
    }
}
