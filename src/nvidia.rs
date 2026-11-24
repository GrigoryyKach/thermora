//! NVIDIA through NVML (official, documented) plus NvAPI thermal sensors (undocumented, Windows only).
//! Both are loaded dynamically: when the DLL/so is missing every reading is simply "not available".

use crate::line;
use nvml_wrapper::enum_wrappers::device::{Clock, TemperatureSensor, TemperatureThreshold};
use nvml_wrapper::Nvml;

pub struct Nvidia {
    nvml: Result<Nvml, String>,
    last_energy: std::cell::Cell<Option<(u64, std::time::Instant)>>,
    #[cfg(windows)]
    nvapi: Option<nvapi::NvApi>,
}

impl Nvidia {
    pub fn new() -> Self {
        let nvml = Nvml::init().map_err(|e| format!("NVML not loadable: {e}"));
        #[cfg(windows)]
        let nvapi = nvapi::NvApi::load();
        Nvidia {
            nvml,
            last_energy: std::cell::Cell::new(None),
            #[cfg(windows)]
            nvapi,
        }
    }

    pub fn specs(&self) {
        let nvml = match &self.nvml {
            Ok(n) => n,
            Err(e) => return line("GPU", "NVIDIA", Err(e.clone())),
        };
        line("GPU", "driver", nvml.sys_driver_version().map_err(|e| e.to_string()));
        let n = nvml.device_count().unwrap_or(0);
        for i in 0..n {
            let Ok(d) = nvml.device_by_index(i) else { continue };
            line("GPU", &format!("#{i} name"), d.name().map_err(|e| e.to_string()));
            // Never call uuid()/serial(): privacy rule.
            if let Ok(m) = d.memory_info() {
                line("GPU", "VRAM total", Ok(format!("{} MiB", m.total / 1048576)));
            }
            line(
                "GPU",
                "max clocks (core/mem)",
                match (d.max_clock_info(Clock::Graphics), d.max_clock_info(Clock::Memory)) {
                    (Ok(a), Ok(b)) => Ok(format!("{a} / {b} MHz")),
                    _ => Err("not reported".into()),
                },
            );
            line(
                "GPU",
                "power limit (enforced/default)",
                match (d.enforced_power_limit(), d.power_management_limit_default()) {
                    (Ok(a), Ok(b)) => Ok(format!("{} / {} W", a / 1000, b / 1000)),
                    _ => Err("not reported".into()),
                },
            );
            line(
                "GPU",
                "PCIe link now (gen x width)",
                match (d.current_pcie_link_gen(), d.current_pcie_link_width()) {
                    (Ok(g), Ok(w)) => Ok(format!("Gen{g} x{w}")),
                    _ => Err("not reported".into()),
                },
            );
            line(
                "GPU",
                "PCIe link max (gen x width)",
                match (d.max_pcie_link_gen(), d.max_pcie_link_width()) {
                    (Ok(g), Ok(w)) => Ok(format!("Gen{g} x{w}")),
                    _ => Err("not reported".into()),
                },
            );
            line(
                "GPU",
                "temp thresholds slowdown/shutdown",
                match (
                    d.temperature_threshold(TemperatureThreshold::Slowdown),
                    d.temperature_threshold(TemperatureThreshold::Shutdown),
                ) {
                    (Ok(a), Ok(b)) => Ok(format!("{a} / {b} C")),
                    _ => Err("not reported".into()),
                },
            );
        }
    }

    pub fn sample(&self) {
        let Ok(nvml) = &self.nvml else {
            return line("GPU", "NVIDIA", Err("NVML not loadable".into()));
        };
        let Ok(d) = nvml.device_by_index(0) else {
            return line("GPU", "NVIDIA", Err("no device".into()));
        };
        line("GPU", "load", d.utilization_rates().map(|u| format!("{}% (mem ctrl {}%)", u.gpu, u.memory)).map_err(|e| e.to_string()));
        line("GPU", "temperature (core, NVML)", d.temperature(TemperatureSensor::Gpu).map(|t| format!("{t} C")).map_err(|e| e.to_string()));
        line(
            "GPU",
            "VRAM used/total",
            d.memory_info().map(|m| format!("{} / {} MiB", m.used / 1048576, m.total / 1048576)).map_err(|e| e.to_string()),
        );
        line("GPU", "core clock", d.clock_info(Clock::Graphics).map(|c| format!("{c} MHz")).map_err(|e| e.to_string()));
        line("GPU", "memory clock", d.clock_info(Clock::Memory).map(|c| format!("{c} MHz")).map_err(|e| e.to_string()));
        line("GPU", "power draw", d.power_usage().map(|p| format!("{:.1} W", p as f64 / 1000.0)).map_err(|e| e.to_string()));
        // Fallback when instantaneous power is unsupported: energy counter (mJ) delta over time.
        if let Ok(mj) = d.total_energy_consumption() {
            let now = std::time::Instant::now();
            if let Some((m0, t0)) = self.last_energy.take() {
                let w = (mj - m0) as f64 / 1000.0 / now.duration_since(t0).as_secs_f64();
                line("GPU", "power (energy counter delta)", Ok(format!("{w:.1} W")));
            }
            self.last_energy.set(Some((mj, now)));
        } else {
            line("GPU", "power (energy counter)", Err("not supported".into()));
        }
        line("GPU", "fan", d.fan_speed(0).map(|f| format!("{f}%")).map_err(|e| e.to_string()));
        #[cfg(windows)]
        match &self.nvapi {
            Some(api) => match api.thermal_sensors() {
                Ok(t) => {
                    // Index meaning is per generation (LibreHardwareMonitor NvidiaGpu.cs):
                    // 40-series: [1] = hotspot, [7] = memory junction. Blackwell: [1] = core, [2] = memory.
                    // Older: [1] = hotspot, [9] = memory junction. Raw values are 8.8 fixed point.
                    line("GPU", "NvAPI hotspot (idx 1)", Ok(format!("{:.1} C", t[1] as f32 / 256.0)));
                    let others: Vec<String> = t.iter().enumerate().filter(|(i, v)| *i != 1 && **v > 0).map(|(i, v)| format!("[{i}]={:.1}", *v as f32 / 256.0)).collect();
                    line("GPU", "NvAPI other non-zero indices", Ok(if others.is_empty() { "none (memory junction 0 = not reported now)".into() } else { others.join(" ") }));
                }
                Err(e) => line("GPU", "NvAPI hotspot", Err(e)),
            },
            None => line("GPU", "NvAPI hotspot", Err("nvapi64.dll not available".into())),
        }
    }
}

#[cfg(windows)]
mod nvapi {
    //! NvAPI is only reachable through `nvapi_QueryInterface(id)`; the ids and the sensor struct below
    //! are not in NVIDIA's public headers. Source of ids and layout: LibreHardwareMonitor
    //! `LibreHardwareMonitorLib/Interop/NvApi.cs` (MPL-2.0, read only, nothing copied).
    //! Fails safe: any non-zero status means "hide the row".
    use libloading::Library;
    use std::ffi::c_void;

    pub struct NvApi {
        _lib: Library,
        gpu: *mut c_void,
        get_sensors: unsafe extern "C" fn(*mut c_void, *mut Sensors) -> i32,
    }

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
            unsafe {
                let lib = Library::new("nvapi64.dll").ok()?;
                let q: libloading::Symbol<Query> = lib.get(b"nvapi_QueryInterface\0").ok()?;
                let init = q(0x0150E828);
                let enum_gpus = q(0xE5AC921F);
                let sensors = q(0x65FE3AAD);
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
                Some(NvApi { _lib: lib, gpu: handles[0], get_sensors })
            }
        }

        pub fn thermal_sensors(&self) -> Result<[i32; 32], String> {
            let mut s = Sensors { version: (std::mem::size_of::<Sensors>() as u32) | (2 << 16), mask: 0x7FFF_FFFF, reserved: [0; 8], temps: [0; 32] };
            // The mask must only contain existing sensor bits; LHM probes the highest valid mask.
            // Try full mask first, then shrink bit by bit.
            // Same probing as LibreHardwareMonitor: grow the mask one bit at a time until the call fails;
            // the last accepted mask is the full set of existing sensors.
            let mut mask: u32 = 0;
            for bit in 0..32 {
                let try_mask = 1u32 << bit;
                s.mask = try_mask;
                if unsafe { (self.get_sensors)(self.gpu, &mut s) } != 0 {
                    break;
                }
                mask = try_mask | (try_mask - 1);
            }
            if mask == 0 {
                return Err("no valid sensor mask".into());
            }
            s.mask = mask;
            let st = unsafe { (self.get_sensors)(self.gpu, &mut s) };
            if st == 0 { Ok(s.temps) } else { Err(format!("NvAPI status {st}")) }
        }
    }
}
