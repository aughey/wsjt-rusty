//! Leptos radio controls. Only transport/resource ownership is JavaScript.
use gloo_timers::callback::Interval;
use js_sys::{Promise, Uint8Array};
use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::{JsFuture, spawn_local};
use wsjt_rig::{Command, DEFAULT_ADDRESS, Mode, Reply, Transaction};

#[wasm_bindgen(module = "/src/serial-runtime.js")]
extern "C" {
    #[wasm_bindgen(js_name = loadRadioPreferences)]
    fn load_preferences() -> String;
    #[wasm_bindgen(js_name = saveRadioPreferences)]
    fn save_preferences(json: &str) -> String;
    #[wasm_bindgen(js_name = serialSupported)]
    fn supported() -> bool;
    #[wasm_bindgen(js_name = createSerial)]
    fn create() -> JsValue;
    #[wasm_bindgen(js_name = openSerial)]
    fn open(s: &JsValue, baud: u32, line: &str) -> Promise;
    #[wasm_bindgen(js_name = writeSerial)]
    fn write(s: &JsValue, bytes: &[u8]) -> Promise;
    #[wasm_bindgen(js_name = readSerial)]
    fn read(s: &JsValue, timeout: f64) -> Promise;
    #[wasm_bindgen(js_name = closeSerial)]
    fn close(s: &JsValue) -> Promise;
    #[wasm_bindgen(js_name = isSerialOpen)]
    fn is_open(s: &JsValue) -> bool;
    #[wasm_bindgen(js_name = setPttLine)]
    fn set_line(s: &JsValue, line: &str, on: bool) -> Promise;
    #[wasm_bindgen(js_name = armRelease)]
    fn arm(s: &JsValue, off: &[u8]);
    #[wasm_bindgen(js_name = disarmRelease)]
    fn disarm(s: &JsValue);
}

fn error_text(error: JsValue) -> String {
    js_sys::Reflect::get(&error, &"message".into())
        .ok()
        .and_then(|v| v.as_string())
        .or_else(|| error.as_string())
        .unwrap_or_else(|| format!("{error:?}"))
}

fn abandon(handle: &JsValue) {
    let pending = close(handle);
    spawn_local(async move {
        let _ = JsFuture::from(pending).await;
    });
}

struct Port {
    handle: JsValue,
    address: u8,
    pending: RefCell<Promise>,
}
impl Port {
    async fn execute(&self, command: Command) -> Result<Reply, String> {
        // Polls and operator/PTT commands share the port without sharing UI busy state.
        // Acquire in call order so an ACK can only satisfy its own transaction.
        let mut complete = None;
        let next = Promise::new(&mut |resolve, _| complete = Some(resolve));
        let previous = self.pending.replace(next);
        let _ = JsFuture::from(previous).await;
        let result = self.execute_transaction(command).await;
        if result.is_err() {
            let _ = JsFuture::from(close(&self.handle)).await;
        }
        if let Some(complete) = complete {
            let _ = complete.call0(&JsValue::UNDEFINED);
        }
        result
    }
    async fn execute_transaction(&self, command: Command) -> Result<Reply, String> {
        let mut transaction = Transaction::new(self.address, command).map_err(|e| e.to_string())?;
        JsFuture::from(write(&self.handle, transaction.request()))
            .await
            .map_err(error_text)?;
        let deadline = js_sys::Date::now() + 2000.0;
        loop {
            let remaining = deadline - js_sys::Date::now();
            if remaining <= 0.0 {
                return Err("CI-V reply timed out; reconnect to recover".into());
            }
            let bytes = JsFuture::from(read(&self.handle, remaining))
                .await
                .map_err(error_text)?;
            if let Some(reply) = transaction
                .feed(&Uint8Array::new(&bytes).to_vec())
                .map_err(|e| e.to_string())?
            {
                return Ok(reply);
            }
        }
    }
}

#[derive(Default)]
struct Ports {
    cat: Option<Rc<Port>>,
    ptt: Option<JsValue>,
}
impl Ports {
    fn close(&mut self) {
        if let Some(port) = self.cat.take() {
            abandon(&port.handle);
        }
        if let Some(port) = self.ptt.take() {
            abandon(&port);
        }
    }
}

#[derive(Clone, Copy)]
struct Ui {
    active: RwSignal<bool>,
    busy: RwSignal<bool>,
    polling: RwSignal<bool>,
    status: RwSignal<String>,
    dial: RwSignal<String>,
    frequency: RwSignal<Option<(u64, f64)>>,
    actual_mode: RwSignal<String>,
    actual_power: RwSignal<String>,
    tx: RwSignal<bool>,
}

async fn refresh(port: &Port, ui: Ui, alive: &Cell<bool>) -> Result<(), String> {
    let frequency = port.execute(Command::ReadFrequency).await?;
    let frequency_read_ms = js_sys::Date::now();
    let mode = port.execute(Command::ReadMode).await?;
    if alive.get() {
        if let Reply::Frequency(hz) = frequency {
            ui.dial.set(format!("{:.6} MHz", hz as f64 / 1_000_000.0));
            ui.frequency.set(Some((hz, frequency_read_ms)));
        }
        if let Reply::Mode(mode) = mode {
            ui.actual_mode.set(mode.to_string());
        }
    }
    Ok(())
}

fn run(ports: Rc<RefCell<Ports>>, alive: Rc<Cell<bool>>, ui: Ui, commands: Vec<Command>) {
    if ui.busy.get_untracked() {
        return;
    }
    let Some(port) = ports.borrow().cat.clone() else {
        return;
    };
    let polling = commands.is_empty();
    if polling && ui.polling.get_untracked() {
        return;
    }
    if polling {
        ui.polling.set(true);
    } else {
        ui.busy.set(true);
    }
    spawn_local(async move {
        let result = async {
            for command in commands {
                let reply = port.execute(command).await?;
                if let Reply::Power(value) = reply
                    && alive.get()
                {
                    ui.actual_power.set(format!("{value} / 255"));
                }
            }
            refresh(&port, ui, &alive).await
        }
        .await;
        if !alive.get()
            || !ports
                .borrow()
                .cat
                .as_ref()
                .is_some_and(|p| Rc::ptr_eq(p, &port))
        {
            return;
        }
        if polling {
            ui.polling.set(false);
        } else {
            ui.busy.set(false);
        }
        match result {
            Ok(()) if !polling => ui.status.set("Radio synchronized".into()),
            Ok(()) => {}
            Err(error) => {
                ports.borrow_mut().close();
                ui.active.set(false);
                ui.tx.set(false);
                ui.dial.set("Unknown".into());
                ui.frequency.set(None);
                ui.actual_mode.set("Unknown".into());
                ui.status.set(format!(
                    "{error}. Disconnected; TX release attempted. Check the radio."
                ));
            }
        }
    });
}

// Shared browser coordinator; protocol libraries remain independent of radio I/O.
#[derive(Clone, Copy)]
pub struct RadioControl {
    access: StoredValue<Option<Rc<RadioAccess>>, LocalStorage>,
    lease: StoredValue<Rc<RefCell<Option<Rc<PttLease>>>>, LocalStorage>,
    reserved: RwSignal<bool>,
}
struct RadioAccess {
    ports: Rc<RefCell<Ports>>,
    alive: Rc<Cell<bool>>,
    ui: Ui,
    method: RwSignal<String>,
    ptt_ready: RwSignal<bool>,
    ptt_busy: RwSignal<bool>,
    ptt_status: RwSignal<String>,
}
struct PttLease {
    access: Rc<RadioAccess>,
    handle: JsValue,
    port: Option<Rc<Port>>,
    method: String,
    keyed: Cell<bool>,
}
impl RadioControl {
    /// Fresh CI-V ReadFrequency result, never the editable frequency form.
    /// Reuse serialized background queries so logging cannot delay timed PTT.
    pub fn frequency(self) -> Option<(u64, f64)> {
        self.access.with_value(|access| {
            let access = access.as_ref()?;
            if !access.alive.get()
                || !access.ui.active.get_untracked()
                || !access
                    .ports
                    .borrow()
                    .cat
                    .as_ref()
                    .is_some_and(|p| is_open(&p.handle))
            {
                return None;
            }
            access.ui.frequency.get_untracked().filter(|(_, ms)| {
                let age = js_sys::Date::now() - ms;
                (0.0..=5_000.0).contains(&age)
            })
        })
    }
    pub fn new() -> Self {
        Self {
            access: StoredValue::new_local(None),
            lease: StoredValue::new_local(Rc::new(RefCell::new(None))),
            reserved: RwSignal::new(false),
        }
    }
    pub fn configured(self) -> bool {
        self.access.with_value(|a| {
            a.as_ref().is_some_and(|a| {
                if a.method.get().starts_with("cat") {
                    a.ui.active.get()
                } else {
                    a.ptt_ready.get()
                }
            })
        })
    }
    pub fn ready(self) -> bool {
        self.configured()
            && !self.reserved.get()
            && self.access.with_value(|a| {
                a.as_ref()
                    .is_some_and(|a| !a.ui.tx.get() && !a.ui.busy.get() && !a.ptt_busy.get())
            })
    }
    pub fn reserve(self) -> Result<(), String> {
        if !self.ready() {
            return Err(
                "Connect the radio and configure PTT first; wait for any radio command to finish"
                    .into(),
            );
        }
        let access = self
            .access
            .with_value(|a| a.clone())
            .ok_or("Radio controls unavailable")?;
        let method = access.method.get_untracked();
        let port = access.ports.borrow().cat.clone();
        let handle = if method.starts_with("cat") {
            port.as_ref().map(|p| p.handle.clone())
        } else {
            access.ports.borrow().ptt.clone()
        }
        .ok_or("PTT disconnected")?;
        if !is_open(&handle) {
            return Err("PTT disconnected".into());
        }
        self.lease.with_value(|cell| {
            *cell.borrow_mut() = Some(Rc::new(PttLease {
                access,
                handle,
                port,
                method,
                keyed: Cell::new(false),
            }))
        });
        self.reserved.set(true);
        Ok(())
    }
    pub fn ptt(self) -> RadioPtt {
        RadioPtt {
            lease: self.lease.get_value(),
            reserved: self.reserved,
        }
    }
}
#[derive(Clone)]
pub struct RadioPtt {
    lease: Rc<RefCell<Option<Rc<PttLease>>>>,
    reserved: RwSignal<bool>,
}
impl RadioPtt {
    pub fn available(&self) -> bool {
        self.lease
            .borrow()
            .as_ref()
            .is_some_and(|l| l.access.alive.get() && is_open(&l.handle))
    }
    pub async fn key(&self) -> Result<(), String> {
        let lease = self.lease.borrow().clone().ok_or("No reserved radio")?;
        if !self.available() {
            return Err("Radio disconnected before transmit".into());
        }
        let off = if lease.method == "cat" {
            Transaction::new(
                lease.port.as_ref().ok_or("Radio disconnected")?.address,
                Command::SetTransmit(false),
            )
            .map_err(|e| e.to_string())?
            .request()
            .to_vec()
        } else {
            vec![]
        };
        arm(&lease.handle, &off);
        lease.keyed.set(true); // Includes a possible lost keying acknowledgment.
        lease.access.ui.tx.set(true);
        lease.access.ptt_busy.set(true);
        let result = lease.set(true).await;
        if lease.access.alive.get() {
            lease.access.ptt_busy.try_set(false);
        }
        if result.is_ok() && lease.access.alive.get() {
            lease
                .access
                .ptt_status
                .set("FT8 audio TX requested; automatic release after message".into());
        }
        result
    }
    pub async fn release(&self) -> Result<(), String> {
        let lease = self.lease.borrow().clone();
        let Some(lease) = lease else {
            return Ok(());
        };
        let result = if lease.keyed.get() {
            match lease.set(false).await {
                Ok(()) => {
                    disarm(&lease.handle);
                    Ok(())
                }
                Err(error) => {
                    let _ = JsFuture::from(close(&lease.handle)).await;
                    Err(format!("{error}; release attempted, verify RX at radio"))
                }
            }
        } else {
            Ok(())
        };
        if lease.access.alive.get() {
            lease.access.ui.tx.set(false);
            lease.access.ptt_busy.set(false);
            lease.access.ptt_status.set(match &result {
                Ok(()) => "FT8 complete/canceled; receive requested".into(),
                Err(e) => e.clone(),
            });
        }
        *self.lease.borrow_mut() = None;
        self.reserved.try_set(false);
        result
    }
}
impl PttLease {
    async fn set(&self, on: bool) -> Result<(), String> {
        if self.method == "cat" {
            self.port
                .as_ref()
                .ok_or("Radio disconnected")?
                .execute(Command::SetTransmit(on))
                .await
                .map(|_| ())
        } else {
            JsFuture::from(set_line(
                &self.handle,
                match self.method.as_str() {
                    "cat-rts" => "requestToSend",
                    "cat-dtr" => "dataTerminalReady",
                    other => other,
                },
                on,
            ))
            .await
            .map(|_| ())
            .map_err(error_text)
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct RadioSettings {
    baud: u32,
    address: String,
    frequency: String,
    mode: u8,
    power: String,
    method: String,
}
impl Default for RadioSettings {
    fn default() -> Self {
        Self {
            baud: 19200,
            address: format!("{DEFAULT_ADDRESS:02X}"),
            frequency: "14074000".into(),
            mode: 1,
            power: "25".into(),
            method: "none".into(),
        }
    }
}
impl RadioSettings {
    fn load() -> Self {
        let mut settings: Self = serde_json::from_str(&load_preferences()).unwrap_or_default();
        let defaults = Self::default();
        if !wsjt_rig::BAUD_RATES.contains(&settings.baud) {
            settings.baud = defaults.baud;
        }
        if u8::from_str_radix(settings.address.trim(), 16)
            .ok()
            .and_then(|a| Transaction::new(a, Command::ReadFrequency).ok())
            .is_none()
        {
            settings.address = defaults.address;
        }
        if settings
            .frequency
            .parse::<u64>()
            .ok()
            .and_then(|hz| Transaction::new(DEFAULT_ADDRESS, Command::SetFrequency(hz)).ok())
            .is_none()
        {
            settings.frequency = defaults.frequency;
        }
        if Mode::from_byte(settings.mode).is_err() {
            settings.mode = defaults.mode;
        }
        if settings.power.parse::<u8>().is_err() {
            settings.power = defaults.power;
        }
        if !matches!(
            settings.method.as_str(),
            "none" | "cat" | "cat-rts" | "cat-dtr" | "requestToSend" | "dataTerminalReady"
        ) {
            settings.method = defaults.method;
        }
        settings
    }
}

#[component]
pub fn RadioPanel(control: RadioControl) -> impl IntoView {
    let ports = Rc::new(RefCell::new(Ports::default()));
    let alive = Rc::new(Cell::new(true));
    let ui = Ui {
        active: RwSignal::new(false),
        busy: RwSignal::new(false),
        polling: RwSignal::new(false),
        status: RwSignal::new(if supported() {
            "Radio disconnected".into()
        } else {
            "Web Serial unavailable. Use a supported browser on localhost or HTTPS, or the native Rust adapter.".into()
        }),
        dial: RwSignal::new("Unknown".into()),
        frequency: RwSignal::new(None),
        actual_mode: RwSignal::new("Unknown".into()),
        actual_power: RwSignal::new("Not read".into()),
        tx: RwSignal::new(false),
    };
    let settings = RadioSettings::load();
    let baud = RwSignal::new(settings.baud);
    let address = RwSignal::new(settings.address);
    let frequency = RwSignal::new(settings.frequency);
    let mode = RwSignal::new(settings.mode);
    let power = RwSignal::new(settings.power);
    let method = RwSignal::new(settings.method);
    let storage_status = RwSignal::new(String::new());
    Effect::new(move |_| {
        let settings = RadioSettings {
            baud: baud.get(),
            address: address.get(),
            frequency: frequency.get(),
            mode: mode.get(),
            power: power.get(),
            method: method.get(),
        };
        if let Ok(json) = serde_json::to_string(&settings) {
            storage_status.set(save_preferences(&json));
        }
    });
    let ptt_ready = RwSignal::new(false);
    let ptt_busy = RwSignal::new(false);
    let ptt_status = RwSignal::new("PTT disabled".to_string());
    let ptt_active = RwSignal::new(false);

    control.access.set_value(Some(Rc::new(RadioAccess {
        ports: ports.clone(),
        alive: alive.clone(),
        ui,
        method,
        ptt_ready,
        ptt_busy,
        ptt_status,
    })));
    let cleanup_ports = StoredValue::new_local(ports.clone());
    let cleanup_alive = StoredValue::new_local(alive.clone());
    on_cleanup(move || {
        cleanup_alive.with_value(|v| v.set(false));
        cleanup_ports.with_value(|v| v.borrow_mut().close());
    });

    let poll_ports = ports.clone();
    let poll_alive = alive.clone();
    let interval = Interval::new(2000, move || {
        if ui.active.get_untracked() && !ui.tx.get_untracked() && !control.reserved.get_untracked()
        {
            run(poll_ports.clone(), poll_alive.clone(), ui, vec![]);
        }
        if ui.tx.get_untracked()
            && poll_ports
                .borrow()
                .cat
                .as_ref()
                .is_some_and(|p| !is_open(&p.handle))
        {
            ui.active.set(false);
            ui.busy.set(false);
            ui.tx.set(false);
            ptt_busy.set(false);
            ptt_status.set("PTT release attempted; verify RX on the radio.".into());
            ui.dial.set("Unknown".into());
            ui.frequency.set(None);
            ui.actual_mode.set("Unknown".into());
            ui.status
                .set("Radio port closed; release attempted. Check radio and reconnect.".into());
        }
        // Watchdog/USB disconnect is reflected even while TX polling is paused.
        if let Some(port) = poll_ports.borrow().ptt.as_ref()
            && !is_open(port)
        {
            ptt_ready.set(false);
            ptt_active.set(false);
            ptt_busy.set(false);
            ui.tx.set(false);
            ptt_status.set("PTT port closed; release attempted. Check radio.".into());
        }
    });
    let interval = StoredValue::new_local(Some(interval));
    on_cleanup(move || {
        interval.update_value(|v| {
            v.take();
        });
    });

    let connect_ports = ports.clone();
    let connect_alive = alive.clone();
    let connect = move |_| {
        let Ok(addr) = u8::from_str_radix(address.get_untracked().trim(), 16) else {
            ui.status.set("Enter a CI-V address in hex".into());
            return;
        };
        if let Err(e) = Transaction::new(addr, Command::ReadFrequency) {
            ui.status.set(e.to_string());
            return;
        }
        let handle = create();
        let port = Rc::new(Port {
            handle: handle.clone(),
            address: addr,
            pending: RefCell::new(Promise::resolve(&JsValue::UNDEFINED)),
        });
        ui.polling.set(false);
        connect_ports.borrow_mut().cat = Some(port.clone());
        ui.active.set(true);
        ui.busy.set(true);
        ui.status.set("Choose the CI-V serial port…".into());
        // Invoke requestPort in the original click's activation, before spawning.
        let pending = open(
            &handle,
            baud.get_untracked(),
            match method.get_untracked().as_str() {
                "cat-rts" => "requestToSend",
                "cat-dtr" => "dataTerminalReady",
                _ => "",
            },
        );
        let ports = connect_ports.clone();
        let alive = connect_alive.clone();
        spawn_local(async move {
            let result = async {
                JsFuture::from(pending).await.map_err(error_text)?;
                refresh(&port, ui, &alive).await
            }
            .await;
            if !alive.get()
                || !ports
                    .borrow()
                    .cat
                    .as_ref()
                    .is_some_and(|p| Rc::ptr_eq(p, &port))
            {
                return;
            }
            ui.busy.set(false);
            match result {
                Ok(()) => ui
                    .status
                    .set("IC-718 connected; polling every 2 seconds".into()),
                Err(error) => {
                    ports.borrow_mut().close();
                    ui.active.set(false);
                    ui.status.set(error);
                }
            }
        });
    };
    let disconnect_ports = ports.clone();
    let disconnect = move |_| {
        disconnect_ports.borrow_mut().close();
        ui.polling.set(false);
        ui.active.set(false);
        ui.busy.set(false);
        ui.tx.set(false);
        ptt_ready.set(false);
        ptt_active.set(false);
        ptt_busy.set(false);
        ui.dial.set("Unknown".into());
        ui.frequency.set(None);
        ui.actual_mode.set("Unknown".into());
        ui.actual_power.set("Not read".into());
        ui.status
            .set("Disconnected; TX release attempted. Verify RX on the radio.".into());
    };
    let tune_ports = ports.clone();
    let tune_alive = alive.clone();
    let tune = move |_| {
        let Ok(hz) = frequency.get_untracked().trim().parse::<u64>() else {
            ui.status.set("Enter a whole frequency in Hz".into());
            return;
        };
        if let Err(error) = Transaction::new(DEFAULT_ADDRESS, Command::SetFrequency(hz)) {
            ui.status.set(error.to_string());
            return;
        }
        run(
            tune_ports.clone(),
            tune_alive.clone(),
            ui,
            vec![Command::SetFrequency(hz)],
        );
    };
    let mode_ports = ports.clone();
    let mode_alive = alive.clone();
    let set_mode = move |_| {
        if let Ok(mode) = Mode::from_byte(mode.get_untracked()) {
            run(
                mode_ports.clone(),
                mode_alive.clone(),
                ui,
                vec![Command::SetMode(mode)],
            );
        }
    };
    let power_ports = ports.clone();
    let power_alive = alive.clone();
    let set_power = move |_| {
        let Ok(level) = power.get_untracked().parse::<u8>() else {
            ui.status
                .set("RF power must be a whole setting from 0 to 255".into());
            return;
        };
        run(
            power_ports.clone(),
            power_alive.clone(),
            ui,
            vec![Command::SetPower(level), Command::ReadPower],
        )
    };
    let read_ports = ports.clone();
    let read_alive = alive.clone();
    let read_power = move |_| {
        run(
            read_ports.clone(),
            read_alive.clone(),
            ui,
            vec![Command::ReadPower],
        )
    };

    let ptt_ports = ports.clone();
    let ptt_alive = alive.clone();
    let connect_ptt = move |_| {
        let handle = create();
        ptt_ports.borrow_mut().ptt = Some(handle.clone());
        ptt_busy.set(true);
        ptt_active.set(true);
        ptt_status.set("Choose the separate PTT interface port…".into());
        let pending = open(&handle, 9600, &method.get_untracked());
        let alive = ptt_alive.clone();
        let ports = ptt_ports.clone();
        let line = method.get_untracked();
        spawn_local(async move {
            let result = async {
                JsFuture::from(pending).await.map_err(error_text)?;
                JsFuture::from(set_line(&handle, &line, false))
                    .await
                    .map_err(error_text)?;
                Ok::<_, String>(())
            }
            .await;
            if !alive.get() || ports.borrow().ptt.as_ref() != Some(&handle) {
                return;
            }
            ptt_busy.set(false);
            match result {
                Ok(()) => {
                    ptt_ready.set(true);
                    ptt_status.set("PTT interface ready; receive requested".into());
                }
                Err(error) => {
                    abandon(&handle);
                    ptt_active.set(false);
                    ptt_status.set(error);
                }
            }
        });
    };
    let tx_ports = ports.clone();
    let tx_alive = alive.clone();
    let transmit = move |_| {
        let method = method.get_untracked();
        let handle = if method.starts_with("cat") {
            tx_ports.borrow().cat.as_ref().map(|p| p.handle.clone())
        } else {
            tx_ports.borrow().ptt.clone()
        };
        let Some(handle) = handle else {
            return;
        };
        let ports = tx_ports.clone();
        let alive = tx_alive.clone();
        // Arm before keying, including a possible lost ACK.
        let off = if method == "cat" {
            let addr = ports.borrow().cat.as_ref().unwrap().address;
            Transaction::new(addr, Command::SetTransmit(false))
                .unwrap()
                .request()
                .to_vec()
        } else {
            vec![]
        };
        arm(&handle, &off);
        ptt_busy.set(true);
        ui.tx.set(true);
        if method == "cat" {
            ui.busy.set(true);
        }
        spawn_local(async move {
            let result = if method == "cat" {
                let port = ports.borrow().cat.clone().unwrap();
                port.execute(Command::SetTransmit(true)).await.map(|_| ())
            } else {
                JsFuture::from(set_line(
                    &handle,
                    match method.as_str() {
                        "cat-rts" => "requestToSend",
                        "cat-dtr" => "dataTerminalReady",
                        _ => &method,
                    },
                    true,
                ))
                .await
                .map(|_| ())
                .map_err(error_text)
            };
            if !alive.get()
                || if method.starts_with("cat") {
                    ports
                        .borrow()
                        .cat
                        .as_ref()
                        .is_none_or(|p| p.handle != handle)
                } else {
                    ports.borrow().ptt.as_ref() != Some(&handle)
                }
            {
                return;
            }
            ptt_busy.set(false);
            ui.busy.set(false);
            match result {
                Ok(()) => ptt_status.set("TX requested · automatic release/disconnect after 30 s · radio state not sensed".into()),
                Err(error) => { abandon(&handle); ui.tx.set(false); ptt_status.set(format!("{error}; release attempted. Check radio.")); }
            }
        });
    };
    let rx_ports = ports.clone();
    let rx_alive = alive.clone();
    let receive = move |_| {
        // Closing releases either line or modern CAT PTT even if a command is pending.
        rx_ports.borrow_mut().close();
        ui.polling.set(false);
        ui.active.set(false);
        ui.busy.set(false);
        ui.tx.set(false);
        ptt_ready.set(false);
        ptt_active.set(false);
        ptt_busy.set(false);
        if rx_alive.get() {
            ui.status.set("Radio disconnected after RX request".into());
            ui.dial.set("Unknown".into());
            ui.frequency.set(None);
            ui.actual_mode.set("Unknown".into());
            ptt_status.set(
                "RX requested; ports closed. Verify receive on the radio, then reconnect.".into(),
            );
        }
    };
    view! {
        <section class="radio-panel monitor" aria-label="IC-718 radio control">
            <div class="toolbar"><h2>"Icom IC-718 control"</h2><strong class="radio-dial">{move || format!("{} · {}", ui.dial.get(), ui.actual_mode.get())}</strong></div>
            <div class="radio-controls">
                <label>"Baud "<select prop:value=move || baud.get().to_string() disabled=move || ui.active.get() on:change=move |e| { if let Ok(v) = event_target_value(&e).parse() { baud.set(v); } }>
                    {wsjt_rig::BAUD_RATES.iter().map(|b| view! { <option value=b.to_string()>{b.to_string()}</option> }).collect_view()}
                </select></label>
                <label>"CI-V hex "<input class="short" prop:value=move || address.get() disabled=move || ui.active.get() on:input=move |e| address.set(event_target_value(&e)) /></label>
                <button on:click=connect disabled=move || ui.active.get() || !supported()>"Connect radio"</button>
                <button class="secondary" on:click=disconnect disabled=move || !ui.active.get() && !ptt_active.get()>"Disconnect radio"</button>
            </div>
            <div class="radio-controls">
                <label>"Dial frequency (Hz) "<input inputmode="numeric" prop:value=move || frequency.get() on:input=move |e| frequency.set(event_target_value(&e)) /></label>
                <button on:click=tune disabled=move || !ui.active.get() || ui.busy.get() || ui.tx.get() || control.reserved.get()>"Set frequency"</button>
                <label>"FT8 preset "<select prop:value=move || frequency.get() on:change=move |e| { let value = event_target_value(&e); if !value.is_empty() { frequency.set(value); } }>
                    <option value="">"Choose band…"</option>
                    {[("80 m",3573000),("40 m",7074000),("30 m",10136000),("20 m",14074000),("17 m",18100000),("15 m",21074000),("12 m",24915000),("10 m",28074000)].into_iter().map(|(band,hz)| view! { <option value=hz.to_string()>{band}</option> }).collect_view()}
                </select></label>
                <label>"Mode "<select prop:value=move || mode.get().to_string() on:change=move |e| { if let Ok(v) = event_target_value(&e).parse() { mode.set(v); } }>
                    {Mode::ALL.into_iter().map(|m| view! { <option value=(m as u8).to_string()>{m.to_string()}</option> }).collect_view()}
                </select></label>
                <button on:click=set_mode disabled=move || !ui.active.get() || ui.busy.get() || ui.tx.get() || control.reserved.get()>"Set mode"</button>
            </div>
            <p class="status" role="status">{move || ui.status.get()}</p>
            <p class="status" hidden=move || storage_status.get().is_empty()>{move || storage_status.get()}</p>
            <div class="radio-controls">
                <label>"RF power setting (0–255) "<input class="short" type="number" min="0" max="255" prop:value=move || power.get().to_string() on:input=move |e| power.set(event_target_value(&e)) /></label>
                <button on:click=set_power disabled=move || !ui.active.get() || ui.busy.get() || ui.tx.get() || control.reserved.get()>"Set RF power"</button>
                <button class="secondary" on:click=read_power disabled=move || !ui.active.get() || ui.busy.get() || ui.tx.get() || control.reserved.get()>"Read RF power"</button>
                <span>{move || format!("Readback: {} (not watts)", ui.actual_power.get())}</span>
            </div>
            <div class="radio-controls">
                <label>"PTT method "<select prop:value=move || method.get() disabled=move || ptt_active.get() || ui.active.get() || ui.tx.get() on:change=move |e| method.set(event_target_value(&e))>
                    <option value="none">"Disabled"</option><option value="cat-rts">"RTS · same CI-V port"</option><option value="cat-dtr">"DTR · same CI-V port"</option><option value="requestToSend">"RTS · separate PTT interface"</option><option value="dataTerminalReady">"DTR · separate PTT interface"</option><option value="cat">"CAT · #82/#83 revisions only"</option>
                </select></label>
                <button class="secondary" on:click=connect_ptt disabled=move || !supported() || !matches!(method.get().as_str(), "requestToSend" | "dataTerminalReady") || ptt_active.get() || control.reserved.get()>"Connect PTT interface"</button>
                <button class="transmit" on:click=transmit disabled=move || control.reserved.get() || ptt_busy.get() || ui.tx.get() || if method.get().starts_with("cat") { !ui.active.get() || ui.busy.get() } else { !ptt_ready.get() }>"Transmit (30 s max)"</button>
                <button class="secondary" on:click=receive disabled=move || !ui.tx.get()>"Receive / unkey"</button>
            </div>
            <p class="status" role="status">{move || ptt_status.get()}</p>
            <p class="spectrum-detail">"Legacy IC-718 PTT needs an RTS/DTR interface wired to ACC SEND; a CI-V-only cable cannot key it. Match baud/address; turn CI-V 731 mode off. Presets fill the frequency field; Set frequency applies it. Select USB separately for FT8."</p>
            <p class="spectrum-detail">"PTT controls keying only; route transmit audio separately. RF power is an uncalibrated radio setting. Receive/unkey closes the ports. Release on disconnect or page exit is best effort; verify RX at the radio."</p>
        </section>
    }
}
