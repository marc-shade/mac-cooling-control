# Validation record

Automatic GPU and temperature control was exercised with the physical pad on
2026-11-17. This record separates device acknowledgements from physical measurements.

## Environment

- `system_profiler SPHardwareDataType`: MacBook Pro Mac17,10, Apple M5 Max, 128 GB.
- `sw_vers`: macOS 27.2.1, build 26B8132.
- USB control interface `1532:0f43`, firmware v1.10, connected through a dock.
- Rust 1.99.0, native arm64 release build. No root privileges used by the controller.
- Automatic configuration: `config.example.toml`, with response verification enabled.
- Upstream base: `hbmartin/razer-cooling-pad-mac` commit
  `79fe6e773cfd95cbfa3a2defacb02b5b9d3acf48`.

## Executed gates

| Gate | Result |
|---|---|
| `cargo fmt --all --check` | Exit 0 |
| `cargo clippy --all-targets --locked -- -D warnings` | Exit 0 |
| `cargo test --locked` | 84 unit tests and 16 property tests passed; one opt-in hardware test skipped in this gate |
| `cargo build --release --locked` | Exit 0 |
| `cargo test hardware_sensor_failure -- --ignored --nocapture` | Exit 0; one hardware test passed |
| `padctl telemetry` | Exit 0; live die temperature, GPU percentage and nominal thermal pressure returned |
| `padctl doctor --json` with service stopped | Eight checks OK, zero failures, one expected service-not-loaded warning |
| `padctl service install` and `padctl service status` | Exit 0; LaunchAgent loaded and process running |
| `ruby -c packaging/homebrew/padctl.rb` | Syntax OK; installation through Homebrew not tested |
| `xcrun swiftc -O tools/metal-load.swift -o /tmp/padctl-metal-load` | Exit 0 |

The hardware failsafe test directly invokes the telemetry-failure handler with
the real pad connected. It sends 3000 RPM, validates the returned command
acknowledgement, and restores 1500 RPM. It does not physically disable an Apple
temperature sensor or establish measured fan RPM.

The software tests cover malformed responses, checksum/transaction mismatches,
busy/rejected commands, invalid curves, unavailable telemetry, hottest die
selection, GPU priority, immediate high-temperature response, thermal-pressure
override, smoothing and delayed cooldown. Scripted transports are used only in
protocol unit tests; the live service uses HID.

## Live control

The synthetic MPS matrix workload ran for 300.003 seconds and completed 210484
matrix multiplications. Sampled output checks passed. GPU utilization reached
97% when the controller changed from 1300 to an acknowledged 2950 RPM.
The controller later returned to an acknowledged 1000 RPM, with the configured
30-second delay applied to downward changes.

A separate 135-sample telemetry recording captured 0% to 99% GPU utilization
and 51.88°C to 74.51°C hottest die temperature during load and recovery. These
are observed values, not proof that the pad caused a temperature reduction.
Other applications remained running.

After adding the thermal-pressure override, the final release binary completed
another 30-second workload with sampled output checks passing. The service
raised its acknowledged command from 1250 to 2900 RPM at 95% GPU use.
Serious/critical pressure is tested by injecting a state into the controller;
the Mac was not deliberately overheated to create these states.

## USB recovery and interface checks

With the service active, the user unplugged only USB while retaining pad power.
The log recorded disconnect at 13:44:50 EST, reconnect at 13:45:05, and an
acknowledged 1600 RPM command at 13:45:06. The process ID remained unchanged.

The controller process was subsequently terminated with `kill -KILL` to exercise
crash recovery. Launchd started a new process, and the log recorded an
acknowledged 1350 RPM command at 13:45:55. This checks process recovery without
sleeping or rebooting the computer.

A subsequent installation check found the inherited legacy `launchctl load`
fallback could report success while the GUI-domain agent was not loaded.
The installer now reports bootstrap errors directly, waits for an existing
agent to unload, and verifies registration after bootstrap. Three consecutive
install/reload operations then exited 0 with a running process each time;
the final process logged an acknowledged 1550 RPM command at 13:48:49.

The user heard a physical fan change during the earlier 1000/3000 RPM test.
This is a manual observation, not a calibrated speed measurement.

Concurrent manual control was rejected by the per-user device lock while the
service held the pad. The terminal dashboard was rendered in cmux with real
temperature/GPU data in dry-run mode. Pause and quit were exercised. Dashboard
edits during inspection were not saved to the service configuration.

## Gaps / not covered

- Sleep/wake and reboot/login recovery were not executed because the user
  requested no interruptions. LaunchAgent installation/reload is not a reboot test.
- No controlled pad-off versus pad-on performance comparison, application FPS,
  throttling reduction, battery impact, or acoustic improvement was measured.
- No independent tachometer, airflow or calibrated noise measurement was taken.
- Lighting effects have protocol support but were not visually verified here.
- Cross-user contention and simultaneous third-party controllers were not tested.
- Other Mac models, Intel GPUs, other docks, firmware versions and future macOS
  releases were not exercised. Apple temperature APIs and GPU counters may change.
- No notarized GUI, binary release or published Homebrew tap is provided.
- Raw diagnostics remain local and gitignored because they contain device IDs
  and user paths. This public record contains the reproducible gates and outcomes.
