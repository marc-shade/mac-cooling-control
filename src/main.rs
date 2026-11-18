use std::io::Write;

use anyhow::{Context, Result, bail};
use clap::{CommandFactory, Parser, Subcommand};

use padctl::device::{self, OpenOpts, Pad, Selector};
use padctl::packet::{self, PACKET_LEN, REPORT_LEN};
use padctl::parse::{self, Speed};
use padctl::temp::units;
use padctl::{config, curve, doctor, fan, lighting, logging, reactive, rgb, service, temp, watch};

#[derive(Parser)]
#[command(
    name = "padctl",
    about = "Control the fans and lights of a Razer Laptop Cooling Pad (1532:0f43)",
    version
)]
struct Cli {
    /// Display temperature unit; saved curves and temperature input flags remain Celsius
    #[arg(long, global = true, value_enum, default_value = "fahrenheit")]
    temperature_unit: units::Unit,

    /// Debug logging: raw packets sent/received, every curve poll
    #[arg(short, long, global = true)]
    verbose: bool,

    /// After each command, read back the device status and fail if it was
    /// rejected or the acknowledgement does not match
    #[arg(long, global = true)]
    verify: bool,

    /// Select a specific pad by USB serial number (see `padctl list`)
    #[arg(long, global = true)]
    serial: Option<String>,

    /// Select a specific pad by HID path (see `padctl list`)
    #[arg(long, global = true, value_name = "HID_PATH")]
    path: Option<String>,

    #[command(subcommand)]
    cmd: Cmd,
}

impl Cli {
    fn selector(&self) -> Selector {
        Selector {
            serial: self.serial.clone(),
            path: self.path.clone(),
        }
    }

    fn open_opts(&self) -> OpenOpts {
        OpenOpts {
            verify: self.verify,
        }
    }
}

#[derive(Subcommand)]
enum Cmd {
    /// Read die temperature, GPU utilization and Mac fan speeds without opening the pad
    Telemetry,
    /// List the pad's HID interfaces
    List,
    /// Fan control
    Fan {
        #[command(subcommand)]
        cmd: FanCmd,
    },
    /// RGB lighting control
    Rgb {
        #[command(subcommand)]
        cmd: RgbCmd,
    },
    /// Show firmware version and serial number
    Info {
        /// Emit machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// One-shot overview: fan command, brightness, firmware, serial, die temperature
    Status {
        /// Emit machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// Print the hottest die temperature reading the fan curve would use
    Temp {
        /// Emit machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// List the temperature sensors visible to padctl
    Sensors {
        /// Emit machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// Send a raw 90-byte packet (hex); advanced/protocol exploration
    Raw {
        /// Hex bytes of the packet, spaces optional (up to 90 bytes; zero-padded)
        hex: String,
        /// Compute and fill in the XOR checksum at byte 88
        #[arg(long)]
        auto_crc: bool,
        /// Read and print the feature report after sending
        #[arg(long)]
        read: bool,
    },
    /// Run an automatic fan curve from CPU temperature (Ctrl-C to stop)
    Curve(curve::CurveArgs),
    /// Live dashboard for the fan curve with in-place tuning (q to quit)
    Watch(curve::CurveArgs),
    /// Manage a launchd agent that runs the fan curve at login (macOS)
    Service {
        #[command(subcommand)]
        cmd: service::ServiceCmd,
    },
    /// Manage ~/.config/padctl/config.toml
    Config {
        #[command(subcommand)]
        cmd: ConfigCmd,
    },
    /// Diagnose common problems: device, permissions, Synapse, config,
    /// service health, temperature source
    Doctor {
        /// Emit machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// Generate shell completions (e.g. `padctl completions zsh`)
    Completions {
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
    /// Generate a man page (roff) on stdout
    Manpage,
}

#[derive(Subcommand)]
enum FanCmd {
    /// Set fan speed: RPM (500-3200, step 50), a percentage like 60%, or off
    Set { speed: String },
    /// Turn the fans off
    Off,
    /// Read the current fan speed
    Get {
        /// Emit machine-readable JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum RgbCmd {
    /// Turn lighting off
    Off,
    /// Static color, e.g. ff6600
    Static { color: String },
    /// Spectrum cycling
    Spectrum,
    /// Wave effect
    Wave {
        #[arg(long, value_enum, default_value = "right")]
        dir: rgb::WaveDirection,
        /// Wave speed byte (device default 40)
        #[arg(long, default_value_t = rgb::DEFAULT_WAVE_SPEED)]
        speed: u8,
    },
    /// Breathing: no color = random, one color = single, two = dual
    Breath { colors: Vec<String> },
    /// Brightness 0-100 (no value: read current)
    Brightness { percent: Option<u8> },
    /// Per-LED colors via a custom frame (1-18 colors, stretched to fit;
    /// experimental on this device)
    Custom {
        /// Colors like ff6600 (1-18); fewer than 18 are stretched in blocks
        #[arg(required = true)]
        colors: Vec<String>,
        /// Put the device in driver mode first (try this if nothing changes)
        #[arg(long)]
        driver_mode: bool,
    },
    /// Linear gradient across the strip (experimental on this device)
    Gradient {
        from: String,
        to: String,
        /// Put the device in driver mode first (try this if nothing changes)
        #[arg(long)]
        driver_mode: bool,
    },
    /// Map CPU temperature onto the strip, updating live (Ctrl-C to stop;
    /// experimental on this device)
    Thermal(reactive::ThermalArgs),
    /// Apply the [lighting] section of ~/.config/padctl/config.toml
    Apply,
}

#[derive(Subcommand)]
enum ConfigCmd {
    /// Write a commented template config file
    Init {
        /// Overwrite an existing config file
        #[arg(long)]
        force: bool,
    },
    /// Print the config file path
    Path,
    /// Show the effective curve settings (config merged with defaults)
    Show,
}

fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(cli) {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<()> {
    units::set(cli.temperature_unit);
    // The watch TUI owns the terminal: a stderr logger would draw over the
    // alternate screen, so leave the log facade uninitialized (no-op) and
    // let the dashboard record events itself.
    if !matches!(cli.cmd, Cmd::Watch(_)) {
        logging::init(cli.verbose);
    }

    // Commands that don't touch the device (or manage it themselves).
    match &cli.cmd {
        Cmd::Telemetry => {
            let reader = temp::TempReader::new()?;
            let celsius = reader.read()?;
            let fans = padctl::mac_fans::read_rpm();
            return print_json(&serde_json::json!({
                "die_temperature": units::value(celsius),
                "mac_fans_rpm": fans.as_ref().ok(),
                "mac_fans_error": fans.as_ref().err().map(|e|format!("{e:#}")),
                "temperature_unit": units::current().name(),
                "die_fahrenheit": units::Unit::Fahrenheit.convert(celsius),
                "die_celsius": celsius, "temperature_source": reader.source_name(),
                "gpu_percent": padctl::gpu::read_percent()?,
                "thermal_pressure": padctl::gpu::pressure_name(padctl::gpu::thermal_pressure()?),
                "gpu_source": "IOKit AGXAccelerator Device Utilization %"
            }));
        }
        Cmd::Completions { shell } => {
            clap_complete::generate(
                *shell,
                &mut Cli::command(),
                "padctl",
                &mut std::io::stdout(),
            );
            return Ok(());
        }
        Cmd::Manpage => {
            let man = clap_mangen::Man::new(Cli::command());
            let mut out = Vec::new();
            man.render(&mut out).context("rendering man page")?;
            std::io::stdout()
                .write_all(&out)
                .context("writing man page")?;
            return Ok(());
        }
        Cmd::Temp { json } => {
            let reader = temp::TempReader::new()?;
            let celsius = reader.read()?;
            if *json {
                print_json(&serde_json::json!({
                    "temperature": units::value(celsius),
                    "temperature_unit": units::current().name(),
                    "fahrenheit": units::Unit::Fahrenheit.convert(celsius),
                    "celsius": celsius,
                    "source": reader.source_name(),
                }))?;
            } else {
                println!("{} ({})", units::display(celsius), reader.source_name());
            }
            return Ok(());
        }
        Cmd::Doctor { json } => {
            // Doctor manages the device itself: a missing pad is a finding,
            // not a reason to abort the other checks.
            return doctor::run(&cli.selector(), *json);
        }
        Cmd::Sensors { json } => {
            let reader = temp::TempReader::new()?;
            let sensors = reader.sensors()?;
            if *json {
                let rows: Vec<serde_json::Value> = sensors
                    .iter()
                    .map(|r| {
                        serde_json::json!({
                            "name": r.name, "celsius": r.celsius,
                            "fahrenheit": units::Unit::Fahrenheit.convert(r.celsius),
                            "temperature": units::value(r.celsius),
                        })
                    })
                    .collect();
                print_json(&serde_json::json!({
                    "source": reader.source_name(),
                    "temperature_unit": units::current().name(),
                    "sensors": rows,
                }))?;
            } else {
                println!("source: {}", reader.source_name());
                for r in sensors {
                    println!("{:>8}  {}", units::display(r.celsius), r.name);
                }
            }
            return Ok(());
        }
        _ => {}
    }

    match cli.cmd {
        Cmd::Config { ref cmd } => {
            match cmd {
                ConfigCmd::Init { force } => {
                    let path = config::init(*force)?;
                    println!("wrote {}", path.display());
                }
                ConfigCmd::Path => println!("{}", config::path().display()),
                ConfigCmd::Show => {
                    let file = config::load()?;
                    let path = config::path();
                    println!(
                        "config file: {} ({})",
                        path.display(),
                        if file.is_some() {
                            "present"
                        } else {
                            "absent, using defaults"
                        }
                    );
                    let s = curve::resolve(
                        &curve::CurveArgs::default(),
                        file.as_ref().map(|c| &c.curve),
                    )?;
                    println!("curve points (Celsius config): {}", s.points_text);
                    println!(
                        "display curve: {}",
                        s.points
                            .iter()
                            .map(|&(t, rpm)| format!("{}:{rpm}", units::display(t)))
                            .collect::<Vec<_>>()
                            .join(",")
                    );
                    println!(
                        "GPU points:   {}",
                        s.gpu_points
                            .iter()
                            .map(|(p, rpm)| format!("{p}:{rpm}"))
                            .collect::<Vec<_>>()
                            .join(",")
                    );
                    if !s.mac_fan_points.is_empty() {
                        println!("Mac fan RPM → pad RPM: {:?}", s.mac_fan_points);
                    }
                    println!("interval:     {}s", s.interval);
                    println!("on exit:      {:?}", s.on_exit);
                    println!("smooth:       {}s", s.smooth);
                    println!("down delay:   {}s", s.down_delay);
                    let lighting = match &file {
                        Some(c) => lighting::plan(&c.lighting)?,
                        None => None,
                    };
                    println!(
                        "lighting:     {}",
                        lighting
                            .map(|p| p.summary)
                            .unwrap_or_else(|| "not configured".into())
                    );
                }
            }
            return Ok(());
        }
        Cmd::Service { cmd } => return service::run(cmd),
        _ => {}
    }

    let mut api = device::api()?;
    let selector = cli.selector();
    let opts = cli.open_opts();

    match cli.cmd {
        Cmd::List => {
            let rows = device::list(&api, &selector);
            if rows.is_empty() {
                bail!("no Razer Laptop Cooling Pad (1532:0f43) found");
            }
            for row in rows {
                println!("{row}");
            }
        }
        Cmd::Fan { cmd } => {
            let pad = Pad::open(&api, &selector, opts)?;
            match cmd {
                FanCmd::Set { speed } => match parse::parse_speed(&speed)? {
                    Speed::Off => {
                        pad.send(&fan::off())?;
                        println!("fan off");
                    }
                    Speed::Rpm(rpm) => {
                        pad.send(&fan::set_rpm(rpm))?;
                        println!("fan set to {} RPM", fan::normalize_rpm(rpm));
                    }
                },
                FanCmd::Off => {
                    pad.send(&fan::off())?;
                    println!("fan off");
                }
                FanCmd::Get { json } => {
                    let report = pad.read_report()?;
                    let rpm = fan::rpm_from_report(&report).context(
                        "fan command readback unavailable: last response is not an acknowledged fan command; no tachometer measurement is available")?;
                    if json {
                        print_json(
                            &serde_json::json!({ "commanded_rpm": rpm, "off": rpm == 0, "measured_rpm": null }),
                        )?;
                    } else {
                        println!("{rpm} RPM commanded (not measured)");
                    }
                }
            }
        }
        Cmd::Rgb { cmd } => {
            let pad = Pad::open(&api, &selector, opts)?;
            match cmd {
                RgbCmd::Off => {
                    pad.send(&rgb::off())?;
                    println!("lighting off");
                }
                RgbCmd::Static { color } => {
                    let (r, g, b) = parse::parse_color(&color)?;
                    pad.send(&rgb::static_color(r, g, b))?;
                    println!("lighting set to static #{}", color.trim_start_matches('#'));
                }
                RgbCmd::Spectrum => {
                    pad.send(&rgb::spectrum())?;
                    println!("lighting set to spectrum");
                }
                RgbCmd::Wave { dir, speed } => {
                    pad.send(&rgb::wave(dir, speed))?;
                    println!("lighting set to wave ({dir:?}, speed {speed})");
                }
                RgbCmd::Breath { colors } => {
                    let packet = match colors.len() {
                        0 => rgb::breath_random(),
                        1 => {
                            let c = parse::parse_color(&colors[0])?;
                            rgb::breath_single(c.0, c.1, c.2)
                        }
                        2 => {
                            let c1 = parse::parse_color(&colors[0])?;
                            let c2 = parse::parse_color(&colors[1])?;
                            rgb::breath_dual(c1, c2)
                        }
                        n => bail!("breath takes 0, 1 or 2 colors, got {n}"),
                    };
                    pad.send(&packet)?;
                    println!("lighting set to breath");
                }
                RgbCmd::Brightness { percent: Some(pct) } => {
                    if pct > 100 {
                        bail!("brightness must be 0-100");
                    }
                    let raw = (pct as u16 * 255 / 100) as u8;
                    pad.send(&rgb::brightness_set(raw))?;
                    println!("brightness set to {pct}%");
                }
                RgbCmd::Brightness { percent: None } => {
                    let resp = pad.query(&rgb::brightness_get())?;
                    println!("brightness: {}%", resp.args[2] as u16 * 100 / 255);
                }
                RgbCmd::Custom {
                    colors,
                    driver_mode,
                } => {
                    if colors.len() > rgb::NUM_LEDS {
                        bail!("at most {} colors fit on the strip", rgb::NUM_LEDS);
                    }
                    let parsed: Vec<rgb::Rgb> = colors
                        .iter()
                        .map(|c| parse::parse_color(c))
                        .collect::<Result<_>>()?;
                    let frame = rgb::stretch(&parsed, rgb::NUM_LEDS);
                    send_custom_frame(&pad, &frame, driver_mode)?;
                    println!("lighting set to custom frame ({} colors)", colors.len());
                }
                RgbCmd::Gradient {
                    from,
                    to,
                    driver_mode,
                } => {
                    let from = parse::parse_color(&from)?;
                    let to = parse::parse_color(&to)?;
                    let frame = rgb::gradient(from, to, rgb::NUM_LEDS);
                    send_custom_frame(&pad, &frame, driver_mode)?;
                    println!("lighting set to gradient");
                }
                RgbCmd::Thermal(args) => reactive::run(pad, args)?,
                RgbCmd::Apply => {
                    let file = config::load()?.with_context(|| {
                        format!(
                            "no config file at {} (run `padctl config init`)",
                            config::path().display()
                        )
                    })?;
                    let plan = lighting::plan(&file.lighting)?
                        .context("the config has no [lighting] settings to apply")?;
                    anyhow::ensure!(
                        !plan.fan_status,
                        "fan-status is applied automatically by curve and watch"
                    );
                    for packet in &plan.packets {
                        pad.send(packet)?;
                    }
                    println!("lighting applied from config: {}", plan.summary);
                }
            }
        }
        Cmd::Info { json } => {
            let pad = Pad::open(&api, &selector, opts)?;
            let fw = pad.query(&rgb::firmware_version())?;
            let serial = pad.query(&rgb::serial())?;
            if json {
                print_json(&serde_json::json!({
                    "firmware": format!("{}.{}", fw.args[0], fw.args[1]),
                    "serial": rgb::serial_text(&serial),
                }))?;
            } else {
                println!("firmware: v{}.{}", fw.args[0], fw.args[1]);
                println!("serial:   {}", rgb::serial_text(&serial));
            }
        }
        Cmd::Status { json } => {
            let pad = Pad::open(&api, &selector, opts)?;
            // Read the fan report first: queries overwrite the report buffer.
            let report = pad.read_report()?;
            let rpm = fan::rpm_from_report(&report);
            let brightness = pad.query(&rgb::brightness_get())?;
            let brightness_pct = brightness.args[2] as u16 * 100 / 255;
            let fw = pad.query(&rgb::firmware_version())?;
            let serial = pad.query(&rgb::serial())?;
            let temp = temp::TempReader::new().and_then(|r| Ok((r.read()?, r.source_name())));
            if json {
                let (celsius, source) = match &temp {
                    Ok((t, s)) => (Some(*t), Some(*s)),
                    Err(_) => (None, None),
                };
                print_json(&serde_json::json!({
                    "fan_commanded_rpm": rpm,
                    "fan_measured_rpm": null,
                    "fan_off": rpm.map(|r| r == 0),
                    "fan_percent": rpm.filter(|r| *r != 0).map(fan::rpm_to_percent),
                    "brightness_percent": brightness_pct,
                    "firmware": format!("{}.{}", fw.args[0], fw.args[1]),
                    "serial": rgb::serial_text(&serial),
                    "die_temp_celsius": celsius,
                    "die_temp_fahrenheit": celsius.map(|t| units::Unit::Fahrenheit.convert(t)),
                    "die_temperature": celsius.map(units::value),
                    "temperature_unit": units::current().name(),
                    "temp_source": source,
                }))?;
            } else {
                match rpm {
                    Some(0) => println!("fan:        commanded off"),
                    Some(rpm) => println!("fan:        {rpm} RPM commanded (not measured)"),
                    None => println!(
                        "fan:        command readback unavailable (last reply was not a fan acknowledgement)"
                    ),
                }
                println!("brightness: {brightness_pct}%");
                println!("firmware:   v{}.{}", fw.args[0], fw.args[1]);
                println!("serial:     {}", rgb::serial_text(&serial));
                match temp {
                    Ok((t, source)) => println!("die temp:   {} ({source})", units::display(t)),
                    Err(e) => println!("die temp:   unavailable ({e:#})"),
                }
            }
        }
        Cmd::Raw {
            hex,
            auto_crc,
            read,
        } => {
            let pad = Pad::open(&api, &selector, opts)?;
            let cleaned: String = hex.chars().filter(|c| !c.is_whitespace()).collect();
            let raw = parse::hex_decode(&cleaned)?;
            if raw.len() > PACKET_LEN {
                bail!("packet is {} bytes, max {PACKET_LEN}", raw.len());
            }
            let mut report = [0u8; REPORT_LEN];
            report[1..1 + raw.len()].copy_from_slice(&raw);
            if auto_crc {
                report[1 + packet::CRC_OFFSET] = packet::crc(&report[1..]);
            }
            pad.send_raw_report(&report)?;
            println!("sent: {}", device::hex(&report[1..]));
            if read {
                std::thread::sleep(std::time::Duration::from_millis(100));
                let resp = pad.read_report()?;
                println!("read: {}", device::hex(&resp[1..]));
            }
        }
        Cmd::Curve(args) => {
            curve::run(&mut api, &selector, opts, args)?;
        }
        Cmd::Watch(args) => {
            watch::run(&mut api, &selector, opts, args, cli.verbose)?;
        }
        // Handled before the device was opened.
        Cmd::Config { .. }
        | Cmd::Telemetry
        | Cmd::Service { .. }
        | Cmd::Completions { .. }
        | Cmd::Manpage
        | Cmd::Temp { .. }
        | Cmd::Sensors { .. }
        | Cmd::Doctor { .. } => unreachable!(),
    }
    Ok(())
}

fn print_json(value: &serde_json::Value) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

/// Store a full-strip custom frame and switch the pad to display it.
fn send_custom_frame(pad: &Pad, frame: &[rgb::Rgb], driver_mode: bool) -> Result<()> {
    if driver_mode {
        pad.send(&rgb::device_mode(0x03))?;
    }
    pad.send(&rgb::custom_frame(0, frame))?;
    pad.send(&rgb::custom_apply())?;
    Ok(())
}
