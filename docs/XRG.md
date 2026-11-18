# XRG integration

The automatic controller publishes a monitoring snapshot for
[marc-shade/XRG](https://github.com/marc-shade/XRG). XRG reads this file without
opening the USB controller or changing the fan speed.

On macOS the file is `~/Library/Caches/padctl/status.json`. Run
`padctl service install` using this build to enable publication. Dry-run mode
does not publish. Interactive `watch` and individual manual commands do not
publish this feed.

## Snapshot contract

| Field | Meaning |
|---|---|
| `schema_version` | Integer 1 |
| `updated_at` | Unix timestamp in seconds |
| `valid_for_seconds` | Freshness window, three polling intervals bounded to 15 through 900 seconds |
| `running` | Whether the controller is active; false on clean exit |
| `connected` | Whether the last poll retained a working USB connection |
| `fan_commanded_rpm` | Last acknowledged command, or null without verification/connection/acknowledgement |
| `lighting_status` | Acknowledged automatic status light (off, blue, amber or red), or null when unavailable. |
| `mac_fans_rpm` | Measured Mac fan speeds in fan index order when the internal fan curve is enabled and readings are available. Zero means the fan is stopped. |
| `fan_measured_rpm` | Null; no validated tachometer measurement |
| `pad_temperature_celsius` | Null; no validated pad-temperature measurement |
| `die_celsius`, `die_fahrenheit` | Current Mac die sample in explicitly named units, or null on telemetry failure |
| `gpu_percent` | Live Apple GPU utilization, or null on failure |
| `thermal_pressure` | macOS thermal state name, or null on failure |
| `error` | Current control error, or null |

Publication uses an atomic rename and owner-only file permissions. A separate
publisher lock prevents a second controller from overwriting the active feed.
Publishing errors are logged and do not stop cooling. Consumers must reject
stale, unsupported or malformed data instead of displaying old RPM as current.

## Executed validation

`cargo test --locked` passed 86 unit and 16 property tests. Formatting, clippy
and the release build exited 0. The native XRG probe consumed the actual feed
alongside live SMC fan readings and HID die temperatures, and rejected six
invalid/stale inputs. The installed XRG view displayed the Mac die in Fahrenheit,
both internal fan readings and the pad's acknowledged command.

Pausing the publisher process made XRG display `Stale` after the 15-second
freshness window. Resuming it restored a numeric command display. The controller
was resumed, and automatic cooling remained active after the check.

A second live controller was refused both monitoring and device locks. After
that process exited, the original service still published a connected status
with its acknowledged fan command.

## Gaps / not covered

The feed is monitoring-only and currently comes from `curve`/the login service.
It does not provide XRG fan controls. No pad tachometer or pad-temperature value
has been validated. Sleep/reboot and other user sessions were not tested. The
XRG integration was exercised on one M5 Max MacBook Pro; Linux XRG is not integrated.
