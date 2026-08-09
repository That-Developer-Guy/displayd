use anyhow::{ anyhow, Result };
use serde::{ Deserialize, Serialize };

use std::{ collections::HashMap, fs, path::PathBuf, sync::Arc, time::Instant };

use tokio::{
    io::{ AsyncBufReadExt, AsyncWriteExt, BufReader },
    net::{ UnixListener, UnixStream },
    sync::{ mpsc, Mutex },
};

use ddcci::{ discovery::find_monitors, feature::Feature, transport::LinuxI2cTransport, DdcDevice };

type Display = Arc<Mutex<DisplayState>>;

type Ddc = DdcDevice<LinuxI2cTransport>;

#[derive(Debug, Deserialize)]
struct Request {
    id: Option<u64>,

    monitor: Option<usize>,

    command: String,

    value: Option<u16>,

    name: Option<String>,

    factor: Option<f32>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct ListMonitor {
    index: usize,
    path: String,
    brightness: u16,
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

    #[serde(rename = "list")] List {
        monitors: Vec<ListMonitor>,
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
        let original_path = self.path.clone();

        eprintln!("Reconnecting monitor {}...", original_path.display());

        match LinuxI2cTransport::probe(&original_path) {
            Ok(Some(_discovered)) => {
                let monitor = Monitor::from_discovery(original_path.clone())?;

                self.path = monitor.path;
                self.device = monitor.device;

                println!("Reconnected monitor: {}", original_path.display());

                return Ok(());
            }

            Ok(None) => {
                eprintln!("No DDC/CI monitor found at {}", original_path.display());
            }

            Err(error) => {
                eprintln!("Probe failed for {}: {}", original_path.display(), error);
            }
        }

        eprintln!("Performing full monitor discovery...");

        let discovered = find_monitors()?;

        let discovered = discovered
            .into_iter()
            .find(|monitor| monitor.path == original_path)
            .ok_or_else(|| {
                anyhow!("Could not rediscover monitor {}", original_path.display())
            })?;

        let path = discovered.path.clone();

        let monitor = Monitor::from_discovery(path.clone())?;

        println!("Rediscovered monitor: {}", path.display());

        self.path = path;
        self.device = monitor.device;

        Ok(())
    }
}

struct MonitorState {
    monitor: Monitor,

    brightness: u16,

    maximum: u16,

    modifiers: HashMap<String, f32>,

    subscribers: Vec<mpsc::UnboundedSender<Message>>,
}

struct DisplayState {
    monitors: Vec<MonitorState>,
}

fn cache_path() -> Result<PathBuf> {
    let base = if let Some(path) = std::env::var_os("XDG_CACHE_HOME") {
        PathBuf::from(path)
    } else if let Some(home) = std::env::var_os("HOME") {
        PathBuf::from(home).join(".cache")
    } else {
        return Err(anyhow!("Could not determine cache directory"));
    };

    Ok(base.join("displayd").join("monitors"))
}

fn load_cached_paths() -> Vec<PathBuf> {
    let cache = match cache_path() {
        Ok(path) => path,
        Err(_) => {
            return Vec::new();
        }
    };

    let contents = match fs::read_to_string(cache) {
        Ok(contents) => contents,
        Err(_) => {
            return Vec::new();
        }
    };

    contents
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .collect()
}

fn save_cached_paths(paths: &[PathBuf]) -> Result<()> {
    let cache = cache_path()?;

    let parent = cache.parent().ok_or_else(|| anyhow!("Invalid cache path"))?;

    fs::create_dir_all(parent)?;

    let temporary = parent.join("monitors.tmp");

    let contents = paths
        .iter()
        .map(|path| path.to_string_lossy())
        .collect::<Vec<_>>()
        .join("\n");

    fs::write(&temporary, format!("{}\n", contents))?;

    fs::rename(temporary, cache)?;

    Ok(())
}

fn monitor_state_from_probe(path: PathBuf, brightness: u16, maximum: u16) -> Result<MonitorState> {
    let monitor = Monitor::from_discovery(path)?;

    Ok(MonitorState {
        monitor,
        brightness,
        maximum,
        modifiers: HashMap::new(),
        subscribers: Vec::new(),
    })
}

fn discover_monitors() -> Result<Vec<MonitorState>> {
    let cached_paths = load_cached_paths();

    if !cached_paths.is_empty() {
        println!("Trying {} cached monitor(s)...", cached_paths.len());

        let mut monitors = Vec::new();
        let mut valid_paths = Vec::new();

        for path in cached_paths {
            println!("Probing cached monitor: {}", path.display());

            match LinuxI2cTransport::probe(&path) {
                Ok(Some(discovered)) => {
                    println!("Cached monitor is valid: {}", discovered.path.display());

                    println!("Current brightness: {}", discovered.brightness);

                    let path = discovered.path.clone();

                    monitors.push(
                        monitor_state_from_probe(
                            path.clone(),
                            discovered.brightness,
                            discovered.maximum
                        )?
                    );

                    valid_paths.push(path);
                }

                Ok(None) => {
                    eprintln!("Cached device is not a DDC/CI monitor: {}", path.display());
                }

                Err(error) => {
                    eprintln!("Cached monitor failed: {}: {}", path.display(), error);
                }
            }
        }

        if !monitors.is_empty() {
            if let Err(error) = save_cached_paths(&valid_paths) {
                eprintln!("Failed to update monitor cache: {}", error);
            }

            return Ok(monitors);
        }

        eprintln!(
            "No cached monitors could be opened; \
             performing full monitor discovery..."
        );
    } else {
        println!("No cached monitors found; \
             performing full monitor discovery...");
    }

    let discovered = find_monitors()?;

    if discovered.is_empty() {
        return Err(anyhow!("No DDC/CI monitors found"));
    }

    let mut monitors = Vec::with_capacity(discovered.len());
    let mut paths = Vec::with_capacity(discovered.len());

    for (index, discovered) in discovered.into_iter().enumerate() {
        println!("Found monitor {}: {}", index, discovered.path.display());

        println!("Current brightness: {}", discovered.brightness);

        let path = discovered.path.clone();

        monitors.push(
            monitor_state_from_probe(path.clone(), discovered.brightness, discovered.maximum)?
        );

        paths.push(path);
    }

    if let Err(error) = save_cached_paths(&paths) {
        eprintln!("Failed to update monitor cache: {}", error);
    }

    Ok(monitors)
}

fn effective_brightness(state: &MonitorState) -> u16 {
    let factor: f32 = state.modifiers.values().product();

    ((state.brightness as f32) * factor).round().clamp(0.0, state.maximum as f32) as u16
}

fn apply_brightness(state: &mut MonitorState) -> Result<()> {
    let hardware = effective_brightness(state);

    state.monitor.set_brightness_with_recovery(hardware)?;

    Ok(())
}

fn notify(state: &mut MonitorState, name: &str) {
    let event = Message::Event {
        event: name.to_string(),
        current: state.brightness,
        effective: effective_brightness(state),
    };

    state.subscribers.retain(|subscriber| subscriber.send(event.clone()).is_ok());
}

fn brightness_percentage(state: &MonitorState) -> f32 {
    if state.maximum == 0 {
        return 0.0;
    }
    ((state.brightness as f32) / (state.maximum as f32)) * 100.0
}

fn response(id: Option<u64>, state: &MonitorState) -> Message {
    Message::Response {
        id,
        current: state.brightness,
        maximum: state.maximum,
        percentage: brightness_percentage(state),
    }
}

fn list_response(state: &DisplayState) -> Message {
    let monitors = state.monitors
        .iter()
        .enumerate()
        .map(|(index, monitor)| ListMonitor {
            index,
            path: monitor.monitor.path.display().to_string(),
            brightness: monitor.brightness,
        })
        .collect();

    Message::List { monitors }
}

async fn execute(request: Request, device: Display) -> Result<Message> {
    let mut state = device.lock().await;

    let id = request.id;

    if request.command == "list" {
        return Ok(list_response(&state));
    }

    let index = request.monitor.unwrap_or(0);

    let monitor_count = state.monitors.len();

    let monitor = state.monitors
        .get_mut(index)
        .ok_or_else(|| {
            anyhow!("Monitor {} does not exist ({} monitor(s) available)", index, monitor_count)
        })?;

    match request.command.as_str() {
        "brightness" => {
            if let Some(value) = request.value {
                if value > 100 {
                    return Err(anyhow!("brightness must be between 0 and 100"));
                }

                apply_brightness(monitor)?;

                monitor.brightness = value;

                notify(monitor, "brightness_changed");
            }

            Ok(response(id, monitor))
        }

        "dim" => {
            let name = request.name.unwrap_or_else(|| "default".into());

            let factor = request.factor.unwrap_or(0.1);

            if !factor.is_finite() {
                return Err(anyhow!("factor must be finite"));
            }

            if factor < 0.0 {
                return Err(anyhow!("factor must not be negative"));
            }

            apply_brightness(monitor)?;

            monitor.modifiers.insert(name, factor);

            notify(monitor, "dim_changed");

            Ok(response(id, monitor))
        }

        "restore" => {
            let name = request.name.unwrap_or_else(|| "default".into());

            apply_brightness(monitor)?;

            monitor.modifiers.remove(&name);

            notify(monitor, "restore");

            Ok(response(id, monitor))
        }

        _ => Err(anyhow!("Unknown command: {}", request.command)),
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
        .map_err(|error| anyhow!("Invalid request: {}", error))?;

    if request.command == "subscribe" {
        let index = request.monitor.unwrap_or(0);

        let (tx, mut rx) = mpsc::unbounded_channel();

        let initial_response = {
            let mut state = device.lock().await;

            let monitor_count = state.monitors.len();

            let monitor = state.monitors
                .get_mut(index)
                .ok_or_else(|| {
                    anyhow!(
                        "Monitor {} does not exist \
                         ({} monitor(s) available)",
                        index,
                        monitor_count
                    )
                })?;

            monitor.subscribers.push(tx);

            response(request.id, monitor)
        };

        println!("New subscriber for monitor {}", index);

        write_message(&mut write, &initial_response).await?;

        while let Some(event) = rx.recv().await {
            if let Err(error) = write_message(&mut write, &event).await {
                eprintln!("Subscriber for monitor {} disconnected: {}", index, error);

                break;
            }
        }

        return Ok(());
    }

    let monitor_text = request.monitor.map(|index| index.to_string()).unwrap_or_else(|| "0".into());

    println!("Executing {} on monitor {}", request.command, monitor_text);

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
        .ok_or_else(|| anyhow!("XDG_RUNTIME_DIR is not set"))?;

    Ok(PathBuf::from(runtime).join("displayd.sock"))
}

#[tokio::main]
async fn main() -> Result<()> {
    let socket = socket_path()?;

    if socket.exists() {
        fs::remove_file(&socket)?;
    }

    let monitors = discover_monitors()?;

    println!("Using {} monitor(s)", monitors.len());

    let state = Arc::new(
        Mutex::new(DisplayState {
            monitors,
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
