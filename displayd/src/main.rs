use anyhow::{ anyhow, Result };

use serde::{ Deserialize, Serialize };

use std::{ collections::HashMap, fs, path::Path, sync::Arc, time::Instant };

use tokio::{
    io::{ AsyncBufReadExt, AsyncWriteExt, BufReader },
    net::{ UnixListener, UnixStream },
    sync::{ mpsc, Mutex },
};

use ddcci::{ discovery::find_monitors, feature::Feature, transport::LinuxI2cTransport, DdcDevice };

const SOCKET: &str = "/tmp/displayd.sock";

type Display = Arc<Mutex<DisplayState>>;

struct DisplayState {
    device: DdcDevice<LinuxI2cTransport>,

    brightness: u16,

    modifiers: HashMap<String, f32>,

    subscribers: Vec<mpsc::UnboundedSender<Event>>,
}

#[derive(Debug, Deserialize)]
struct Request {
    command: String,

    value: Option<u16>,

    name: Option<String>,

    factor: Option<f32>,
}

#[derive(Debug, Serialize)]
struct Response {
    current: u16,

    maximum: u16,

    percentage: f32,
}

#[derive(Debug, Serialize, Clone)]
struct Event {
    event: String,

    current: u16,

    effective: u16,
}

#[tokio::main]
async fn main() -> Result<()> {
    if Path::new(SOCKET).exists() {
        fs::remove_file(SOCKET)?;
    }

    let path = find_monitors()?
        .first()
        .ok_or_else(|| anyhow!("No DDC/CI monitor found"))?
        .clone();

    println!("Using monitor: {}", path.display());

    let transport = LinuxI2cTransport::open(path)?;

    let mut device = DdcDevice::new(transport);

    let brightness = device.get_vcp(Feature::Brightness)?.current;

    let state = Arc::new(
        Mutex::new(DisplayState {
            device,
            brightness,
            modifiers: HashMap::new(),
            subscribers: Vec::new(),
        })
    );

    let listener = UnixListener::bind(SOCKET)?;

    println!("Listening on {}", SOCKET);

    loop {
        let (stream, _) = listener.accept().await?;

        let state = state.clone();

        tokio::spawn(async move {
            if let Err(e) = handle_client(stream, state).await {
                eprintln!("client error: {e}");
            }
        });
    }
}

async fn handle_client(stream: UnixStream, device: Display) -> Result<()> {
    let (read, mut write) = stream.into_split();

    let mut reader = BufReader::new(read);

    let mut line = String::new();

    reader.read_line(&mut line).await?;

    println!("Received command: {}", line.trim());

    let request: Request = serde_json::from_str(&line)?;

    if request.command == "subscribe" {
        let (tx, mut rx) = mpsc::unbounded_channel();

        {
            let mut state = device.lock().await;

            state.subscribers.push(tx);
        }

        println!("New subscriber");

        while let Some(event) = rx.recv().await {
            let json = serde_json::to_string(&event)?;

            write.write_all(json.as_bytes()).await?;

            write.write_all(b"\n").await?;
        }

        return Ok(());
    }

    println!("Executing {}", request.command);

    let start = Instant::now();

    let response = execute(request, device).await?;

    println!("Command completed in {:?}", start.elapsed());

    let json = serde_json::to_string(&response)?;

    write.write_all(json.as_bytes()).await?;

    write.write_all(b"\n").await?;

    Ok(())
}

fn effective_brightness(state: &DisplayState) -> u16 {
    let factor: f32 = state.modifiers.values().product();

    ((state.brightness as f32) * factor).round().clamp(0.0, 100.0) as u16
}

fn apply_brightness(state: &mut DisplayState) -> Result<()> {
    let hardware = effective_brightness(state);

    state.device.set_vcp(Feature::Brightness, hardware)?;

    Ok(())
}

fn notify(state: &mut DisplayState, name: &str) {
    let event = Event {
        event: name.to_string(),
        current: state.brightness,
        effective: effective_brightness(state),
    };

    state.subscribers.retain(|subscriber| { subscriber.send(event.clone()).is_ok() });
}

async fn execute(request: Request, device: Display) -> Result<Response> {
    let mut state = device.lock().await;

    match request.command.as_str() {
        "brightness" => {
            if let Some(value) = request.value {
                state.brightness = value;

                apply_brightness(&mut state)?;

                notify(&mut state, "brightness_changed");
            }

            Ok(Response {
                current: state.brightness,

                maximum: 100,

                percentage: state.brightness as f32,
            })
        }

        "dim" => {
            let name = request.name.unwrap_or_else(|| "default".into());

            let factor = request.factor.unwrap_or(0.1);

            state.modifiers.insert(name, factor);

            apply_brightness(&mut state)?;

            notify(&mut state, "dim_changed");

            Ok(Response {
                current: state.brightness,

                maximum: 100,

                percentage: state.brightness as f32,
            })
        }

        "restore" => {
            let name = request.name.unwrap_or_else(|| "default".into());

            state.modifiers.remove(&name);

            apply_brightness(&mut state)?;

            notify(&mut state, "restore");

            Ok(Response {
                current: state.brightness,

                maximum: 100,

                percentage: state.brightness as f32,
            })
        }

        _ => Err(anyhow!("Unknown command")),
    }
}
