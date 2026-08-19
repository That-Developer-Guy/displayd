use anyhow::{ anyhow, Result };
use serde::{ Deserialize, Serialize };

use std::{
    collections::{ HashMap, HashSet },
    fs,
    path::{ Path, PathBuf },
    sync::Arc,
    thread,
    time::{ Duration, Instant },
};

use edid::{ parse::{ parse as parse_edid, EdidData, ProductionDate }, read::read_edid };

use tokio::{
    io::{ AsyncBufReadExt, AsyncWriteExt, BufReader },
    net::{ UnixListener, UnixStream },
    sync::{ mpsc, oneshot, RwLock },
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
enum ProductionDateInfo {
    Manufacture {
        week: u8,
        year: u16,
    },
    ModelYear {
        year: u16,
    },
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct MonitorEdidInfo {
    manufacturer: String,
    company_name: Option<String>,
    name: Option<String>,
    product_code: u16,
    edid_version: String,
    serial_number: u32,
    production_date: ProductionDateInfo,
}

impl From<&EdidData> for MonitorEdidInfo {
    fn from(edid: &EdidData) -> Self {
        Self {
            manufacturer: edid.id.clone(),
            company_name: edid.manifacturer.clone(),
            name: edid.name.clone(),
            product_code: edid.product_code,
            serial_number: edid.serial_number,
            edid_version: edid.edid_version.clone(),
            production_date: match &edid.production_date {
                ProductionDate::Manufacture { week, year } =>
                    ProductionDateInfo::Manufacture {
                        week: *week,
                        year: *year,
                    },

                ProductionDate::ModelYear { year } =>
                    ProductionDateInfo::ModelYear {
                        year: *year,
                    },
            },
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct ListMonitor {
    connector: String,

    path: String,

    name: Option<String>,

    edid_data: MonitorEdidInfo,

    mccs_version: Option<String>,

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

    #[serde(rename = "info")] Info {
        monitor: ListMonitor,
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

    #[serde(default)]
    default: bool,
}

struct Monitor {
    id: MonitorId,

    connector: String,

    name: Option<String>,

    edid_data: EdidData,

    mccs_version: Option<String>,

    path: PathBuf,
    device: Ddc,
}

impl Monitor {
    fn from_discovery(path: PathBuf) -> Result<Self> {
        let transport = LinuxI2cTransport::open(path.clone())?;
        let mut device = DdcDevice::new(transport);

        let mccs_version = device
            .get_mccs_version()
            .ok()
            .map(|version| version.to_string());

        let edid_bytes = read_edid(&path)?;
        let edid = parse_edid(&edid_bytes)?;

        let id = MonitorId::from_edid(&edid)?;
        let connector = find_drm_connector(&edid_bytes)?;

        Ok(Self {
            id,
            connector,
            name: edid.name.clone(),
            edid_data: edid,
            mccs_version,
            path,
            device,
        })
    }

    fn get_vcp(&mut self, feature: Feature) -> Result<VcpValue> {
        Ok(self.device.get_vcp(feature)?)
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
                .ok_or_else(|| { anyhow!("Invalid DRM connector name: {}", name) })?;

            return Ok(connector.to_string());
        }
    }

    Err(anyhow!("Could not resolve DRM connector from EDID"))
}

fn connected_drm_connectors() -> Result<HashSet<String>> {
    let drm = Path::new("/sys/class/drm");
    let mut connected = HashSet::new();

    for entry in fs::read_dir(drm)? {
        let entry = entry?;
        let connector_path = entry.path();

        if !connector_path.is_dir() {
            continue;
        }

        let name = match entry.file_name().into_string() {
            Ok(name) => name,
            Err(_) => {
                continue;
            }
        };

        let connector = match
            name
                .strip_prefix("card")
                .and_then(|rest| rest.split_once('-'))
                .map(|(_, connector)| connector.to_string())
        {
            Some(connector) => connector,
            None => {
                continue;
            }
        };

        let status = match fs::read_to_string(connector_path.join("status")) {
            Ok(status) => status,
            Err(_) => {
                continue;
            }
        };

        if status.trim() == "connected" {
            connected.insert(connector);
        }
    }

    Ok(connected)
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
    monitors: RwLock<HashMap<String, Arc<MonitorHandle>>>,
    default_monitor: RwLock<Option<String>>,
}

impl DisplayState {
    async fn get(&self, connector: &str) -> Result<Arc<MonitorHandle>, String> {
        let monitors = self.monitors.read().await;

        monitors
            .get(connector)
            .cloned()
            .ok_or_else(|| {
                format!(
                    "Monitor {} does not exist ({} monitor(s) available)",
                    connector,
                    monitors.len()
                )
            })
    }

    async fn resolve(&self, connector: Option<&str>) -> Result<Arc<MonitorHandle>, String> {
        match connector {
            Some(connector) => self.get(connector).await,

            None => {
                let connector = self.default_monitor
                    .read().await
                    .clone()
                    .ok_or_else(|| { "No default monitor is available".to_string() })?;

                self.get(&connector).await
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

                    edid_data: (&state.monitor.edid_data).into(),

                    mccs_version: state.monitor.mccs_version.clone(),

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
    mut monitor: Monitor,
    brightness: u16,
    brightness_maximum: u16
) -> Result<MonitorState> {
    let contrast = monitor.get_vcp(Feature::Contrast)?;

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

fn discover_monitors() -> Result<(Vec<MonitorState>, Option<String>)> {
    let cached_monitors = load_cached_monitors();

    let cached_default = cached_monitors
        .iter()
        .find(|monitor| monitor.default)
        .map(|monitor| monitor.connector.clone());

    let mut monitors_by_connector = HashMap::<String, MonitorState>::new();

    for cached in &cached_monitors {
        println!("Probing cached monitor {} at {}", cached.connector, cached.path.display());

        match LinuxI2cTransport::probe(&cached.path) {
            Ok(Some(discovered)) => {
                let path = discovered.path.clone();

                match Monitor::from_discovery(path.clone()) {
                    Ok(monitor) if monitor.connector == cached.connector => {
                        match
                            monitor_state_from_probe(
                                monitor,
                                discovered.brightness,
                                discovered.brightness_maximum
                            )
                        {
                            Ok(state) => {
                                println!(
                                    "Restored monitor {} on {}",
                                    state.monitor.connector,
                                    state.monitor.path.display()
                                );

                                monitors_by_connector.insert(
                                    state.monitor.connector.clone(),
                                    state
                                );
                            }

                            Err(error) => {
                                eprintln!("Cached monitor {} failed: {}", cached.connector, error);
                            }
                        }
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

    println!("Performing full monitor discovery...");

    let discovered = match find_monitors() {
        Ok(monitors) => monitors,

        Err(error) => {
            eprintln!("Full monitor discovery failed: {}", error);

            let mut monitors = monitors_by_connector.into_values().collect::<Vec<_>>();

            monitors.sort_by(|a, b| { a.monitor.connector.cmp(&b.monitor.connector) });

            if monitors.is_empty() {
                return Err(error.into());
            }

            let default_monitor = cached_default
                .filter(|connector| {
                    monitors.iter().any(|monitor| monitor.monitor.connector == *connector)
                })
                .or_else(|| { monitors.first().map(|monitor| monitor.monitor.connector.clone()) });

            return Ok((monitors, default_monitor));
        }
    };

    for discovered in discovered {
        let path = discovered.path.clone();

        let monitor = match Monitor::from_discovery(path.clone()) {
            Ok(monitor) => monitor,

            Err(error) => {
                eprintln!("Failed to inspect discovered monitor at {}: {}", path.display(), error);

                continue;
            }
        };

        if monitors_by_connector.contains_key(&monitor.connector) {
            continue;
        }

        let connector = monitor.connector.clone();

        println!("Found new monitor I²C at {}", path.display());
        println!("Current brightness: {}", discovered.brightness);

        match
            monitor_state_from_probe(monitor, discovered.brightness, discovered.brightness_maximum)
        {
            Ok(state) => {
                println!(
                    "Found monitor {} on {}",
                    state.monitor.connector,
                    state.monitor.path.display()
                );

                monitors_by_connector.insert(connector, state);
            }

            Err(error) => {
                eprintln!("Failed to probe monitor {}: {}", connector, error);

                continue;
            }
        }
    }

    if monitors_by_connector.is_empty() {
        return Err(anyhow!("No DDC/CI monitors found"));
    }

    let mut monitors = monitors_by_connector.into_values().collect::<Vec<_>>();

    monitors.sort_by(|a, b| { a.monitor.connector.cmp(&b.monitor.connector) });

    let default_monitor = cached_default
        .filter(|connector| {
            monitors.iter().any(|monitor| monitor.monitor.connector == *connector)
        })
        .or_else(|| { monitors.first().map(|monitor| monitor.monitor.connector.clone()) });

    save_monitor_cache(&monitors, default_monitor.as_deref())?;

    Ok((monitors, default_monitor))
}

fn save_monitor_cache(monitors: &[MonitorState], default_monitor: Option<&str>) -> Result<()> {
    let cached = monitors
        .iter()
        .map(|state| CachedMonitor {
            connector: state.monitor.connector.clone(),
            path: state.monitor.path.clone(),
            default: Some(state.monitor.connector.as_str()) == default_monitor,
        })
        .collect::<Vec<_>>();

    save_cached_monitors(&cached)
}

async fn save_display_cache(display: &DisplayState) -> Result<()> {
    let default_monitor = display.default_monitor.read().await.clone();

    let handles = {
        let monitors = display.monitors.read().await;

        monitors.values().cloned().collect::<Vec<_>>()
    };

    let mut cached = Vec::with_capacity(handles.len());

    for handle in handles {
        let (reply_tx, reply_rx) = oneshot::channel();

        handle
            .send(MonitorCommand::Info { reply: reply_tx }).await
            .map_err(|error| anyhow!(error))?;

        let info = reply_rx.await
            .map_err(|_| anyhow!("Monitor worker stopped"))?
            .map_err(|error| anyhow!(error))?;

        cached.push(CachedMonitor {
            connector: info.connector.clone(),
            path: PathBuf::from(info.path),
            default: Some(info.connector.as_str()) == default_monitor.as_deref(),
        });
    }

    save_cached_monitors(&cached)
}

async fn hotplug_monitor(display: Arc<DisplayState>) {
    let mut interval = tokio::time::interval(Duration::from_secs(5));

    loop {
        interval.tick().await;

        let connected = match tokio::task::spawn_blocking(connected_drm_connectors).await {
            Ok(Ok(connectors)) => connectors,

            Ok(Err(error)) => {
                eprintln!("DRM connector status check failed: {}", error);
                continue;
            }

            Err(error) => {
                eprintln!("DRM connector status task failed: {}", error);
                continue;
            }
        };

        let removed_connectors = {
            let mut monitors = display.monitors.write().await;

            let existing = monitors.keys().cloned().collect::<Vec<_>>();
            let mut removed = Vec::new();

            for connector in existing {
                if !connected.contains(&connector) {
                    if monitors.remove(&connector).is_some() {
                        removed.push(connector);
                    }
                }
            }

            removed
        };

        for connector in &removed_connectors {
            println!("Monitor unplugged: {}", connector);

            let default_changed = {
                let mut default = display.default_monitor.write().await;

                if default.as_deref() == Some(connector.as_str()) {
                    let replacement = {
                        let monitors = display.monitors.read().await;
                        monitors.keys().next().cloned()
                    };

                    *default = replacement;
                    true
                } else {
                    false
                }
            };

            if default_changed {
                let default = display.default_monitor.read().await.clone();

                println!("Default monitor changed to {}", default.as_deref().unwrap_or("<none>"));
            }
        }

        let existing = {
            let monitors = display.monitors.read().await;
            monitors.keys().cloned().collect::<HashSet<_>>()
        };

        let needs_discovery = connected.iter().any(|connector| !existing.contains(connector));

        if !needs_discovery {
            if !removed_connectors.is_empty() {
                if let Err(error) = save_display_cache(&display).await {
                    eprintln!("Failed to update monitor cache after removal: {}", error);
                }
            }

            continue;
        }

        let discovered = match tokio::task::spawn_blocking(find_monitors).await {
            Ok(Ok(monitors)) => monitors,

            Ok(Err(error)) => {
                eprintln!("Monitor hotplug discovery failed: {}", error);
                continue;
            }

            Err(error) => {
                eprintln!("Monitor discovery task failed: {}", error);
                continue;
            }
        };

        for discovered in discovered {
            let path = discovered.path.clone();

            let monitor = match Monitor::from_discovery(path.clone()) {
                Ok(monitor) => monitor,

                Err(error) => {
                    eprintln!(
                        "Failed to inspect discovered monitor at {}: {}",
                        path.display(),
                        error
                    );

                    continue;
                }
            };

            let connector = monitor.connector.clone();

            if !connected.contains(&connector) {
                continue;
            }

            let already_present = {
                let monitors = display.monitors.read().await;
                monitors.contains_key(&connector)
            };

            if already_present {
                continue;
            }

            println!(
                "Hotplug detected: {} ({})",
                connector,
                monitor.name.as_deref().unwrap_or("unnamed")
            );

            let state = match
                monitor_state_from_probe(
                    monitor,
                    discovered.brightness,
                    discovered.brightness_maximum
                )
            {
                Ok(state) => state,

                Err(error) => {
                    eprintln!("Failed to probe newly connected monitor {}: {}", connector, error);

                    continue;
                }
            };

            let handle = match start_monitor_worker(state) {
                Ok(handle) => handle,

                Err(error) => {
                    eprintln!("Failed to start worker for new monitor {}: {}", connector, error);

                    continue;
                }
            };

            let inserted = {
                let mut monitors = display.monitors.write().await;

                if monitors.contains_key(&connector) {
                    false
                } else {
                    monitors.insert(connector.clone(), handle);
                    true
                }
            };

            if inserted {
                println!("Added hotplugged monitor {}", connector);
            }
        }

        let default_missing = {
            let default = display.default_monitor.read().await;
            default.is_none()
        };

        if default_missing {
            let replacement = {
                let monitors = display.monitors.read().await;
                monitors.keys().next().cloned()
            };

            if let Some(replacement) = replacement {
                *display.default_monitor.write().await = Some(replacement.clone());

                println!("Selected {} as the default monitor", replacement);
            }
        }

        if !removed_connectors.is_empty() || needs_discovery {
            if let Err(error) = save_display_cache(&display).await {
                eprintln!("Failed to update monitor cache after hotplug: {}", error);
            }
        }
    }
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

    let monitor = display
        .resolve(request.monitor.as_deref()).await
        .map_err(|error| anyhow!(error))?;

    if request.command == "info" {
        let (reply_tx, reply_rx) = oneshot::channel();

        monitor
            .send(MonitorCommand::Info { reply: reply_tx }).await
            .map_err(|error| anyhow!(error))?;

        let info = reply_rx.await
            .map_err(|_| anyhow!("Monitor worker stopped"))?
            .map_err(|error| anyhow!(error))?;

        return Ok(Message::Info { monitor: info });
    }

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
    let monitors = {
        let handles = display.monitors.read().await;

        handles.values().cloned().collect::<Vec<_>>()
    };

    let mut result = Vec::with_capacity(monitors.len());

    for monitor in monitors {
        let (reply_tx, reply_rx) = oneshot::channel();

        monitor
            .send(MonitorCommand::Info { reply: reply_tx }).await
            .map_err(|error| anyhow!(error))?;

        let info = reply_rx.await
            .map_err(|_| anyhow!("Monitor worker stopped"))?
            .map_err(|error| anyhow!(error))?;

        result.push(info);
    }

    result.sort_by(|a, b| a.connector.cmp(&b.connector));

    Ok(Message::List { monitors: result })
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
        .map_err(|error| anyhow!("Invalid request: {}", error))?;

    if request.command == "subscribe" {
        let monitor = display
            .resolve(request.monitor.as_deref()).await
            .map_err(|error| anyhow!(error))?;

        let connector = monitor.connector.clone();

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

fn start_monitor_worker(monitor_state: MonitorState) -> Result<Arc<MonitorHandle>> {
    let connector = monitor_state.monitor.connector.clone();

    let (tx, rx) = mpsc::channel::<MonitorCommand>(32);

    let worker_connector = connector.clone();

    thread::Builder
        ::new()
        .name(format!("displayd-monitor-{}", connector))
        .spawn(move || {
            monitor_worker(worker_connector, monitor_state, rx);
        })?;

    Ok(
        Arc::new(MonitorHandle {
            connector,
            tx,
        })
    )
}

#[tokio::main]
async fn main() -> Result<()> {
    let socket = socket_path()?;

    if socket.exists() {
        fs::remove_file(&socket)?;
    }

    let (monitors, default_monitor) = tokio::task::spawn_blocking(discover_monitors).await??;

    println!("Using {} monitor(s)", monitors.len());

    let mut handles = HashMap::with_capacity(monitors.len());

    for monitor_state in monitors {
        let handle = start_monitor_worker(monitor_state)?;

        handles.insert(handle.connector.clone(), handle);
    }

    let display = Arc::new(DisplayState {
        monitors: RwLock::new(handles),
        default_monitor: RwLock::new(default_monitor),
    });

    let hotplug_display = Arc::clone(&display);

    tokio::spawn(async move {
        hotplug_monitor(hotplug_display).await;
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
