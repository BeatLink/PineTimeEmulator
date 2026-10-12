// SPDX-License-Identifier: GPL-3.0-only
//! `pinetime-engine`: runs an emulated PineTime and takes monitor commands over TCP or stdin.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use pinetime_core::time::{cycles_from_milliseconds, nanoseconds_from_cycles};
use pinetime_emulator::monitor::{default_hci_controller, Monitor, Reply};

// ============================================================================
// Command line
// ============================================================================

const USAGE: &str = "usage: pinetime-engine [--port PORT] [--bluetooth LINK [--hci-controller ELF]]
                       [--execute COMMAND]... [SCRIPT]

Runs an emulated PineTime. Commands come from SCRIPT, each --execute, then the TCP port (one
connection at a time is fine, several work) or, without --port, standard input. Each reply ends
with the prompt line `(pinetime) `.

--bluetooth adds the Bluetooth bridge, an emulated controller whose HCI is a pseudo-terminal
behind the symlink LINK, for `btattach -B LINK -P h4 -S 1000000`. It runs ELF, or the firmware
$PINETIME_EMULATOR_HCI_CONTROLLER names, and stays across `mach create`.";

pub const PROMPT: &str = "(pinetime) ";

struct Options {
    port: Option<u16>,
    commands: Vec<String>,
    bluetooth_link: Option<PathBuf>,
    hci_controller: Option<PathBuf>,
}

fn parse_options() -> Result<Options, String> {
    let mut options = Options {
        port: None,
        commands: Vec::new(),
        bluetooth_link: None,
        hci_controller: None,
    };
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--port" | "-P" => {
                let port = arguments.next().ok_or("--port needs a number")?;
                options.port = Some(port.parse().map_err(|_| format!("not a port: {port}"))?);
            }
            "--bluetooth" => {
                options.bluetooth_link =
                    Some(arguments.next().ok_or("--bluetooth needs a link path")?.into());
            }
            "--hci-controller" => {
                options.hci_controller =
                    Some(arguments.next().ok_or("--hci-controller needs a file")?.into());
            }
            "--execute" | "-e" => options
                .commands
                .push(arguments.next().ok_or("--execute needs a command")?),
            "--help" | "-h" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            "--disable-gui" | "--plain" | "--console" | "--disable-xwt" => {}
            script if !script.starts_with('-') => options.commands.insert(0, format!("include @{script}")),
            unknown => return Err(format!("unknown option {unknown}\n\n{USAGE}")),
        }
    }
    if options.hci_controller.is_some() && options.bluetooth_link.is_none() {
        return Err("--hci-controller needs --bluetooth".into());
    }
    Ok(options)
}

fn main() {
    let options = match parse_options() {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    };

    let (sender, receiver) = mpsc::channel::<Request>();
    match options.port {
        Some(port) => listen_on(port, sender),
        None => read_standard_input(sender),
    }

    let mut monitor = Monitor::new();
    if let Some(link) = &options.bluetooth_link {
        if let Err(message) = attach_bluetooth(&mut monitor, link, options.hci_controller.as_deref()) {
            eprintln!("--bluetooth: {message}");
            std::process::exit(1);
        }
    }
    for command in &options.commands {
        match monitor.execute(command) {
            Ok(output) if !output.is_empty() => println!("{output}"),
            Ok(_) => {}
            Err(message) => eprintln!("{command}: {message}"),
        }
    }
    run_engine(&mut monitor, receiver);
}

/// Attaches the bridge before any command runs, so scripts and clients find it in place.
fn attach_bluetooth(monitor: &mut Monitor, link: &Path, controller: Option<&Path>) -> Result<(), String> {
    let controller = match controller {
        Some(path) => path.to_path_buf(),
        None => default_hci_controller()?,
    };
    monitor.attach_bluetooth(link, &controller)?;
    eprintln!(
        "pinetime-engine: the Bluetooth controller's HCI is at {} ({})",
        link.display(),
        monitor
            .bluetooth
            .as_ref()
            .map(|bridge| bridge.terminal.terminal_path().display().to_string())
            .unwrap_or_default()
    );
    Ok(())
}

// ============================================================================
// Command sources
// ============================================================================

struct Request {
    line: String,
    reply_to: Sender<Reply>,
}

fn listen_on(port: u16, sender: Sender<Request>) {
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("could not listen on port {port}: {error}");
            std::process::exit(1);
        }
    };
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let sender = sender.clone();
            thread::spawn(move || serve_connection(stream, sender));
        }
    });
}

fn serve_connection(stream: TcpStream, sender: Sender<Request>) {
    let _ = stream.set_nodelay(true);
    let Ok(mut writer) = stream.try_clone() else {
        return;
    };
    let _ = writer.write_all(PROMPT.as_bytes());
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else {
            return;
        };
        let reply = send_request(&sender, line);
        if writer.write_all(format_reply(reply).as_bytes()).is_err() {
            return;
        }
    }
}

fn read_standard_input(sender: Sender<Request>) {
    thread::spawn(move || {
        let standard_input = std::io::stdin();
        for line in standard_input.lock().lines() {
            let Ok(line) = line else {
                break;
            };
            let reply = send_request(&sender, line);
            print!("{}", format_reply(reply));
            let _ = std::io::stdout().flush();
        }
        let _ = send_request(&sender, "quit".into());
    });
}

fn send_request(sender: &Sender<Request>, line: String) -> Reply {
    let (reply_to, reply) = mpsc::channel();
    if sender.send(Request { line, reply_to }).is_err() {
        return Err("the emulator has stopped".into());
    }
    reply
        .recv()
        .unwrap_or_else(|_| Err("the emulator has stopped".into()))
}

fn format_reply(reply: Reply) -> String {
    match reply {
        Ok(text) if text.is_empty() => PROMPT.to_string(),
        Ok(text) => format!("{text}\n{PROMPT}"),
        Err(message) => format!("error: {message}\n{PROMPT}"),
    }
}

// ============================================================================
// Engine loop
// ============================================================================

/// Emulated time run between looks at the command queue while the watch runs freely.
const SLICE_MILLISECONDS: u64 = 2;
const SHARE_INTERVAL: Duration = Duration::from_millis(16);
const IDLE_WAIT: Duration = Duration::from_millis(20);

/// Runs commands as they come; between them, a started watch keeps pace with the wall clock.
fn run_engine(monitor: &mut Monitor, receiver: Receiver<Request>) {
    let mut pace = Pace::new(monitor.machine.now());
    let mut last_share = Instant::now();

    while !monitor.quit_requested {
        if monitor.started {
            while let Ok(request) = receiver.try_recv() {
                handle_request(monitor, request);
                pace = Pace::new(monitor.machine.now());
            }
            if !monitor.started {
                continue;
            }
            let target = monitor.machine.now() + cycles_from_milliseconds(SLICE_MILLISECONDS);
            monitor.run_until(target);
            pace.wait_for_wall_clock(monitor.machine.now());
        } else {
            match receiver.recv_timeout(IDLE_WAIT) {
                Ok(request) => {
                    handle_request(monitor, request);
                    pace = Pace::new(monitor.machine.now());
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }

        if last_share.elapsed() >= SHARE_INTERVAL {
            monitor.share_now();
            last_share = Instant::now();
        }
    }
}

fn handle_request(monitor: &mut Monitor, request: Request) {
    let reply = monitor.execute(&request.line);
    monitor.share_now();
    let _ = request.reply_to.send(reply);
}

/// Keeps emulated time from running ahead of the wall clock.
struct Pace {
    wall_start: Instant,
    emulated_start: u64,
}

impl Pace {
    fn new(emulated_now: u64) -> Pace {
        Pace {
            wall_start: Instant::now(),
            emulated_start: emulated_now,
        }
    }

    fn wait_for_wall_clock(&mut self, emulated_now: u64) {
        let emulated = Duration::from_nanos(nanoseconds_from_cycles(emulated_now - self.emulated_start));
        let wall = self.wall_start.elapsed();
        if emulated > wall {
            thread::sleep(emulated - wall);
        } else if wall - emulated > Duration::from_millis(250) {
            *self = Pace::new(emulated_now);
        }
    }
}
