# Mac Cooling Control

GPU-aware automatic control of the Razer Laptop Cooling Pad on macOS.
The `padctl` CLI reads real die-temperature sensors and Apple GPU utilization,
then commands the higher of the temperature and GPU fan targets. It can run
as a user login service or an interactive terminal dashboard.

This project is derived from [Harold Martin's padctl](https://github.com/hbmartin/razer-cooling-pad-mac).
The original MIT license and upstream history are retained. It is independent
of Razer and Apple and does not change CPU/GPU power limits.

## Hardware and behavior

The control interface must appear as USB `1532:0f43`. A generic USB hub alone
is not sufficient. On the development machine, moving to another USB-A port
on the same dock exposed both Razer HID interfaces.

- Temperature input: hottest available `PMU tdie` or `MTR Temp Sensor` reading.
- GPU input: IOKit `AGXAccelerator` / `Device Utilization %`.
- The higher requested speed wins. GPU load can ramp cooling before heat builds.
- Temperature smoothing and a delayed downward transition limit fan oscillation.
- The hottest configured temperature bypasses smoothing.
- macOS serious or critical thermal pressure requests at least 3000 RPM.
- Missing telemetry requests a 3000 RPM failsafe and logs the error.
- Missing temperature readings at startup cause an error, not invented Celsius values.
- `--verify` checks the device response, checksum, transaction and fan-command value.
- A device lock prevents competing padctl instances within one user account.
- The loop probes USB liveness each poll and reconnects after unplug/replug.

Fan RPM is an acknowledged command, **not a measured tachometer speed**.
An unrelated lighting/firmware reply makes fan readback unavailable rather
than displaying arbitrary bytes as a speed. See [validation](docs/VALIDATION.md).

## Build and install

Requires macOS, Rust 1.99.0 and Xcode Command Line Tools. Linux builds support
protocol tests; the automatic GPU controller requires macOS.

```sh
git clone https://github.com/marc-shade/mac-cooling-control.git
cd mac-cooling-control
rustup toolchain install 1.99.0
export PATH="$(dirname "$(rustup which --toolchain 1.99.0 rustc)"):$PATH"
cargo build --release --locked
mkdir -p ~/.local/bin
install -m 755 target/release/padctl ~/.local/bin/padctl
~/.local/bin/padctl doctor --json
~/.local/bin/padctl telemetry
```

Commands need access to IOKit/HID. A restricted execution sandbox can block
both sensors and USB access. The native process does not require root on the
validated machine.

## Automatic cooling

Copy `config.example.toml` to `~/.config/padctl/config.toml` after checking for
an existing configuration, or use `padctl config init` to create one without
overwriting. Command-line options override the configuration.

```sh
~/.local/bin/padctl --verify curve
# Or install and start the user login service:
~/.local/bin/padctl service install
~/.local/bin/padctl service status
~/.local/bin/padctl telemetry
tail -f ~/Library/Logs/padctl.log
```

The example configuration uses these targets:

| Die temperature | RPM | GPU use | RPM |
|---|---:|---|---:|
| 45°C | 1000 | 0% | 1000 |
| 55°C | 1400 | 20% | 1400 |
| 65°C | 2000 | 50% | 2000 |
| 75°C | 2600 | 80% | 2600 |
| 85°C | 3000 | 100% | 3000 |

Targets interpolate between points. The example polls every two seconds,
uses five-second temperature smoothing and a 30-second downward delay.
`on_exit = "keep"` keeps the fan running at its last command when stopped
or asleep. These settings are configurable and are not universal thermal limits.

Stop the service before issuing manual hardware commands or running the
interactive controller:

```sh
launchctl bootout gui/$(id -u)/io.github.hbmartin.padctl
~/.local/bin/padctl --verify fan set 1500
~/.local/bin/padctl --verify watch
# Restart after finishing manual control or editing the config:
~/.local/bin/padctl service install
```

The upstream LaunchAgent identifier is retained for compatibility.
`watch --dry-run` reads real telemetry and displays decisions without opening
the pad, so it can run alongside the service. Arrow keys edit temperature
curve points, `S/s` changes smoothing, `D/d` changes downward delay, `w` saves
and `q` quits. GPU curve points are edited in the configuration.

## Diagnostics and lighting

```sh
padctl list
padctl doctor --json       # stop the service first
padctl status --json       # stop the service first
padctl temp --json
padctl sensors --json
padctl telemetry           # safe alongside the service
padctl --verify rgb static ff0000
padctl --verify rgb spectrum
padctl --verify rgb brightness 50
```

`fan get` reports the last acknowledged fan command. Lighting/firmware
queries replace that response buffer: `fan get` then returns an error and
`status` reports a null commanded RPM. Neither measures actual RPM.

## Development and validation

```sh
cargo fmt --all --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
xcrun swiftc -O tools/metal-load.swift -o /tmp/padctl-metal-load
/tmp/padctl-metal-load 60
```

The Metal program is a synthetic matrix workload with checked outputs and
GPU timestamps, not an application performance benchmark. The opt-in hardware
failsafe test sends real fan commands and requires the service to be stopped:

```sh
cargo test hardware_sensor_failure -- --ignored --nocapture
```

Raw local validation files and installation records are gitignored. Public
results in `docs/VALIDATION.md` distinguish hardware checks, software tests,
manual observations and limitations.

## Gaps / not covered

The private Apple temperature API and GPU counter may change across macOS
versions. Sensor names are not a verified CPU/GPU core map. An unplugged or
unpowered pad cannot execute failsafe commands. Acknowledgements do not prove
physical RPM or airflow. Independent RPM/noise measurement requires external
instruments. This is a CLI and service, not a notarized GUI application.
See the validation record for the exact tests performed and remaining gaps.
