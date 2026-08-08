use anyhow::Result;

use clap::{ Parser, Subcommand };

use serde::{ Deserialize, Serialize };

use std::{ io::{ BufRead, BufReader, Write }, os::unix::net::UnixStream };

const SOCKET: &str = "/tmp/displayd.sock";

#[derive(Parser)]
#[command(name = "displayctl")]
#[command(about = "Control DDC/CI displays")]
struct Cli {
    #[arg(short, long, help = "Show detailed output")]
    verbose: bool,

    #[arg(long, help = "Output JSON")]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Get or change brightness
    Brightness {
        #[command(subcommand)]
        action: Option<ValueCommand>,
    },

    /// Get or change contrast
    Contrast {
        #[command(subcommand)]
        action: Option<ValueCommand>,
    },

    /// Enable dim overlay
    Dim,

    /// Disable dim overlay
    Undim,

    /// Listen for display events
    Watch,
}

#[derive(Subcommand)]
enum ValueCommand {
    /// Set absolute value
    Set {
        value: u16,
    },

    /// Increase value
    Up {
        amount: u16,
    },

    /// Decrease value
    Down {
        amount: u16,
    },
}

#[derive(Debug, Serialize)]
struct Request {
    command: String,

    value: Option<u16>,
}

#[derive(Debug, Deserialize, Serialize)]
struct Response {
    current: u16,

    maximum: u16,

    percentage: f32,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Brightness { action } => {
            handle_feature("brightness", action, cli.verbose, cli.json)?;
        }

        Command::Contrast { action } => {
            handle_feature("contrast", action, cli.verbose, cli.json)?;
        }

        Command::Dim => {
            send_simple("dim")?;
        }

        Command::Undim => {
            send_simple("restore")?;
        }

        Command::Watch => {
            watch()?;
        }
    }

    Ok(())
}

fn handle_feature(
    command: &str,
    action: Option<ValueCommand>,
    verbose: bool,
    json: bool
) -> Result<()> {
    match action {
        None => {
            let response = send(Request {
                command: command.into(),
                value: None,
            })?;

            print_value(response, verbose, json);
        }

        Some(ValueCommand::Set { value }) => {
            let response = send(Request {
                command: command.into(),
                value: Some(value),
            })?;

            print_value(response, verbose, json);
        }

        Some(ValueCommand::Up { amount }) => {
            let current = send(Request {
                command: command.into(),
                value: None,
            })?;

            let value = (current.current + amount).min(current.maximum);

            let response = send(Request {
                command: command.into(),
                value: Some(value),
            })?;

            print_value(response, verbose, json);
        }

        Some(ValueCommand::Down { amount }) => {
            let current = send(Request {
                command: command.into(),
                value: None,
            })?;

            let value = current.current.saturating_sub(amount);

            let response = send(Request {
                command: command.into(),
                value: Some(value),
            })?;

            print_value(response, verbose, json);
        }
    }

    Ok(())
}

fn send_simple(command: &str) -> Result<()> {
    let response = send(Request {
        command: command.into(),
        value: None,
    })?;

    print_value(response, false, false);

    Ok(())
}

fn send(request: Request) -> Result<Response> {
    let mut stream = UnixStream::connect(SOCKET)?;

    let json = serde_json::to_string(&request)?;

    stream.write_all(json.as_bytes())?;

    stream.write_all(b"\n")?;

    let mut reader = BufReader::new(stream);

    let mut line = String::new();

    reader.read_line(&mut line)?;

    Ok(serde_json::from_str(&line)?)
}

fn watch() -> Result<()> {
    let mut stream = UnixStream::connect(SOCKET)?;

    let request = serde_json::json!({
            "command": "subscribe"
        });

    stream.write_all(request.to_string().as_bytes())?;

    stream.write_all(b"\n")?;

    let reader = BufReader::new(stream);

    for line in reader.lines() {
        println!("{}", line?);
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
