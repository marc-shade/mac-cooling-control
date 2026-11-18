# Mac Cooling Control

GPU-aware automatic control of the Razer Laptop Cooling Pad on macOS.
The `padctl` CLI reads real die-temperature sensors and Apple GPU utilization,
then commands the higher of the temperature and GPU fan targets. It can run
as a user login service or an interactive terminal dashboard.

This project is derived from [Harold Martin's padctl](https://github.com/hbmartin/razer-cooling-pad-mac).
The original MIT license and upstream history are retained. It is independent
of Razer and Apple and does not change CPU/GPU power limits.

The login service also supplies a live monitoring feed to the
[XRG cooling overview](docs/XRG.md), alongside XRG's measured Mac fan speeds.

## Contributors

- [Marc Shade (@marc-shade)](https://github.com/marc-shade): project direction,
  macOS requirements, and physical fan-response and USB-reconnect validation.
- [Harold Martin (@hbmartin)](https://github.com/hbmartin): original padctl project.

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

Readings, dashboard charts and service logs use Fahrenheit by default. Use
`--temperature-unit celsius` for Celsius displays, for example
`padctl --temperature-unit celsius temp`. JSON includes a selected-unit value
and `temperature_unit`, plus explicitly named Fahrenheit and Celsius fields.
Existing Celsius JSON fields retain their original meaning.

Saved temperature curves, `--points`, and thermal-lighting `--min`/`--max`
remain explicitly Celsius. Dashboard curve edits use 1°C (1.8°F) steps and
save Celsius values. Changing display units does not change cooling targets.

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
| 113°F (45°C) | 1000 | 0% | 1000 |
| 131°F (55°C) | 1400 | 20% | 1400 |
| 149°F (65°C) | 2000 | 50% | 2000 |
| 167°F (75°C) | 2600 | 80% | 2600 |
| 185°F (85°C) | 3000 | 100% | 3000 |

Targets interpolate between points. The example polls every two seconds,
uses five-second temperature smoothing and a 30-second downward delay.
`on_exit = "keep"` keeps the fan running at its last command when stopped
or asleep. These settings are configurable and are not universal thermal limits.

### Follow the MacBook Pro's internal fans

Use `config.mac-fans.toml` for a quieter fan-led profile. The controller reads
the actual speed of each internal fan from AppleSMC and uses the fastest.
The first positive reading from either Mac fan starts the pad at a minimum
of 500 RPM. The fan curve holds 500 through 1800 internal RPM, then ramps to
1000 at 2500, 1800 at 3500 and 3000 at 5000 internal RPM. A lower target must persist
for 30 seconds. GPU spikes alone leave the pad off. The independent temperature
curve ramps from 75°C to 85°C, and serious or critical thermal pressure still
requests 3000 RPM. Missing required fan readings use the telemetry failsafe.

`padctl telemetry` reports individual tachometers in `mac_fans_rpm` and any
read error in `mac_fans_error`. The service publishes the same RPM array in
`status.json` while following the fan curve and includes it in decision logs.
The profile also enables `[lighting]` with `effect = "fan-status"` and
35% brightness. In automatic mode the strip is dark at 0 commanded RPM,
blue below 1500 RPM, amber from 1500 through 2950 RPM, and red at 3000 RPM
or higher. Both `curve` and `watch` verify fan commands before changing
the color. An LED error is logged and retried while cooling continues.
The service publishes the acknowledged color in `lighting_status`.

Existing configurations without `mac_fan_points` keep their original behavior.

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

### Unattended macOS service evaluation

The controller writes fan decisions, USB acknowledgements, errors, reconnects
and idle heartbeats to `~/Library/Logs/padctl.log`. To check the service while
it owns the pad, install the independent evaluator from this checkout:

```sh
mkdir -p ~/.local/bin ~/Library/LaunchAgents
install -m 755 tools/padctl-eval ~/.local/bin/padctl-eval
install -m 644 packaging/launchd/io.github.hbmartin.padctl.eval.plist ~/Library/LaunchAgents/
launchctl bootstrap "gui/$(id -u)" ~/Library/LaunchAgents/io.github.hbmartin.padctl.eval.plist
```

The login agent records a result every minute while the Mac is awake. Each
check requires a running launchd controller, a fresh and connected snapshot,
an acknowledged fan command, valid die/GPU/thermal readings, a current event
log and a clear controller error field. It runs independently of the controller
and leaves the pad available for automatic control.

```sh
~/.local/bin/padctl-eval                 # JSON on stdout, exit 0 on pass or 1 on failure
tail -n 5 ~/Library/Logs/padctl-eval.jsonl
tail -f ~/Library/Logs/padctl.log
```

The evaluation history contains UTC timestamps, readings and failure reasons.
It rotates at 1 MiB with four backups. Check the timestamp as well as `ok`
when reading history. The most recent row describes the instant it was sampled.

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
