use anyhow::{ anyhow, Result };

use serde::{ Deserialize, Serialize };

use std::{ collections::HashMap, fs, path::{ Path, PathBuf }, sync::Arc, time::Instant };

use tokio::{
    io::{ AsyncBufReadExt, AsyncWriteExt, BufReader },
    net::{ UnixListener, UnixStream },
    sync::{ mpsc, Mutex },
};

use ddcci::{ discovery::find_monitor, feature::Feature, transport::LinuxI2cTransport, DdcDevice };

type Display = Arc<Mutex<DisplayState>>;

type Ddc = DdcDevice<LinuxI2cTransport>;

#[derive(Debug, Deserialize)]
struct Request {
    id: Option<u64>,

    command: String,

    value: Option<u16>,

    name: Option<String>,

    factor: Option<f32>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(tag = "type")]
enum Message {
    #[serde(rename = "response")] Response {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<u64>,

        current: u16,

        maximum: u16,

        percentage: f32,
    },

    #[serde(rename = "event")] Event {
        event: String,

        current: u16,

        effective: u16,
    },

    #[serde(rename = "error")] Error {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<u64>,

        error: String,
    },
}

struct Monitor {
    path: PathBuf,
    device: Ddc,
}

impl Monitor {
    fn open(path: PathBuf) -> Result<(Self, u16)> {
        let transport = LinuxI2cTransport::open(path.clone())?;

        let mut device = DdcDevice::new(transport);

        let brightness = device.get_vcp(Feature::Brightness)?.current;

        Ok((
            Self {
                path,
                device,
            },
            brightness,
        ))
    }

    fn from_discovery(path: PathBuf) -> Result<Self> {
        let transport = LinuxI2cTransport::open(path.clone())?;

        let device = DdcDevice::new(transport);

        Ok(Self {
            path,
            device,
        })
    }

    fn set_brightness_with_recovery(&mut self, hardware: u16) -> Result<()> {
        match self.device.set_vcp(Feature::Brightness, hardware) {
            Ok(()) => Ok(()),

            Err(first_error) => {
                eprintln!("DDC write failed on {}: {}", self.path.display(), first_error);

                self.reconnect()?;

                self.device.set_vcp(Feature::Brightness, hardware)?;

                Ok(())
            }
        }
    }

    fn reconnect(&mut self) -> Result<()> {
        eprintln!("Rediscovering monitor...");

        let (path, device, _brightness) = discover_and_open_monitor()?;

        println!("Using monitor: {}", path.display());

        self.path = path;
        self.device = device;

        Ok(())
    }
}

struct DisplayState {
    monitor: Monitor,

    brightness: u16,

    modifiers: HashMap<String, f32>,

    subscribers: Vec<mpsc::UnboundedSender<Message>>,
}

fn cache_path() -> Result<PathBuf> {
    let base = if let Some(path) = std::env::var_os("XDG_CACHE_HOME") {
        PathBuf::from(path)
    } else if let Some(home) = std::env::var_os("HOME") {
        PathBuf::from(home).join(".cache")
    } else {
        return Err(anyhow!("Could not determine cache directory"));
    };

    Ok(base.join("displayd").join("monitor"))
}

fn load_cached_path() -> Option<PathBuf> {
    let cache = cache_path().ok()?;

    let contents = fs::read_to_string(cache).ok()?;

    let path = PathBuf::from(contents.trim());

    if path.as_os_str().is_empty() {
        return None;
    }

    Some(path)
}

fn save_cached_path(path: &Path) -> Result<()> {
    let cache = cache_path()?;

    let parent = cache.parent().ok_or_else(|| anyhow!("Invalid cache path"))?;

    fs::create_dir_all(parent)?;

    let temporary = parent.join("monitor.tmp");

    fs::write(&temporary, format!("{}\n", path.display()))?;

    fs::rename(temporary, cache)?;

    Ok(())
}

fn discover_and_open_monitor() -> Result<(PathBuf, Ddc, u16)> {
    if let Some(path) = load_cached_path() {
        println!("Trying cached monitor: {}", path.display());

        match Monitor::open(path.clone()) {
            Ok((monitor, brightness)) => {
                println!("Cached monitor is valid: {}", path.display());

                return Ok((monitor.path, monitor.device, brightness));
            }

            Err(error) => {
                eprintln!("Cached monitor failed: {}", error);

                eprintln!("Performing full monitor discovery...");
            }
        }
    } else {
        println!("No cached monitor found; performing discovery...");
    }

    let discovered = find_monitor()?.ok_or_else(|| { anyhow!("No DDC/CI monitor found") })?;

    println!("Found monitor: {}", discovered.path.display());

    println!("Current brightness: {}", discovered.brightness);

    // no brightness query -> already done from the probe request
    let monitor = Monitor::from_discovery(discovered.path.clone())?;

    if let Err(error) = save_cached_path(&discovered.path) {
        eprintln!("Failed to update monitor cache: {}", error);
    }

    Ok((monitor.path, monitor.device, discovered.brightness))
}

fn effective_brightness(state: &DisplayState) -> u16 {
    let factor: f32 = state.modifiers.values().product();

    ((state.brightness as f32) * factor).round().clamp(0.0, 100.0) as u16
}

fn apply_brightness(state: &mut DisplayState) -> Result<()> {
    let hardware = effective_brightness(state);

    state.monitor.set_brightness_with_recovery(hardware)?;

    Ok(())
}

fn notify(state: &mut DisplayState, name: &str) {
    let event = Message::Event {
        event: name.to_string(),

        current: state.brightness,

        effective: effective_brightness(state),
    };

    state.subscribers.retain(|subscriber| { subscriber.send(event.clone()).is_ok() });
}

async fn execute(request: Request, device: Display) -> Result<Message> {
    let mut state = device.lock().await;

    let id = request.id;

    match request.command.as_str() {
        "brightness" => {
            if let Some(value) = request.value {
                state.brightness = value;

                apply_brightness(&mut state)?;

                notify(&mut state, "brightness_changed");
            }

            Ok(Message::Response {
                id,

                current: state.brightness,

                maximum: 100,

                percentage: state.brightness as f32,
            })
        }

        "dim" => {
            let name = request.name.unwrap_or_else(|| "default".into());

            let factor = request.factor.unwrap_or(0.1);

            if !factor.is_finite() {
                return Err(anyhow!("factor must be finite"));
            }

            state.modifiers.insert(name, factor);

            apply_brightness(&mut state)?;

            notify(&mut state, "dim_changed");

            Ok(Message::Response {
                id,

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

            Ok(Message::Response {
                id,

                current: state.brightness,

                maximum: 100,

                percentage: state.brightness as f32,
            })
        }

        _ => { Err(anyhow!("Unknown command: {}", request.command)) }
    }
}

async fn write_message(
    write: &mut tokio::net::unix::OwnedWriteHalf,
    message: &Message
) -> Result<()> {
    let json = serde_json::to_string(message)?;

    write.write_all(json.as_bytes()).await?;

    write.write_all(b"\n").await?;

    Ok(())
}

async fn handle_client(stream: UnixStream, device: Display) -> Result<()> {
    let (read, mut write) = stream.into_split();

    let mut reader = BufReader::new(read);

    let mut line = String::new();

    let bytes = reader.read_line(&mut line).await?;

    if bytes == 0 {
        return Ok(());
    }

    println!("Received command: {}", line.trim());

    let request: Request = serde_json
        ::from_str(&line)
        .map_err(|error| { anyhow!("Invalid request: {}", error) })?;

    if request.command == "subscribe" {
        let (tx, mut rx) = mpsc::unbounded_channel();

        {
            let mut state = device.lock().await;

            state.subscribers.push(tx);
        }

        println!("New subscriber");

        write_message(
            &mut write,
            &(Message::Response {
                id: request.id,

                current: {
                    let state = device.lock().await;

                    state.brightness
                },

                maximum: 100,

                percentage: {
                    let state = device.lock().await;

                    state.brightness as f32
                },
            })
        ).await?;

        while let Some(event) = rx.recv().await {
            write_message(&mut write, &event).await?;
        }

        return Ok(());
    }

    println!("Executing {}", request.command);

    let start = Instant::now();

    let request_id = request.id;

    match execute(request, device).await {
        Ok(response) => {
            println!("Command completed in {:?}", start.elapsed());

            write_message(&mut write, &response).await?;
        }

        Err(error) => {
            eprintln!("Command failed after {:?}: {}", start.elapsed(), error);

            let response = Message::Error {
                id: request_id,

                error: error.to_string(),
            };

            write_message(&mut write, &response).await?;
        }
    }

    Ok(())
}

fn socket_path() -> Result<PathBuf> {
    let runtime = std::env
        ::var_os("XDG_RUNTIME_DIR")
        .ok_or_else(|| { anyhow!("XDG_RUNTIME_DIR is not set") })?;

    Ok(PathBuf::from(runtime).join("displayd.sock"))
}

#[tokio::main]
async fn main() -> Result<()> {
    let socket = socket_path()?;

    if socket.exists() {
        fs::remove_file(&socket)?;
    }

    let (path, device, brightness) = discover_and_open_monitor()?;

    println!("Using monitor: {}", path.display());

    let monitor = Monitor {
        path,
        device,
    };

    let state = Arc::new(
        Mutex::new(DisplayState {
            monitor,

            brightness,

            modifiers: HashMap::new(),

            subscribers: Vec::new(),
        })
    );

    let listener = UnixListener::bind(&socket)?;

    println!("Listening on {}", socket.display());

    loop {
        let (stream, _) = listener.accept().await?;

        let state = state.clone();

        tokio::spawn(async move {
            if let Err(error) = handle_client(stream, state).await {
                eprintln!("client error: {}", error);
            }
        });
    }
}
