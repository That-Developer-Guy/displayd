use anyhow::{ anyhow, Result };

use clap::{ Parser, Subcommand };

use serde::{ Deserialize, Serialize };

use std::{ io::{ BufRead, BufReader, Write }, os::unix::net::UnixStream, path::PathBuf };

#[derive(Debug, Deserialize, Serialize)]
struct ListMonitor {
    index: usize,
    path: String,
    brightness: u16,
}

#[derive(Parser)]
#[command(name = "displayctl")]
#[command(about = "Control DDC/CI displays")]
struct Cli {
    #[arg(
        short = 'm',
        long = "monitor",
        default_value_t = 0,
        global = true,
        help = "Monitor index (zero-based)"
    )]
    monitor: usize,

    #[arg(short, long, help = "Show detailed output")]
    verbose: bool,

    #[arg(long, help = "Output JSON")]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Brightness {
        #[command(subcommand)]
        action: Option<ValueCommand>,
    },

    Contrast {
        #[command(subcommand)]
        action: Option<ValueCommand>,
    },

    Dim,

    Undim,

    Watch,

    List,
}

#[derive(Subcommand)]
enum ValueCommand {
    Set {
        value: u16,
    },

    Up {
        amount: u16,
    },

    Down {
        amount: u16,
    },
}

#[derive(Debug, Serialize)]
struct Request {
    command: String,

    monitor: Option<usize>,

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
    monitor: usize,
    verbose: bool,
    json: bool
) -> Result<()> {
    match action {
        None => {
            let response = send_response(Request {
                command: command.into(),
                monitor: Some(monitor),
                value: None,
            })?;

            print_value(response, verbose, json);
        }

        Some(ValueCommand::Set { value }) => {
            let response = send_response(Request {
                command: command.into(),
                monitor: Some(monitor),
                value: Some(value),
            })?;

            print_value(response, verbose, json);
        }

        Some(ValueCommand::Up { amount }) => {
            let current = send_response(Request {
                command: command.into(),
                monitor: Some(monitor),
                value: None,
            })?;

            let value = current.current.saturating_add(amount).min(current.maximum);

            let response = send_response(Request {
                command: command.into(),
                monitor: Some(monitor),
                value: Some(value),
            })?;

            print_value(response, verbose, json);
        }

        Some(ValueCommand::Down { amount }) => {
            let current = send_response(Request {
                command: command.into(),
                monitor: Some(monitor),
                value: None,
            })?;

            let value = current.current.saturating_sub(amount);

            let response = send_response(Request {
                command: command.into(),
                monitor: Some(monitor),
                value: Some(value),
            })?;

            print_value(response, verbose, json);
        }
    }

    Ok(())
}

fn send_simple(command: &str, monitor: usize) -> Result<()> {
    let response = send_response(Request {
        command: command.into(),
        monitor: Some(monitor),
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

fn watch(monitor: usize) -> Result<()> {
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
        if verbose {
            println!("{}: {} (brightness: {}%)", monitor.index, monitor.path, monitor.brightness);
        } else {
            println!("{}: {}", monitor.index, monitor.path);
        }
    }

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
    }

    Ok(())
}
