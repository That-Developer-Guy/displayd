use anyhow::{ anyhow, Result };

use clap::{ Parser, Subcommand };

use serde::{ Deserialize, Serialize };

use std::{ io::{ BufRead, BufReader, Write }, os::unix::net::UnixStream, path::PathBuf };

#[derive(Debug, Deserialize, Serialize)]
struct MonitorId {
    manufacturer: String,
    product: u16,
    serial: Option<u32>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ListMonitor {
    connector: String,
    path: String,
    name: Option<String>,
    id: MonitorId,
}

#[derive(Parser)]
#[command(name = "displayctl")]
#[command(about = "Control DDC/CI displays")]
struct Cli {
    #[arg(
        short = 'm',
        long = "monitor",
        global = true,
        help = "Monitor connector (for example DP-2)"
    )]
    monitor: Option<String>,

    #[arg(short, long, help = "Show detailed output")]
    verbose: bool,

    #[arg(long, help = "Output JSON")]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Show and modify the brightness level
    Brightness {
        #[command(subcommand)]
        action: Option<ValueCommand>,
    },

    /// Show and modify the contrast level
    Contrast {
        #[command(subcommand)]
        action: Option<ValueCommand>,
    },

    /// Apply dim (10% of brightness level)
    Dim,

    /// Reset dim (10% of brightness level)
    Undim,

    /// Listen for display changes
    Watch,

    /// List available monitors
    List,

    /// Request information about a monitor
    Info,
}

#[derive(Subcommand)]
enum ValueCommand {
    /// Set the value directly
    Set {
        /// Value to set
        value: u16,
    },

    /// Increase the value
    Up {
        /// Amount to increase by
        amount: u16,
    },

    /// Decrease the value
    Down {
        /// Amount to decrease by
        amount: u16,
    },
}
#[derive(Debug, Serialize)]
struct Request {
    command: String,

    monitor: Option<String>,

    value: Option<u16>,
}

#[derive(Debug, Deserialize, Serialize)]
struct Response {
    current: u16,

    maximum: u16,

    percentage: f32,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum Message {
    #[serde(rename = "response")] Response {
        current: u16,
        maximum: u16,
        percentage: f32,
    },

    #[serde(rename = "list")] List {
        monitors: Vec<ListMonitor>,
    },

    #[serde(rename = "info")] Info {
        monitor: ListMonitor,
    },

    #[serde(rename = "error")] Error {
        error: String,
    },
}

fn socket_path() -> Result<PathBuf> {
    let runtime_dir = std::env
        ::var_os("XDG_RUNTIME_DIR")
        .ok_or_else(|| anyhow!("XDG_RUNTIME_DIR is not set"))?;

    Ok(PathBuf::from(runtime_dir).join("displayd.sock"))
}

fn handle_feature(
    command: &str,
    action: Option<ValueCommand>,
    monitor: Option<String>,
    verbose: bool,
    json: bool
) -> Result<()> {
    match action {
        None => {
            let response = send_response(Request {
                command: command.into(),
                monitor: monitor.clone(),
                value: None,
            })?;

            print_value(response, verbose, json);
        }

        Some(ValueCommand::Set { value }) => {
            let response = send_response(Request {
                command: command.into(),
                monitor: monitor.clone(),
                value: Some(value),
            })?;

            print_value(response, verbose, json);
        }

        Some(ValueCommand::Up { amount }) => {
            let current = send_response(Request {
                command: command.into(),
                monitor: monitor.clone(),
                value: None,
            })?;

            let value = current.current.saturating_add(amount).min(current.maximum);

            let response = send_response(Request {
                command: command.into(),
                monitor: monitor.clone(),
                value: Some(value),
            })?;

            print_value(response, verbose, json);
        }

        Some(ValueCommand::Down { amount }) => {
            let current = send_response(Request {
                command: command.into(),
                monitor: monitor.clone(),
                value: None,
            })?;

            let value = current.current.saturating_sub(amount);

            let response = send_response(Request {
                command: command.into(),
                monitor: monitor,
                value: Some(value),
            })?;

            print_value(response, verbose, json);
        }
    }

    Ok(())
}

fn send_simple(command: &str, monitor: Option<String>) -> Result<()> {
    let response = send_response(Request {
        command: command.into(),
        monitor: monitor,
        value: None,
    })?;

    print_value(response, false, false);

    Ok(())
}

fn send_list() -> Result<Vec<ListMonitor>> {
    match
        send(Request {
            command: "list".into(),
            monitor: None,
            value: None,
        })?
    {
        Message::List { monitors } => Ok(monitors),

        Message::Error { error } => { Err(anyhow!("{}", error)) }

        other => { Err(anyhow!("Unexpected response from daemon: {:?}", other)) }
    }
}

fn send_info() -> Result<ListMonitor> {
    match
        send(Request {
            command: "info".into(),
            monitor: None,
            value: None,
        })?
    {
        Message::Info { monitor } => Ok(monitor),

        Message::Error { error } => { Err(anyhow!("{}", error)) }

        other => { Err(anyhow!("Unexpected response from daemon: {:?}", other)) }
    }
}

fn send(request: Request) -> Result<Message> {
    let socket = socket_path()?;
    let mut stream = UnixStream::connect(&socket)?;

    let json = serde_json::to_string(&request)?;

    stream.write_all(json.as_bytes())?;

    stream.write_all(b"\n")?;

    let mut reader = BufReader::new(stream);

    let mut line = String::new();

    reader.read_line(&mut line)?;

    if line.trim().is_empty() {
        return Err(anyhow!("Daemon returned an empty response"));
    }

    serde_json::from_str(&line).map_err(|error| anyhow!("Invalid response from daemon: {}", error))
}

fn send_response(request: Request) -> Result<Response> {
    match send(request)? {
        Message::Response { current, maximum, percentage } =>
            Ok(Response {
                current,
                maximum,
                percentage,
            }),

        Message::Error { error } => { Err(anyhow!("{}", error)) }

        other => { Err(anyhow!("Unexpected response from daemon: {:?}", other)) }
    }
}

fn watch(monitor: Option<String>) -> Result<()> {
    let socket = socket_path()?;
    let mut stream = UnixStream::connect(&socket)?;

    let request =
        serde_json::json!({
        "command": "subscribe",
        "monitor": monitor,
    });

    stream.write_all(request.to_string().as_bytes())?;

    stream.write_all(b"\n")?;

    let reader = BufReader::new(stream);

    for line in reader.lines() {
        println!("{}", line?);
    }

    Ok(())
}

fn list(verbose: bool, json: bool) -> Result<()> {
    let monitors = send_list()?;

    if monitors.is_empty() {
        if json {
            println!("[]");
        } else {
            println!("No DDC/CI monitors found.");
        }

        return Ok(());
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&monitors)?);

        return Ok(());
    }

    for monitor in monitors {
        let name = monitor.name.as_deref().unwrap_or("unnamed");

        let id = match monitor.id.serial {
            Some(serial) =>
                format!(
                    "{}, 0x{:04x} 0x{:08x}",
                    monitor.id.manufacturer,
                    monitor.id.product,
                    serial
                ),
            None => format!("{}, {}", monitor.id.manufacturer, monitor.id.product),
        };

        if verbose {
            println!("{}: {} ({}) [{}]", monitor.connector, name, id, monitor.path);
        } else {
            println!("{}: {} ({})", monitor.connector, name, id);
        }
    }

    Ok(())
}

fn info(json: bool) -> Result<()> {
    let monitor = send_info()?;

    if json {
        println!("{}", serde_json::to_string_pretty(&monitor)?);
        return Ok(());
    }

    println!("Monitor information:");
    println!("\tConnector: {}", monitor.connector);
    println!("\tName: {}", monitor.name.as_deref().unwrap_or("Unknown"));
    println!("\tManufacturer: {}", monitor.id.manufacturer);
    println!("\tProduct code: 0x{:04x}", monitor.id.product);

    match monitor.id.serial {
        Some(serial) => println!("\tSerial number: 0x{:08x}", serial),
        None => println!("\tSerial number: Unknown"),
    }

    println!("\tI²C path: {}", monitor.path);

    Ok(())
}

fn print_value(value: Response, verbose: bool, json: bool) {
    if json {
        println!("{}", serde_json::to_string(&value).unwrap());

        return;
    }

    if verbose {
        println!("{}/{} ({:.0}%)", value.current, value.maximum, value.percentage);
    } else {
        println!("{:.0}%", value.percentage);
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Brightness { action } => {
            handle_feature("brightness", action, cli.monitor, cli.verbose, cli.json)?;
        }

        Command::Contrast { action } => {
            handle_feature("contrast", action, cli.monitor, cli.verbose, cli.json)?;
        }

        Command::Dim => {
            send_simple("dim", cli.monitor)?;
        }

        Command::Undim => {
            send_simple("restore", cli.monitor)?;
        }

        Command::Watch => {
            watch(cli.monitor)?;
        }

        Command::List => {
            list(cli.verbose, cli.json)?;
        }

        Command::Info => {
            info(cli.json)?;
        }
    }

    Ok(())
}
