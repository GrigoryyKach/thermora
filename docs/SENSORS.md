# Thermora: sensor research (T1)

Status: research and prototype, 2026-11-24. Prototype: `Thermora --probe [--seconds N] [--pawn-modules <dir>]`.
Test machine: Windows 11, Core i5-12400F (Alder Lake-S), RTX 4060, ASUS PRIME B660-PLUS D4, 2x Kingston KF3600C18D4/16GX, Kingston SKC3000S1024G + Viper VP4300L 2TB NVMe, Realtek 2.5GbE. PawnIO is **not** installed here (`sc query PawnIO` and the uninstall key are both absent); nothing was installed during research, the probe never elevates and never loads a driver.

Privacy: the prototype only requests whitelisted WMI fields (no serial, UUID or MAC property is ever selected), never calls NVML `uuid()`/`serial()`, never reads sysfs `serial`/`address`/DMI serial files, and prints local IPv4 only as "present". Public IP is never queried.

## 1. Summary table: what works on this PC

| Reading | Source | Needs admin/driver | Result here |
|---|---|---|---|
| CPU load, total + per logical | PDH `\Processor Information(*)\% Processor Time` | no | works |
| CPU clock | PDH `% Processor Performance` x base (`CallNtPowerInformation`) | no | works (4.0-4.2 GHz under light load on a 2.5 GHz base) |
| CPU package temperature | MSR 0x1B1 via PawnIO | driver + elevation | not available without it |
| CPU hotspot (hottest core) | MSR 0x19C per core, max | driver + elevation | not available without it |
| CPU package power | RAPL MSR 0x611 via PawnIO | driver + elevation | not available without it |
| ACPI thermal zone (PDH) | `\Thermal Zone Information` | no | useless: one zone `TZ00`, constant 27.9 C |
| GPU load, temp, VRAM, clocks, limit, PCIe, fan | NVML | no | works |
| GPU power draw | NVML `power_usage` | no | **N/A on this PC**, `nvidia-smi` also prints `[N/A]`; show "-" |
| GPU hotspot | NvAPI thermal sensors idx 1 | no | works (49.7 C vs core 41 C) |
| GPU memory junction | NvAPI idx 7 (RTX 40) / NVML | no | reads 0 here: hide the row |
| GPU load/VRAM, any vendor | PDH `GPU Engine`, `GPU Adapter Memory` | no | works (fallback for AMD/Intel) |
| RAM used/total/available/cached | `GlobalMemoryStatusEx`, `GetPerformanceInfo` | no | works |
| RAM modules (part, size, type, speeds, voltage, slot) | WMI `Win32_PhysicalMemory` | no | works |
| RAM timings | SPD over SMBus / IMC registers | driver + elevation | "-" |
| Disk activity %, read/write B/s | PDH `PhysicalDisk(*)` (instance name also maps disk to drive letters, e.g. `0 C: E:`) | no | works |
| NVMe temperature | NVMe SMART log through `StorageDeviceProtocolSpecificProperty` | no (zero-access handle) | works on both drives |
| NVMe temperature, simple ioctl | `StorageDeviceTemperatureProperty` | no | works on one drive, "not reported" on the other: use as fallback only |
| NVMe health (spare, used %, power-on h, TB written, media errors) | same SMART log | no | works |
| Model, bus type, capacity, SSD/HDD | `IOCTL_STORAGE_QUERY_PROPERTY` + `IOCTL_DISK_GET_DRIVE_GEOMETRY_EX` | no | works |
| Volumes, file system, free space | WMI `Win32_LogicalDisk` | no | works |
| Network rates, link speed | PDH `Network Interface(*)`, `GetAdaptersAddresses` | no | works |
| Board, BIOS | WMI `Win32_BaseBoard`, `Win32_BIOS` | no | works |
| Chipset | PCI id of the PCI-ISA bridge, own table | no | works (`DEV_7A86` = B660) |
| Audio | WMI `Win32_PnPEntity` where `PNPClass='AudioEndpoint'` | no | works, real product names |
| CPU name, family/model/stepping, cores, threads, caches | CPUID, `GetLogicalProcessorInformationEx` | no | works |

Polling cost with a 1 s interval (specs excluded): **0.5 to 1.1 % of one core** (the `GPU Engine(*)` wildcard counter is the most expensive part; poll it every 2-3 s or only for 3D/Copy), working set about **49 MB** for the probe (PDH + WMI/COM + NVML loaded). The final egui window will add to that; budget the 80 MB target by dropping COM after the specs pass and not creating the window while hidden in the tray.

## 2. CPU temperature on Windows

Ring-0 MSR access is the only route to real CPU sensor data on Windows. Windows exposes no per-core or package temperature to user mode (the ACPI thermal zone is a board-level constant here).

### Options

| Option | Verdict |
|---|---|
| WinRing0 | Forbidden. Defender flags it as `VulnerableDriver:WinNT/Winring0` (CVE-2020-14979, https://nvd.nist.gov/vuln/detail/CVE-2020-14979). |
| WMI `MSAcpi_ThermalZoneTemperature` | Rejected: needs admin and reads the same ACPI zone that PDH shows as a constant 27.9 C here. |
| LibreHardwareMonitorLib sidecar (https://github.com/LibreHardwareMonitor/LibreHardwareMonitor, MPL-2.0) | Rejected: adds a .NET runtime and a second process, and it **also** needs PawnIO and elevation since v0.9.5. No gain over talking to PawnIO ourselves. |
| **PawnIO driver, our own thin client** | **Recommended.** |

### PawnIO facts (sources: https://pawnio.eu, https://github.com/namazso/PawnIO, https://github.com/namazso/PawnIO.Modules, https://github.com/namazso/PawnIO.Setup)

- What it is: a signed kernel driver that runs small sandboxed "Pawn" modules; user mode can only call functions a module exports. It is not a raw "read any MSR" driver: the `IntelMSR` module whitelists MSR indexes (temperature, RAPL, APERF/MPERF, platform info and a few write-able power/OC registers). Source of the whitelist: `IntelMSR.p` in PawnIO.Modules. We only ever call reads.
- Install: official `PawnIO_setup.exe` (latest release at research time: 2.2.0, https://github.com/namazso/PawnIO.Setup/releases) installs the driver once, needs admin. It registers an uninstall entry `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\PawnIO` with `DisplayVersion` (LibreHardwareMonitor detects it that way; the probe does too). Silent switches are not documented on the site: verify the installer's switches in T2 before relying on them, otherwise run it interactively.
- **Reading needs admin on every run.** The device security descriptor is `D:P(A;;GA;;;SY)(A;;GA;;;BA)` (SYSTEM + Administrators only), set in `PawnIO/PawnIO.inf.in`. A non-elevated process gets ERROR_ACCESS_DENIED opening `\\?\GLOBALROOT\Device\PawnIO` (the probe reports exactly this).
- Licence: driver GPL-2.0-or-later **with an exception** allowing programs that talk to it solely through the device IO control interface (README of the PawnIO repo). Modules LGPL-2.1(-or-later). Our app uses only the IOCTL interface, so it stays under the suite licence. Ship nothing of the driver; ship (or fetch) the signed module `.bin` files with the LGPL notice and a source link (PawnIO.Modules 0.2.12 at research time).
- How to call from Rust: no PawnIOLib needed. `src/pawnio.rs` talks to the device directly, as LibreHardwareMonitor's `PawnIo.cs` does:
  - open `\\?\GLOBALROOT\Device\PawnIO` read/write;
  - `DeviceIoControl` load (code `CTL_CODE(41394, 0x821, METHOD_BUFFERED, FILE_ANY_ACCESS)`) with the module blob;
  - execute (`0x841`): input = 32-byte zero-padded function name + u64 args, output = u64 results; the Intel read function is `ioctl_read_msr`.
  - (PawnIOLib.dll offers the same as `pawnio_open/load/execute`.)
- **Status of the PawnIO code in the prototype: written, compiles, NOT exercised on hardware** (no driver here and installing one was out of scope). The register maths follows the Intel SDM vol. 4: TjMax = `IA32_TEMPERATURE_TARGET` (0x1A2) bits 23:16; core = TjMax - `IA32_THERM_STATUS` (0x19C) bits 22:16 when bit 31 is set (needs thread affinity per logical CPU); package = TjMax - `IA32_PACKAGE_THERM_STATUS` (0x1B1) bits 22:16; hotspot = max over cores. Power = delta of `MSR_PKG_ENERGY_STATUS` (0x611) x 2^-ESU, ESU from `MSR_RAPL_POWER_UNIT` (0x606) bits 12:8. Clock alternative = base x delta(APERF 0xE8) / delta(MPERF 0xE7). T2 must validate these against HWiNFO/HWMonitor on the user's PC once PawnIO is installed with consent.
- AMD: module `AMDFamily17` (Zen) exposes SMN reads for Tctl/Tdie; not testable here, design only.
- HVCI / memory integrity compatibility is unverified; check in T2 when installing with consent.

### Recommendation: unelevated UI + one elevated headless helper, set up once

Do **not** run the whole GUI elevated (UAC at every start, no drag and drop, bad autostart).

1. The UI process (normal user) does everything that needs no driver.
2. A second mode of the same exe, `Thermora.exe --sensor-helper` (no window, tiny), runs elevated and does only the PawnIO reads. It publishes readings (JSON lines) over a named pipe whose DACL grants only the installing user's SID, read-only, no commands.
3. **One-time setup ("Enable CPU temperature")**, one UAC prompt, performed by an elevated `Thermora.exe --setup-sensor`:
   - install PawnIO from the official installer if missing, installer hash-pinned like Padyx pins ViGEmBus;
   - copy the helper exe (and the signed module `.bin`) to `%ProgramFiles%\Thermora\` (admin-writable only; a helper left in a user-writable folder would be a silent privilege escalation);
   - register a scheduled task "Thermora Sensor" with "Run with highest privileges", principal = the current user (interactive token, no password), action = the Program Files helper, no trigger (started on demand) or an at-logon trigger.
4. From then on the UI starts the task as the normal user (`schtasks /run /tn "Thermora Sensor"` or the Task Scheduler COM API) with no UAC, and stops it on exit. If the app updates, the helper copy is stale until the user accepts one more UAC prompt ("Update sensor component"); show that in Settings.

User experience: first run works without any prompt. CPU card shows load and clock, temperature reads "-" with tooltip "Needs CPU temperature access" and a dismissible one-line banner with **Enable**. Enable opens an in-app dialog naming PawnIO, its author and licence, what the helper does and that Windows will ask once for permission; Agree leads to the UAC prompt. **If the user declines** (dialog or UAC): nothing is installed, the banner is dismissed, the temperature stays "-", every other reading keeps working, and "Enable CPU temperature" stays in Settings and in the tooltip. The scheduled-task design also makes "Start with the system" simple: the UI starts from the Run key and asks the task for the helper.

Open question for review: the helper needs the Program Files copy kept in sync with the UI exe after each self-update. If that is unwanted, the alternative is the whole app registered as an elevated scheduled task and launched through it (one UAC at setup, elevated GUI forever); I recommend the split because the elevated surface is a few hundred lines with no UI.

## 3. CPU load, clock, power

- Load per logical CPU: PDH `\Processor Information(*)\% Processor Time` (instances `0,0` ... and `_Total`). Alternative without PDH: `NtQuerySystemInformation(SystemProcessorPerformanceInformation)`; the `sysinfo` crate does the latter. PDH matched Task Manager closely in the probe.
- Clock: `% Processor Performance` x base MHz. Base from `CallNtPowerInformation(ProcessorInformation)` `MaxMhz` (2500 here, equals the 12400F base). Values above 100 % mean turbo. With the driver, APERF/MPERF gives a truer effective clock.
- Package power: RAPL MSR only, driver + elevation. On Linux `energy_uj` is root-only since CVE-2020-8694, so power is "-" without root there.
- Sources: PDH https://learn.microsoft.com/windows/win32/perfctrs/performance-counters-portal ; Intel SDM vol. 4 (MSR tables).

## 4. NVIDIA GPU

- NVML (https://docs.nvidia.com/deploy/nvml-api/ , Rust wrapper `nvml-wrapper` https://github.com/rust-nvml/nvml-wrapper, loaded with libloading at runtime, so no hard dependency): name, driver version, load, core temperature, VRAM used/total, core/memory clock and maximums, power limit, fan, PCIe gen and width (current and max), slowdown/shutdown temperature. All worked on the RTX 4060 except power draw (see table).
- Power draw is "not available" on this PC through NVML and `nvidia-smi`, so it is a driver/board property, not a bug. Show "-" with "Not reported by this GPU".
- **Hotspot** through NvAPI `NvAPI_GPU_GetThermalSensors` (interface id `0x65FE3AAD`, via `nvapi64.dll!nvapi_QueryInterface`; also `0x0150E828` Initialize, `0xE5AC921F` EnumPhysicalGPUs). Struct (`version`, `mask`, 8 reserved ints, 32 temperatures, values in 1/256 C) and the per-generation index meaning come from LibreHardwareMonitor `Interop/NvApi.cs` and `Hardware/Gpu/NvidiaGpu.cs`: RTX 40: index 1 hotspot, index 7 memory junction; Blackwell: index 1 core, 2 memory; older: 1 hotspot, 9 memory junction. The mask is probed bit by bit like LHM. Measured here: index 1 = 49.7 C while NVML core = 41 C (plausible hotspot delta). Memory junction reads 0 at idle on this board: hide it when 0.
- Use it, behind a guard: it is undocumented and NVIDIA can change it with a driver update. Failure modes are all safe: DLL missing, interface id returns null, init or call returns non-zero, or all values 0 -> the hotspot row shows "-". Only call it with the exact struct version; never write anything through NvAPI. Re-check the index table when a new GPU generation appears.

## 5. AMD and Intel GPUs (design only, no hardware to test)

- Linux amdgpu: hwmon (`/sys/class/drm/card*/device/hwmon/hwmon*/`): `temp1_input` edge, `temp2_input` junction, `temp3_input` mem (labels `edge`/`junction`/`mem`), `power1_average` (uW), `fan1_input`, `freq1_input` (sclk, Hz); load `device/gpu_busy_percent`; VRAM `device/mem_info_vram_used` / `_total`. https://docs.kernel.org/gpu/amdgpu/thermal.html , https://docs.kernel.org/admin-guide/pm/amdgpu.html
- Linux Intel: i915/xe hwmon where present, else "not available".
- Windows AMD: ADLX (MIT SDK, `amdadlx64.dll` loaded dynamically) gives temperature, hotspot, power, fan; skip in 1.0 and use the PDH fallback. Windows Intel: IGCL is possible later; skip.
- Windows fallback for any vendor, no admin, already in the prototype: PDH `\GPU Engine(*)\Utilization Percentage` (sum by `engtype_3D`, as Task Manager) and `\GPU Adapter Memory(*)\Dedicated Usage`. Temperature, power, fan show "Not reported by this GPU".

## 6. RAM

- Used/total/available: `GlobalMemoryStatusEx`. Cached: `GetPerformanceInfo().SystemCache` x page size (file cache; Task Manager's "Cached" also counts the modified list, which needs a privileged call, so ours is a close lower bound).
- Modules from WMI `Win32_PhysicalMemory` (no admin): Manufacturer, PartNumber, Capacity, Speed (rated), ConfiguredClockSpeed, SMBIOSMemoryType (26 = DDR4, 34 = DDR5), DeviceLocator, ConfiguredVoltage. Measured: two Kingston KF3600C18D4/16GX, 16 GB DDR4, rated 3600, configured 3600 MT/s, 1200 mV. `SerialNumber` is never selected.
- **Timings (e.g. 18-22-22-39) are not available without the driver.** WMI has none. Two driver routes exist: SPD EEPROM over the chipset SMBus (PawnIO module `SmbusI801`; this PC has the PCH SMBus controller, `DEV_7AA3`), which returns the JEDEC/XMP **profiles** stored on the module, not necessarily what the BIOS applies; or the memory controller registers (`IntelMCHBAR` module) which give the **active** timings but need per-generation register offsets we do not have documented. SMBus access also races with ASUS RGB/ACPI SMBus users. Recommendation: show "-" for timings in 1.0; revisit as an optional later feature.
- Linux: SMBIOS type 17 needs root (`dmidecode -t 17`, or reading `/sys/firmware/dmi/tables/DMI` which is root-only); without root show only totals. Source: `dmidecode(8)` man page (DMI tables are root-readable only).

## 7. Storage

- Query-only handle: `CreateFileW(\\.\PhysicalDriveN, access = 0, share read+write)` needs no elevation for the ioctls below (https://learn.microsoft.com/windows/win32/api/winioctl/ni-winioctl-ioctl_storage_query_property).
- Model/vendor/bus type: `StorageDeviceProperty` descriptor (offsets for vendor, product, bus type; the serial offset is never read). Bus 17 = NVMe, 11 = SATA, 7 = USB. SSD vs HDD: `StorageDeviceSeekPenaltyProperty`. Capacity: `IOCTL_DISK_GET_DRIVE_GEOMETRY_EX` (the `LENGTH_INFO` ioctl needs read access, avoid).
- NVMe temperature and health: `StorageDeviceProtocolSpecificProperty` with `STORAGE_PROTOCOL_SPECIFIC_DATA` (ProtocolTypeNvme, NVMeDataTypeLogPage, log page 2, 512 bytes) returns the SMART/Health log: composite temperature (Kelvin), available spare, percentage used, critical warning, data units written, power-on hours, media errors. https://learn.microsoft.com/windows/win32/fileio/working-with-nvme-devices . **This worked without admin on both drives.** `StorageDeviceTemperatureProperty` is only a fallback: it was "not reported" on the Kingston SKC3000S1024G.
- Activity: PDH `\PhysicalDisk(*)\% Disk Time`, `Disk Read Bytes/sec`, `Disk Write Bytes/sec`. The instance name carries the drive letters (`0 C: E:`), which maps volumes to drives without WMI associators. Partitions/file systems: `Win32_LogicalDisk` (DriveType 3).
- Linux: `/sys/block/*/{size,queue/rotational,device/model}`, `/proc/diskstats`, hwmon `nvme` chip for temperature (no root). SMART health on Linux needs root or `nvme-cli`; show "-".

## 8. Network

- `GetAdaptersAddresses`: FriendlyName, Description, OperStatus, IfType, TransmitLinkSpeed, first unicast presence. Rates: PDH `\Network Interface(*)\Bytes Received/sec`, `Bytes Sent/sec`, `Current Bandwidth`. Linux: `/sys/class/net/<if>/{operstate,speed,statistics/rx_bytes,tx_bytes}`.
- Local IPv4 may be shown in the UI later (documented requirement), but the **MAC (`PhysicalAddress`, `/sys/class/net/*/address`) is never read** and the public IP is never queried. The prototype prints the IP as "present (hidden)".
- The Windows list includes virtual and Bluetooth adapters; the app should show only adapters that are up and not loopback/tunnel, with the one carrying traffic as "active".

## 9. Motherboard, BIOS, audio, CPU details

- Board: WMI `Win32_BaseBoard` (Manufacturer, Product); BIOS: `Win32_BIOS` (Manufacturer, SMBIOSBIOSVersion, ReleaseDate). Linux: `/sys/class/dmi/id/{board_vendor,board_name,bios_vendor,bios_version,bios_date}` (world-readable; the serial/UUID files are root-only and never read).
- Chipset: not in WMI. The LPC/eSPI bridge shows up as the generic "PCI-ISA bridge"; its PCI device id is a bus identifier, not a serial. Our own table maps it (`7A86` = Intel B660 verified here). Extend the table from https://pci-ids.ucw.cz . Unknown id -> "-".
- Audio: WMI `Win32_SoundDevice` only has driver names ("USB Audio Device"). **Use `Win32_PnPEntity` with `PNPClass='AudioEndpoint'`**: names are the ones users know ("Microphone (fifine SC3)", "Digital Audio Interface (C1-1 USB3 Audio)", "DELL ... (NVIDIA High Definition Audio)"). Names are localized by Windows; group by the parenthesised device. Linux: `/proc/asound/cards`.
- CPU details: name from CPUID brand string; vendor/family/model/stepping from CPUID leaf 1; cores/threads/caches/hybrid split from `GetLogicalProcessorInformationEx` (6 cores, 12 threads, L1i 6x32 KB, L1d 6x48 KB, L2 6x1280 KB, L3 18 MB here). Base clock from `CallNtPowerInformation`.
- **Codename, socket, TDP come from a table we maintain**: `(vendor, family, model)` -> codename + socket (in `win.rs::codename`, e.g. GenuineIntel 6-0x97 = Alder Lake-S, LGA1700; Raptor Lake 0xB7/0xBF; Arrow Lake 0xC6; AMD 0x19/0x1A = Zen 3-5), and brand substring -> base power for TDP (`tdp()`). WMI's `SocketDesignation` was correct here ("LGA1700") but is often a vendor string, so it is only a hint. With the driver, exact TDP comes from `MSR_PKG_POWER_INFO`. Unknown entries show "-".

## 10. Linux

No root needed for: `/proc/stat` (load), `/proc/meminfo`, `/proc/cpuinfo`, `/sys/devices/system/cpu/cpu*/cpufreq/scaling_cur_freq`, `/sys/class/hwmon/*` for `coretemp` (Intel: `Package id 0`, `Core N`; hotspot = max core), `k10temp` (AMD: `Tctl`, `Tdie`, `Tccd*`), `nvme` (composite), `amdgpu` (edge/junction/mem), `/sys/class/net`, `/sys/block`, `/proc/diskstats`, `/sys/class/dmi/id` and NVML via `libnvidia-ml.so.1` (same dynamic loading as Windows). Docs: https://docs.kernel.org/hwmon/coretemp.html , https://docs.kernel.org/hwmon/k10temp.html .
Needs root: RAPL `energy_uj` (since CVE-2020-8694), SMBIOS memory modules (`dmidecode`), SPD timings, SMART via ioctl. These show "-" with a tooltip. The Linux half of the prototype (`src/linux.rs`) compiles for `x86_64-unknown-linux-gnu` (`cargo check`) but was **not run** (no Linux machine, no Docker by rule); CI at release is the first real run.

## 11. Measured cost

`Thermora --probe --seconds 10`, release build, 1 s poll, specs excluded: 0.5-1.1 % of one core, working set ~49 MB (probe, no window). The most expensive counter is `GPU Engine(*)` (hundreds of per-process instances). Plan for T2: PDH query created once; GPU Engine every 2 s; drop COM after the specs pass; no egui repaint while hidden in the tray.

## Findings that change the T2 plan

1. NVMe temperature and SMART need no admin and no driver: ship them in the base feature set.
2. GPU hotspot via NvAPI works here; memory junction and power draw do not on this board: "-" is a normal state, not an error.
3. CPU package/hotspot temperature, package power and RAM timings are the only things behind the driver. CPU load, clock, everything else is driver-free.
4. Audio and chipset need the two workarounds above, otherwise the lists show generic names.
