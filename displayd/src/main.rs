use anyhow::{ anyhow, Result };
use serde::{ Deserialize, Serialize };

use std::{ collections::HashMap, fs, path::{ Path, PathBuf }, sync::Arc, thread, time::Instant };

use edid::{ parse::{ parse as parse_edid, EdidData }, read::read_edid };

use tokio::{
    io::{ AsyncBufReadExt, AsyncWriteExt, BufReader },
    net::{ UnixListener, UnixStream },
    sync::{ mpsc, oneshot },
};

use ddcci::{
    discovery::find_monitors,
    feature::Feature,
    protocol::VcpValue,
    transport::LinuxI2cTransport,
    DdcDevice,
};

type Ddc = DdcDevice<LinuxI2cTransport>;

#[derive(Debug, Deserialize)]
struct Request {
    id: Option<u64>,

    monitor: Option<String>,

    command: String,

    value: Option<u16>,

    name: Option<String>,

    factor: Option<f32>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct ListMonitor {
    connector: String,

    path: String,

    name: Option<String>,

    id: MonitorId,
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

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
struct MonitorId {
    manufacturer: String,
    product: u16,
    serial: Option<u32>,
}

impl MonitorId {
    fn from_edid(edid: &EdidData) -> Result<Self> {
        Ok(Self {
            manufacturer: edid.id.clone(),
            product: edid.product_code,
            serial: Some(edid.serial_number),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedMonitor {
    connector: String,

    path: PathBuf,
}

struct Monitor {
    id: MonitorId,

    connector: String,

    name: Option<String>,

    path: PathBuf,
    device: Ddc,
}

impl Monitor {
    fn from_discovery(path: PathBuf) -> Result<Self> {
        let transport = LinuxI2cTransport::open(path.clone())?;
        let device = DdcDevice::new(transport);

        let edid_bytes = read_edid(&path)?;
        let edid = parse_edid(&edid_bytes)?;

        let id = MonitorId::from_edid(&edid)?;
        let connector = find_drm_connector(&edid_bytes);

        Ok(Self {
            id,
            connector: connector?,
            name: edid.name,
            path,
            device,
        })
    }

    fn get_vcp_with_recovery(&mut self, feature: Feature) -> Result<VcpValue> {
        match self.device.get_vcp(feature) {
            Ok(value) => Ok(value),
            Err(first_error) => {
                eprintln!("DDC read failed on {}: {}", self.path.display(), first_error);
                self.reconnect()?;
                Ok(self.device.get_vcp(feature)?)
            }
        }
    }

    fn set_vcp_with_recovery(&mut self, feature: Feature, value: u16) -> Result<()> {
        match self.device.set_vcp(feature, value) {
            Ok(()) => Ok(()),

            Err(first_error) => {
                eprintln!("DDC write failed on {}: {}", self.path.display(), first_error);

                self.reconnect()?;

                self.device.set_vcp(feature, value)?;

                Ok(())
            }
        }
    }

    fn reconnect(&mut self) -> Result<()> {
        let original_path = self.path.clone();
        let connector = self.connector.clone();

        eprintln!("Reconnecting monitor {}...", connector);

        match LinuxI2cTransport::probe(&original_path) {
            Ok(Some(_discovered)) => {
                match Monitor::from_discovery(original_path.clone()) {
                    Ok(monitor) if monitor.connector == connector => {
                        self.path = monitor.path;
                        self.device = monitor.device;

                        println!(
                            "Reconnected monitor {} on {}",
                            connector,
                            original_path.display()
                        );

                        return Ok(());
                    }

                    Ok(monitor) => {
                        eprintln!(
                            "I2C path {} now belongs to connector {}, expected {}",
                            original_path.display(),
                            monitor.connector,
                            connector
                        );
                    }

                    Err(error) => {
                        eprintln!(
                            "Could not rediscover monitor on {}: {}",
                            original_path.display(),
                            error
                        );
                    }
                }
            }

            Ok(None) => {
                eprintln!("No DDC/CI monitor found at {}", original_path.display());
            }

            Err(error) => {
                eprintln!("Probe failed for {}: {}", original_path.display(), error);
            }
        }

        eprintln!("Performing full monitor discovery for {}...", connector);

        let discovered = find_monitors()?;

        let discovered = discovered
            .into_iter()
            .find_map(|monitor| {
                let path = monitor.path.clone();

                match Monitor::from_discovery(path) {
                    Ok(discovered_monitor) if discovered_monitor.connector == connector => {
                        Some(discovered_monitor)
                    }

                    _ => None,
                }
            })
            .ok_or_else(|| { anyhow!("Could not rediscover monitor {}", connector) })?;

        let path = discovered.path.clone();

        let device = discovered.device;

        println!("Rediscovered monitor {} on {}", connector, path.display());

        self.path = path;
        self.device = device;

        Ok(())
    }
}

fn find_drm_connector(edid: &[u8]) -> Result<String> {
    let drm = Path::new("/sys/class/drm");

    for entry in fs::read_dir(drm)? {
        let entry = entry?;
        let connector_path = entry.path();

        if !connector_path.is_dir() {
            continue;
        }

        let edid_path = connector_path.join("edid");

        let drm_edid = match fs::read(&edid_path) {
            Ok(edid) => edid,
            Err(_) => {
                continue;
            }
        };

        if drm_edid == edid {
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| anyhow!("DRM connector name is not valid UTF-8"))?;

            let connector = name
                .strip_prefix("card")
                .and_then(|rest| rest.split_once('-'))
                .map(|(_, connector)| connector)
                .ok_or_else(|| anyhow!("Invalid DRM connector name: {}", name))?;

            return Ok(connector.to_string());
        }
    }

    Err(anyhow!("Could not resolve DRM connector from EDID"))
}

struct MonitorState {
    monitor: Monitor,

    brightness: u16,

    brightness_maximum: u16,

    contrast: u16,

    contrast_maximum: u16,

    modifiers: HashMap<String, f32>,

    subscribers: Vec<mpsc::UnboundedSender<Message>>,
}

struct DisplayState {
    monitors: HashMap<String, MonitorHandle>,
    default_monitor: Option<String>,
}

impl DisplayState {
    fn get(&self, connector: &str) -> Result<&MonitorHandle, String> {
        self.monitors
            .get(connector)
            .ok_or_else(|| {
                format!(
                    "Monitor {} does not exist ({} monitor(s) available)",
                    connector,
                    self.monitors.len()
                )
            })
    }

    fn resolve(&self, connector: Option<&str>) -> Result<&MonitorHandle, String> {
        match connector {
            Some(connector) => self.get(connector),

            None => {
                let connector = self.default_monitor
                    .as_deref()
                    .ok_or_else(|| "No default monitor is available".to_string())?;

                self.get(connector)
            }
        }
    }
}

enum MonitorCommand {
    Brightness {
        id: Option<u64>,
        value: Option<u16>,
        reply: oneshot::Sender<Result<Message, String>>,
    },

    Contrast {
        id: Option<u64>,
        value: Option<u16>,
        reply: oneshot::Sender<Result<Message, String>>,
    },

    Dim {
        id: Option<u64>,
        name: String,
        factor: f32,
        reply: oneshot::Sender<Result<Message, String>>,
    },

    Restore {
        id: Option<u64>,
        name: String,
        reply: oneshot::Sender<Result<Message, String>>,
    },

    Subscribe {
        id: Option<u64>,
        sender: mpsc::UnboundedSender<Message>,
        reply: oneshot::Sender<Result<Message, String>>,
    },

    Info {
        reply: oneshot::Sender<Result<ListMonitor, String>>,
    },
}

struct MonitorHandle {
    connector: String,

    tx: mpsc::Sender<MonitorCommand>,
}

impl MonitorHandle {
    async fn send(&self, command: MonitorCommand) -> Result<(), String> {
        self.tx.send(command).await.map_err(|_| "Monitor worker has stopped".to_string())
    }
}

fn monitor_worker(
    connector: String,
    mut state: MonitorState,
    mut rx: mpsc::Receiver<MonitorCommand>
) {
    println!("Monitor worker for {} started for {}", connector, state.monitor.path.display());

    while let Some(command) = rx.blocking_recv() {
        match command {
            MonitorCommand::Brightness { id, value, reply } => {
                let result = handle_brightness(&mut state, id, value);

                let _ = reply.send(result);
            }

            MonitorCommand::Contrast { id, value, reply } => {
                let result = handle_contrast(&mut state, id, value);

                let _ = reply.send(result);
            }

            MonitorCommand::Dim { id, name, factor, reply } => {
                let result = handle_dim(&mut state, id, name, factor);

                let _ = reply.send(result);
            }

            MonitorCommand::Restore { id, name, reply } => {
                let result = handle_restore(&mut state, id, name);

                let _ = reply.send(result);
            }

            MonitorCommand::Subscribe { id, sender, reply } => {
                state.subscribers.push(sender);

                let result = Ok(response(id, state.brightness, state.brightness_maximum));

                let _ = reply.send(result);
            }

            MonitorCommand::Info { reply } => {
                let result = Ok(ListMonitor {
                    connector: state.monitor.connector.clone(),

                    path: state.monitor.path.display().to_string(),

                    name: state.monitor.name.clone(),

                    id: state.monitor.id.clone(),
                });

                let _ = reply.send(result);
            }
        }
    }

    println!("Monitor worker for {} stopped", connector);
}

fn handle_brightness(
    state: &mut MonitorState,
    id: Option<u64>,
    value: Option<u16>
) -> Result<Message, String> {
    if let Some(value) = value {
        if value > state.brightness_maximum {
            return Err(format!("brightness must be between 0 and {}", state.brightness_maximum));
        }

        let old_brightness = state.brightness;

        state.brightness = value;

        if let Err(error) = apply_brightness(state) {
            state.brightness = old_brightness;

            return Err(error.to_string());
        }

        notify(state, "brightness_changed");
    }

    Ok(response(id, state.brightness, state.brightness_maximum))
}

fn handle_contrast(
    state: &mut MonitorState,
    id: Option<u64>,
    value: Option<u16>
) -> Result<Message, String> {
    if let Some(value) = value {
        if value > state.contrast_maximum {
            return Err(format!("contrast must be between 0 and {}", state.contrast_maximum));
        }

        let old_contrast = state.contrast;

        state.contrast = value;

        if let Err(error) = state.monitor.set_vcp_with_recovery(Feature::Contrast, value) {
            state.contrast = old_contrast;

            return Err(error.to_string());
        }

        notify(state, "contrast_changed");
    }

    Ok(response(id, state.contrast, state.contrast_maximum))
}

fn handle_dim(
    state: &mut MonitorState,
    id: Option<u64>,
    name: String,
    factor: f32
) -> Result<Message, String> {
    if !factor.is_finite() {
        return Err("factor must be finite".to_string());
    }

    if factor < 0.0 {
        return Err("factor must not be negative".to_string());
    }

    let old_factor = state.modifiers.insert(name.clone(), factor);

    if let Err(error) = apply_brightness(state) {
        match old_factor {
            Some(old_factor) => {
                state.modifiers.insert(name, old_factor);
            }

            None => {
                state.modifiers.remove(&name);
            }
        }

        return Err(error.to_string());
    }

    notify(state, "dim_changed");

    Ok(response(id, state.brightness, state.brightness_maximum))
}

fn handle_restore(
    state: &mut MonitorState,
    id: Option<u64>,
    name: String
) -> Result<Message, String> {
    let old_factor = state.modifiers.remove(&name);

    if let Err(error) = apply_brightness(state) {
        if let Some(old_factor) = old_factor {
            state.modifiers.insert(name, old_factor);
        }

        return Err(error.to_string());
    }

    notify(state, "restore");

    Ok(response(id, state.brightness, state.brightness_maximum))
}

fn cache_path() -> Result<PathBuf> {
    let base = if let Some(path) = std::env::var_os("XDG_CACHE_HOME") {
        PathBuf::from(path)
    } else if let Some(home) = std::env::var_os("HOME") {
        PathBuf::from(home).join(".cache")
    } else {
        return Err(anyhow!("Could not determine cache directory"));
    };

    Ok(base.join("displayd").join("monitors.json"))
}

fn load_cached_monitors() -> Vec<CachedMonitor> {
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

    serde_json::from_str(&contents).unwrap_or_default()
}

fn save_cached_monitors(monitors: &[CachedMonitor]) -> Result<()> {
    let cache = cache_path()?;

    let parent = cache.parent().ok_or_else(|| anyhow!("Invalid cache path"))?;

    fs::create_dir_all(parent)?;

    let temporary = parent.join("monitors.json.tmp");

    let contents = serde_json::to_string_pretty(monitors)?;

    fs::write(&temporary, format!("{}\n", contents))?;

    fs::rename(temporary, cache)?;

    Ok(())
}

fn monitor_state_from_probe(
    path: PathBuf,
    brightness: u16,
    brightness_maximum: u16
) -> Result<MonitorState> {
    let mut monitor = Monitor::from_discovery(path)?;

    let contrast = monitor.get_vcp_with_recovery(Feature::Contrast)?;

    println!("Current contrast: {}/{}", contrast.current, contrast.maximum);

    Ok(MonitorState {
        monitor,
        brightness,
        brightness_maximum,
        contrast: contrast.current,
        contrast_maximum: contrast.maximum,
        modifiers: HashMap::new(),
        subscribers: Vec::new(),
    })
}

fn discover_monitors() -> Result<Vec<MonitorState>> {
    let cached_monitors = load_cached_monitors();

    if !cached_monitors.is_empty() {
        println!("Trying {} cached monitor(s)...", cached_monitors.len());

        let mut monitors = Vec::new();

        let mut valid_cache = Vec::new();

        for cached in cached_monitors {
            println!("Probing cached monitor {} at {}", cached.connector, cached.path.display());

            match LinuxI2cTransport::probe(&cached.path) {
                Ok(Some(discovered)) => {
                    println!("Cached device is valid: {}", discovered.path.display());

                    println!("Current brightness: {}", discovered.brightness);

                    let path = discovered.path.clone();

                    match Monitor::from_discovery(path.clone()) {
                        Ok(monitor) if monitor.connector == cached.connector => {
                            drop(monitor);

                            monitors.push(
                                monitor_state_from_probe(
                                    path.clone(),
                                    discovered.brightness,
                                    discovered.brightness_maximum
                                )?
                            );

                            valid_cache.push(CachedMonitor {
                                connector: cached.connector,
                                path,
                            });
                        }

                        Ok(monitor) => {
                            eprintln!(
                                "Cached path {} belongs to connector {}, expected {}",
                                cached.path.display(),
                                monitor.connector,
                                cached.connector
                            );
                        }

                        Err(error) => {
                            eprintln!("Cached monitor {} failed: {}", cached.connector, error);
                        }
                    }
                }

                Ok(None) => {
                    eprintln!("Cached device is not a DDC/CI monitor: {}", cached.path.display());
                }

                Err(error) => {
                    eprintln!("Cached monitor {} failed: {}", cached.connector, error);
                }
            }
        }

        if !monitors.is_empty() {
            if let Err(error) = save_cached_monitors(&valid_cache) {
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

    let mut cached = Vec::with_capacity(discovered.len());

    for discovered in discovered {
        println!("Found monitor I²C at {}", discovered.path.display());

        println!("Current brightness: {}", discovered.brightness);

        let path = discovered.path.clone();

        let state = monitor_state_from_probe(
            path.clone(),
            discovered.brightness,
            discovered.brightness_maximum
        )?;

        println!("Found monitor {} on {}", state.monitor.connector, state.monitor.path.display());

        cached.push(CachedMonitor {
            connector: state.monitor.connector.clone(),

            path,
        });

        monitors.push(state);
    }

    if let Err(error) = save_cached_monitors(&cached) {
        eprintln!("Failed to update monitor cache: {}", error);
    }

    Ok(monitors)
}

fn effective_brightness(state: &MonitorState) -> u16 {
    let factor: f32 = state.modifiers.values().product();

    ((state.brightness as f32) * factor).round().clamp(0.0, state.brightness_maximum as f32) as u16
}

fn apply_brightness(state: &mut MonitorState) -> Result<()> {
    let hardware = effective_brightness(state);

    state.monitor.set_vcp_with_recovery(Feature::Brightness, hardware)?;

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

fn response(id: Option<u64>, current: u16, maximum: u16) -> Message {
    Message::Response {
        id,
        current,
        maximum,
        percentage: if maximum == 0 {
            0.0
        } else {
            ((current as f32) / (maximum as f32)) * 100.0
        },
    }
}

async fn execute(request: Request, display: Arc<DisplayState>) -> Result<Message> {
    if request.command == "list" {
        return list_monitors(&display).await;
    }

    let monitor = display.resolve(request.monitor.as_deref()).map_err(|error| anyhow!(error))?;

    let id = request.id;

    match request.command.as_str() {
        "brightness" => {
            let (reply_tx, reply_rx) = oneshot::channel();

            monitor
                .send(MonitorCommand::Brightness {
                    id,
                    value: request.value,
                    reply: reply_tx,
                }).await
                .map_err(|error| anyhow!(error))?;

            reply_rx.await
                .map_err(|_| anyhow!("Monitor worker stopped"))?
                .map_err(|error| anyhow!(error))
        }

        "contrast" => {
            let (reply_tx, reply_rx) = oneshot::channel();

            monitor
                .send(MonitorCommand::Contrast {
                    id,
                    value: request.value,
                    reply: reply_tx,
                }).await
                .map_err(|error| anyhow!(error))?;

            reply_rx.await
                .map_err(|_| anyhow!("Monitor worker stopped"))?
                .map_err(|error| anyhow!(error))
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

            let (reply_tx, reply_rx) = oneshot::channel();

            monitor
                .send(MonitorCommand::Dim {
                    id,
                    name,
                    factor,
                    reply: reply_tx,
                }).await
                .map_err(|error| anyhow!(error))?;

            reply_rx.await
                .map_err(|_| anyhow!("Monitor worker stopped"))?
                .map_err(|error| anyhow!(error))
        }

        "restore" => {
            let name = request.name.unwrap_or_else(|| "default".into());

            let (reply_tx, reply_rx) = oneshot::channel();

            monitor
                .send(MonitorCommand::Restore {
                    id,
                    name,
                    reply: reply_tx,
                }).await
                .map_err(|error| anyhow!(error))?;

            reply_rx.await
                .map_err(|_| anyhow!("Monitor worker stopped"))?
                .map_err(|error| anyhow!(error))
        }

        _ => Err(anyhow!("Unknown command: {}", request.command)),
    }
}

async fn list_monitors(display: &DisplayState) -> Result<Message> {
    let mut monitors = Vec::with_capacity(display.monitors.len());

    for monitor in display.monitors.values() {
        let (reply_tx, reply_rx) = oneshot::channel();

        monitor
            .send(MonitorCommand::Info {
                reply: reply_tx,
            }).await
            .map_err(|error| anyhow!(error))?;

        let info = reply_rx.await
            .map_err(|_| anyhow!("Monitor worker stopped"))?
            .map_err(|error| anyhow!(error))?;

        monitors.push(info);
    }

    Ok(Message::List { monitors })
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

async fn handle_client(stream: UnixStream, display: Arc<DisplayState>) -> Result<()> {
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
        let monitor = display.resolve(request.monitor.as_deref()).map_err(|error| anyhow!(error))?;

        let connector = monitor.connector.clone();

        let monitor = display.get(&connector).map_err(|error| anyhow!(error))?;

        let (tx, mut rx) = mpsc::unbounded_channel();

        let (reply_tx, reply_rx) = oneshot::channel();

        monitor
            .send(MonitorCommand::Subscribe {
                id: request.id,
                sender: tx,
                reply: reply_tx,
            }).await
            .map_err(|error| anyhow!(error))?;

        let initial_response = reply_rx.await
            .map_err(|_| anyhow!("Monitor worker stopped"))?
            .map_err(|error| anyhow!(error))?;

        println!("New subscriber for monitor {}", connector);

        write_message(&mut write, &initial_response).await?;

        while let Some(event) = rx.recv().await {
            if let Err(error) = write_message(&mut write, &event).await {
                eprintln!("Subscriber for monitor {} disconnected: {}", connector, error);

                break;
            }
        }

        return Ok(());
    }

    let monitor_text = request.monitor.as_deref().unwrap_or("<none>");

    println!("Executing {} on monitor {}", request.command, monitor_text);

    let start = Instant::now();

    let request_id = request.id;

    match execute(request, display).await {
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

    let monitors = tokio::task::spawn_blocking(discover_monitors).await??;

    println!("Using {} monitor(s)", monitors.len());

    let mut handles = HashMap::with_capacity(monitors.len());
    let mut default_monitor = None;

    for monitor_state in monitors {
        let connector = monitor_state.monitor.connector.clone();

        if default_monitor.is_none() {
            default_monitor = Some(connector.clone());
        }

        let (tx, rx) = mpsc::channel::<MonitorCommand>(32);
        let worker_connector = connector.clone();

        thread::Builder
            ::new()
            .name(format!("displayd-monitor-{}", connector))
            .spawn(move || {
                monitor_worker(worker_connector, monitor_state, rx);
            })?;

        handles.insert(connector.clone(), MonitorHandle {
            connector,
            tx,
        });
    }

    let display = Arc::new(DisplayState {
        monitors: handles,
        default_monitor,
    });

    let listener = UnixListener::bind(&socket)?;

    println!("Listening on {}", socket.display());

    loop {
        let (stream, _) = listener.accept().await?;

        let display = Arc::clone(&display);

        tokio::spawn(async move {
            if let Err(error) = handle_client(stream, display).await {
                eprintln!("client error: {}", error);
            }
        });
    }
}
