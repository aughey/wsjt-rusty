//! One resource-owner thread serializes polls, operator commands, and PTT.
//! Its watchdog and cancellation remain active independently of egui repaints.
use crate::{
    output::{Authority, Output},
    preferences::{Preferences, PttMethod},
    runtime::utc_ms,
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread,
    time::{Duration, Instant},
};
use wsjt_receiver::contact_queue::Plan;
use wsjt_rig_native::{
    Command, Mode, Reply,
    native::{Ptt, PttLine, Radio},
};

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub connected: bool,
    pub ptt_ready: bool,
    pub frequency: Option<u64>,
    pub mode: Option<Mode>,
    pub power: Option<u8>,
    pub frequency_read_ms: f64,
}
pub enum Event {
    Radio(Snapshot),
    Armed(u64),
    Disarmed(u64),
    Playing(u64),
    Complete(u64),
    Canceled(u64),
    Error(String),
    Status(String),
    ContactFrequency(String, Option<(u64, f64)>),
}
pub enum Request {
    Connect(Preferences),
    Disconnect,
    Execute(Command),
    ManualTransmit(u64),
    Arm(u64, String),
    Disarm,
    Schedule(Plan, f32, bool),
    Cancel,
    ReadContactFrequency(String),
}
pub struct Hardware {
    commands: SyncSender<Request>,
    pub events: Receiver<Event>,
    pub authority: Arc<Authority>,
    alive: Arc<AtomicBool>,
}
impl Hardware {
    pub fn new() -> Self {
        let (commands, rx) = mpsc::sync_channel(32);
        let (events, inbox) = mpsc::sync_channel(128);
        let authority = Authority::new();
        let owner_authority = authority.clone();
        let alive = Arc::new(AtomicBool::new(true));
        let owner_alive = alive.clone();
        thread::spawn(move || {
            let mut engine = Engine::new(Native::default(), owner_authority, events);
            while owner_alive.load(Ordering::Acquire) {
                match rx.recv_timeout(Duration::from_millis(5)) {
                    Ok(request) => {
                        if !owner_alive.load(Ordering::Acquire) {
                            break;
                        }
                        engine.request(request);
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
                engine.tick();
            }
            engine.cancel(true);
            engine.backend.disconnect();
        });
        Self {
            commands,
            events: inbox,
            authority,
            alive,
        }
    }
    pub fn prepare(&self, name: String) -> Result<u64, String> {
        let token = self.authority.new_generation();
        self.send(Request::Arm(token, name))?;
        Ok(token)
    }
    pub fn send(&self, request: Request) -> Result<(), String> {
        self.commands
            .try_send(request)
            .map_err(|_| "Native hardware queue unavailable; retry after stopping.".into())
    }
    pub fn disarm(&self) {
        self.authority.new_generation();
        let _ = self.send(Request::Disarm);
    }
}
impl Drop for Hardware {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Release);
        self.authority.new_generation();
    }
}

trait Backend {
    fn connect(&mut self, p: &Preferences) -> Result<Snapshot, String>;
    fn disconnect(&mut self);
    fn execute(&mut self, c: Command) -> Result<Reply, String>;
    fn key(&mut self, on: bool) -> Result<(), String>;
    fn arm(&mut self, name: &str, authority: Arc<Authority>) -> Result<(), String>;
    fn close_output(&mut self);
    fn prepare(&mut self, p: &Plan, db: f32) -> Result<(), String>;
    fn play(&mut self, id: u64, start: Instant) -> Result<(), String>;
    fn progress(&self) -> Result<(bool, Option<Instant>), String>;
    fn cancel_audio(&mut self);
    fn reset_output(&mut self) -> Result<(), String>;
}
#[derive(Default)]
struct Native {
    radio: Option<Radio>,
    ptt: Option<Ptt>,
    method: PttMethod,
    output: Option<Output>,
    pcm: Option<Vec<f32>>,
    output_name: String,
    output_authority: Option<Arc<Authority>>,
}
impl Backend for Native {
    fn connect(&mut self, p: &Preferences) -> Result<Snapshot, String> {
        p.validate_radio()?;
        self.disconnect();
        self.method = p.ptt;
        let radio = match p.ptt {
            PttMethod::Rts => Radio::open_with_ptt(&p.port, p.baud, p.address()?, PttLine::Rts),
            PttMethod::Dtr => Radio::open_with_ptt(&p.port, p.baud, p.address()?, PttLine::Dtr),
            _ => Radio::open(&p.port, p.baud, p.address()?),
        }
        .map_err(|e| e.to_string())?;
        self.radio = Some(radio);
        if matches!(p.ptt, PttMethod::SeparateRts | PttMethod::SeparateDtr) {
            self.ptt = Some(
                Ptt::open(
                    &p.ptt_port,
                    if p.ptt == PttMethod::SeparateRts {
                        PttLine::Rts
                    } else {
                        PttLine::Dtr
                    },
                )
                .map_err(|e| e.to_string())?,
            );
        }
        let mut snapshot = Snapshot {
            connected: true,
            ptt_ready: p.ptt != PttMethod::None,
            ..Default::default()
        };
        update_snapshot(&mut snapshot, self.execute(Command::ReadFrequency)?);
        update_snapshot(&mut snapshot, self.execute(Command::ReadMode)?);
        update_snapshot(&mut snapshot, self.execute(Command::ReadPower)?);
        Ok(snapshot)
    }
    fn disconnect(&mut self) {
        let _ = self.key(false);
        self.ptt = None;
        self.radio = None;
    }
    fn execute(&mut self, c: Command) -> Result<Reply, String> {
        self.radio
            .as_mut()
            .ok_or("Radio disconnected")?
            .execute(c)
            .map_err(|e| e.to_string())
    }
    fn key(&mut self, on: bool) -> Result<(), String> {
        match self.method {
            PttMethod::None => {
                if on {
                    Err("Configure a PTT method first.".into())
                } else {
                    Ok(())
                }
            }
            PttMethod::Cat => self.execute(Command::SetTransmit(on)).map(|_| ()),
            PttMethod::Rts | PttMethod::Dtr => self
                .radio
                .as_mut()
                .ok_or("Radio disconnected")?
                .set_line_ptt(
                    if self.method == PttMethod::Rts {
                        PttLine::Rts
                    } else {
                        PttLine::Dtr
                    },
                    on,
                )
                .map_err(|e| e.to_string()),
            PttMethod::SeparateRts | PttMethod::SeparateDtr => self
                .ptt
                .as_mut()
                .ok_or("PTT interface disconnected")?
                .set(on)
                .map_err(|e| e.to_string()),
        }
    }
    fn arm(&mut self, name: &str, authority: Arc<Authority>) -> Result<(), String> {
        self.output_name = name.into();
        self.output_authority = Some(authority.clone());
        self.output = Some(Output::open(name, authority)?);
        Ok(())
    }
    fn close_output(&mut self) {
        self.output = None;
        self.pcm = None;
    }
    fn prepare(&mut self, p: &Plan, db: f32) -> Result<(), String> {
        if !db.is_finite() || !(0.0..=60.0).contains(&db) {
            return Err("Invalid transmit attenuation.".into());
        }
        let rate = self
            .output
            .as_ref()
            .ok_or("Audio output is not prepared")?
            .rate;
        let pcm =
            wsjt_rusty::ft8::transmit::audio(&p.message, p.frequency_hz).map_err(str::to_string)?;
        let mut pcm = wsjt_dsp::resample_up(&pcm, 12000, rate).map_err(str::to_string)?;
        let gain = 10.0_f32.powf(-db / 20.0);
        for sample in &mut pcm {
            *sample *= gain;
        }
        self.pcm = Some(pcm);
        Ok(())
    }
    fn play(&mut self, id: u64, start: Instant) -> Result<(), String> {
        let pcm = self.pcm.take().ok_or("No prepared transmit PCM")?;
        self.output
            .as_mut()
            .ok_or("Output disconnected")?
            .schedule(id, start, pcm)
    }
    fn progress(&self) -> Result<(bool, Option<Instant>), String> {
        self.output
            .as_ref()
            .ok_or("Output disconnected")?
            .progress()
    }
    fn cancel_audio(&mut self) {
        if let Some(mut output) = self.output.take() {
            output.cancel();
        }
        self.pcm = None;
    }
    fn reset_output(&mut self) -> Result<(), String> {
        self.output = Some(Output::open(
            &self.output_name,
            self.output_authority
                .clone()
                .ok_or("Output not configured")?,
        )?);
        Ok(())
    }
}

struct Reservation {
    id: u64,
    start: Instant,
    use_radio: bool,
    keyed: bool,
    submitted: bool,
    playing: bool,
}
struct Engine<B: Backend> {
    backend: B,
    authority: Arc<Authority>,
    events: SyncSender<Event>,
    snapshot: Snapshot,
    frame: Option<Reservation>,
    armed: bool,
    arm_token: u64,
    manual_deadline: Option<Instant>,
    next_poll: Instant,
}
impl<B: Backend> Engine<B> {
    fn new(backend: B, authority: Arc<Authority>, events: SyncSender<Event>) -> Self {
        Self {
            backend,
            authority,
            events,
            snapshot: Snapshot::default(),
            frame: None,
            armed: false,
            arm_token: 0,
            manual_deadline: None,
            next_poll: Instant::now(),
        }
    }
    fn emit(&self, event: Event) {
        if self.events.try_send(event).is_err() {
            self.authority.new_generation();
        }
    }
    fn fail(&mut self, error: String) {
        self.authority.revoke();
        self.cancel(true);
        self.backend.disconnect();
        self.snapshot = Snapshot::default();
        self.emit(Event::Radio(self.snapshot.clone()));
        self.emit(Event::Error(error));
    }
    fn cancel(&mut self, mut close: bool) {
        self.backend.cancel_audio();
        if (self.frame.as_ref().is_some_and(|f| f.use_radio && f.keyed)
            || self.manual_deadline.is_some())
            && let Err(error) = self.backend.key(false)
        {
            close = true;
            self.backend.disconnect();
            self.snapshot = Snapshot::default();
            self.emit(Event::Radio(self.snapshot.clone()));
            self.emit(Event::Error(format!("Unkey failed: {error}")));
        }
        if let Some(frame) = self.frame.take() {
            self.emit(Event::Canceled(frame.id));
        }
        self.manual_deadline = None;
        if !close
            && self.armed
            && let Err(error) = self.backend.reset_output()
        {
            self.armed = false;
            self.emit(Event::Error(format!("Output reset failed: {error}")));
        }
        if close {
            self.backend.close_output();
            self.armed = false;
            self.emit(Event::Disarmed(self.arm_token));
        }
    }
    fn request(&mut self, request: Request) {
        let result: Result<(), String> = (|| {
            match request {
                Request::ReadContactFrequency(id) => {
                    if self.frame.is_some()
                        || self.manual_deadline.is_some()
                        || !self.snapshot.connected
                    {
                        self.emit(Event::ContactFrequency(id, None));
                        return Ok(());
                    }
                    match self.backend.execute(Command::ReadFrequency) {
                        Ok(reply) => {
                            update_snapshot(&mut self.snapshot, reply);
                            self.emit(Event::Radio(self.snapshot.clone()));
                            self.emit(Event::ContactFrequency(
                                id,
                                self.snapshot
                                    .frequency
                                    .map(|hz| (hz, self.snapshot.frequency_read_ms)),
                            ));
                        }
                        Err(error) => {
                            self.emit(Event::ContactFrequency(id, None));
                            return Err(error);
                        }
                    }
                }
                Request::Connect(p) => {
                    self.authority.revoke();
                    self.cancel(true);
                    self.backend.disconnect();
                    self.snapshot = self.backend.connect(&p)?;
                    self.emit(Event::Radio(self.snapshot.clone()));
                    self.next_poll = Instant::now() + Duration::from_secs(2);
                }
                Request::Disconnect => {
                    self.authority.revoke();
                    self.cancel(true);
                    self.backend.disconnect();
                    self.snapshot = Snapshot::default();
                    self.emit(Event::Radio(self.snapshot.clone()));
                }
                Request::Execute(c) => {
                    if !self.snapshot.connected {
                        self.emit(Event::Status(
                            "Radio disconnected; reconnect before queued commands can run.".into(),
                        ));
                        return Ok(());
                    }
                    if self.frame.is_some() || self.manual_deadline.is_some() {
                        self.emit(Event::Status(
                            "Radio reserved for transmission; operator command skipped.".into(),
                        ));
                        return Ok(());
                    }
                    let reply = self.backend.execute(c)?;
                    update_snapshot(&mut self.snapshot, reply);
                    self.emit(Event::Radio(self.snapshot.clone()));
                    self.emit(Event::Status(format!("Radio: {reply:?}")));
                }
                Request::ManualTransmit(token) => {
                    if token != self.authority.generation() {
                        return Ok(());
                    }
                    self.arm_token = self.authority.generation();
                    if self.armed || self.frame.is_some() {
                        return Err("Stop contacting before manual PTT.".into());
                    }
                    // Record the release obligation BEFORE requesting keying, including failure.
                    self.manual_deadline = Some(Instant::now() + Duration::from_secs(30));
                    self.backend.key(true)?;
                    self.emit(Event::Status(
                        "PTT requested · 30-second watchdog active (RF not confirmed).".into(),
                    ));
                }
                Request::Arm(token, name) => {
                    if token != self.authority.generation() {
                        return Ok(());
                    }
                    self.authority.revoke();
                    self.cancel(true);
                    self.backend.arm(&name, self.authority.clone())?;
                    self.armed = true;
                    self.arm_token = token;
                    self.emit(Event::Armed(token));
                }
                Request::Disarm => {
                    self.authority.revoke();
                    self.cancel(true);
                }
                Request::Cancel => {
                    self.authority.revoke();
                    self.cancel(false);
                }
                Request::Schedule(plan, db, use_radio) => {
                    if !self.armed || self.frame.is_some() || !self.authority.allows(plan.id) {
                        return Err("Frame is not authorized or output is unavailable.".into());
                    }
                    let delay = (plan.start_ms - utc_ms()) / 1000.0;
                    if !delay.is_finite() || !(0.15..=31.0).contains(&delay) {
                        return Err("Transmit reservation missed its UTC deadline.".into());
                    }
                    if use_radio && !self.snapshot.ptt_ready {
                        return Err("Radio PTT is not connected/configured.".into());
                    }
                    let start = Instant::now() + Duration::from_secs_f64(delay);
                    self.backend.prepare(&plan, db)?;
                    self.frame = Some(Reservation {
                        id: plan.id,
                        start,
                        use_radio,
                        keyed: false,
                        submitted: false,
                        playing: false,
                    });
                }
            }
            Ok(())
        })();
        if let Err(error) = result {
            self.fail(error);
        }
    }
    fn tick(&mut self) {
        let result: Result<(), String> = (|| {
            let now = Instant::now();
            if (self.armed || self.manual_deadline.is_some())
                && self.arm_token != self.authority.generation()
            {
                self.cancel(true);
                return Ok(());
            }
            if self.manual_deadline.is_some_and(|deadline| now >= deadline) {
                self.cancel(false);
                self.emit(Event::Status("PTT watchdog released transmit.".into()));
            }
            if let Some(frame) = &mut self.frame {
                if !self.authority.allows(frame.id) {
                    self.cancel(false);
                    return Ok(());
                }
                if now > frame.start + Duration::from_secs(15) {
                    return Err("Transmit completion watchdog expired.".into());
                }
                if !frame.submitted
                    && (!frame.use_radio || now >= frame.start - Duration::from_millis(500))
                {
                    if now > frame.start - Duration::from_millis(50) {
                        return Err("Missed key/audio deadline.".into());
                    }
                    if frame.use_radio {
                        frame.keyed = true;
                        self.backend.key(true)?;
                        if !self.authority.allows(frame.id) {
                            self.cancel(false);
                            return Ok(());
                        }
                    }
                    if Instant::now() > frame.start - Duration::from_millis(25) {
                        return Err("PTT acknowledgment arrived too late; audio canceled.".into());
                    }
                    self.backend.play(frame.id, frame.start)?;
                    frame.submitted = true;
                }
                if frame.submitted {
                    let (playing, end) = self.backend.progress()?;
                    if playing && !frame.playing {
                        frame.playing = true;
                        if self.events.try_send(Event::Playing(frame.id)).is_err() {
                            self.authority.new_generation();
                        }
                    }
                    if end.is_some_and(|end| now >= end + Duration::from_millis(200)) {
                        let id = frame.id;
                        if frame.use_radio {
                            self.backend.key(false)?;
                        }
                        self.frame = None;
                        self.emit(Event::Complete(id));
                    }
                }
            } else if self.armed {
                self.backend.progress()?;
            }
            if self.snapshot.connected
                && self.frame.is_none()
                && self.manual_deadline.is_none()
                && now >= self.next_poll
                && utc_ms().rem_euclid(15000.0) < 12000.0
            {
                // Background polls never change operator-busy or readiness state.
                for c in [
                    Command::ReadFrequency,
                    Command::ReadMode,
                    Command::ReadPower,
                ] {
                    update_snapshot(&mut self.snapshot, self.backend.execute(c)?);
                }
                self.next_poll = Instant::now() + Duration::from_secs(2);
                self.emit(Event::Radio(self.snapshot.clone()));
            }
            Ok(())
        })();
        if let Err(error) = result {
            self.fail(error);
        }
    }
}
fn update_snapshot(s: &mut Snapshot, r: Reply) {
    match r {
        Reply::Frequency(hz) => {
            s.frequency = Some(hz);
            s.frequency_read_ms = utc_ms();
        }
        Reply::Mode(m) => s.mode = Some(m),
        Reply::Power(p) => s.power = Some(p),
        Reply::Accepted => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct Fake {
        ops: Vec<String>,
        output: bool,
        end: Option<Instant>,
        fail: bool,
        late: bool,
        revoke: Option<Arc<Authority>>,
    }
    impl Backend for Fake {
        fn connect(&mut self, _: &Preferences) -> Result<Snapshot, String> {
            self.ops.push("connect".into());
            Ok(Snapshot {
                connected: true,
                ptt_ready: true,
                ..Default::default()
            })
        }
        fn disconnect(&mut self) {
            self.ops.push("disconnect".into());
        }
        fn execute(&mut self, _: Command) -> Result<Reply, String> {
            self.ops.push("execute".into());
            if self.fail {
                Err("serial failure".into())
            } else {
                Ok(Reply::Frequency(14074000))
            }
        }
        fn key(&mut self, on: bool) -> Result<(), String> {
            self.ops.push(format!("key {on}"));
            if on {
                if let Some(a) = &self.revoke {
                    a.revoke();
                }
                if self.late {
                    thread::sleep(Duration::from_millis(450));
                }
                if self.fail {
                    return Err("key failure".into());
                }
            }
            Ok(())
        }
        fn arm(&mut self, _: &str, _: Arc<Authority>) -> Result<(), String> {
            self.ops.push("arm".into());
            self.output = true;
            Ok(())
        }
        fn close_output(&mut self) {
            self.ops.push("close".into());
            self.output = false;
        }
        fn prepare(&mut self, _: &Plan, _: f32) -> Result<(), String> {
            self.ops.push("prepare".into());
            Ok(())
        }
        fn play(&mut self, id: u64, _: Instant) -> Result<(), String> {
            self.ops.push(format!("play {id}"));
            Ok(())
        }
        fn progress(&self) -> Result<(bool, Option<Instant>), String> {
            Ok((self.end.is_some(), self.end))
        }
        fn cancel_audio(&mut self) {
            self.ops.push("cancel".into());
            self.output = false;
        }
        fn reset_output(&mut self) -> Result<(), String> {
            self.ops.push("reset".into());
            self.output = true;
            Ok(())
        }
    }
    fn reserved() -> (Engine<Fake>, Receiver<Event>) {
        let (events, rx) = mpsc::sync_channel(128);
        let a = Authority::new();
        a.approve(1);
        let mut e = Engine::new(
            Fake {
                output: true,
                ..Default::default()
            },
            a,
            events,
        );
        e.armed = true;
        e.snapshot = Snapshot {
            connected: true,
            ptt_ready: true,
            ..Default::default()
        };
        e.next_poll = Instant::now() + Duration::from_secs(60);
        e.frame = Some(Reservation {
            id: 1,
            start: Instant::now() + Duration::from_millis(400),
            use_radio: true,
            keyed: false,
            submitted: false,
            playing: false,
        });
        (e, rx)
    }
    #[test]
    fn key_ack_precedes_audio_and_completion_follows_unkey_without_closing_output() {
        let (mut e, rx) = reserved();
        e.tick();
        assert_eq!(e.backend.ops, ["key true", "play 1"]);
        e.backend.end = Some(Instant::now() - Duration::from_millis(201));
        e.tick();
        assert_eq!(e.backend.ops.last().unwrap(), "key false");
        assert!(e.backend.output);
        assert!(e.frame.is_none());
        assert!(rx.try_iter().any(|e| matches!(e, Event::Complete(1))));
    }
    #[test]
    fn canceled_pending_key_waits_for_ack_then_unkeys_without_audio_or_success() {
        let (mut e, rx) = reserved();
        e.backend.revoke = Some(e.authority.clone());
        e.tick();
        assert!(e.backend.ops.iter().any(|s| s == "key false"));
        assert!(!e.backend.ops.iter().any(|s| s.starts_with("play")));
        assert!(!rx.try_iter().any(|e| matches!(e, Event::Complete(_))));
        assert!(e.frame.is_none());
    }
    #[test]
    fn late_key_ack_and_failure_never_start_audio_and_both_release_ptt() {
        for late in [false, true] {
            let (mut e, rx) = reserved();
            e.backend.late = late;
            e.backend.fail = !late;
            e.tick();
            assert!(e.backend.ops.iter().any(|s| s == "key false"));
            assert!(!e.backend.ops.iter().any(|s| s.starts_with("play")));
            assert!(!rx.try_iter().any(|e| matches!(e, Event::Complete(_))));
        }
    }
    #[test]
    fn operator_commands_wait_out_reservations_and_failed_sessions_reject_queued_reuse() {
        let (mut e, _) = reserved();
        e.request(Request::Execute(Command::ReadMode));
        assert!(e.backend.ops.is_empty());
        e.frame = None;
        e.backend.fail = true;
        e.request(Request::Execute(Command::ReadFrequency));
        assert!(!e.snapshot.connected);
        let before = e.backend.ops.len();
        e.request(Request::Execute(Command::ReadMode));
        assert_eq!(e.backend.ops.len(), before);
    }
    #[test]
    fn manual_and_frame_watchdogs_release_without_ui_repaints() {
        let (mut e, _) = reserved();
        e.frame.as_mut().unwrap().keyed = true;
        e.frame.as_mut().unwrap().start = Instant::now() - Duration::from_secs(16);
        e.tick();
        assert!(e.backend.ops.iter().any(|s| s == "key false"));
        assert!(!e.armed);
        let (events, _) = mpsc::sync_channel(128);
        let mut e = Engine::new(Fake::default(), Authority::new(), events);
        e.manual_deadline = Some(Instant::now() - Duration::from_secs(1));
        e.tick();
        assert!(e.backend.ops.iter().any(|s| s == "key false"));
        assert!(e.manual_deadline.is_none());
    }
    #[test]
    fn canceled_startup_and_queued_manual_key_cannot_restore_authorization() {
        let (events, _) = mpsc::sync_channel(128);
        let a = Authority::new();
        let token = a.new_generation();
        a.new_generation();
        let mut e = Engine::new(Fake::default(), a, events);
        e.request(Request::Arm(token, "missing".into()));
        e.request(Request::ManualTransmit(token));
        assert!(e.backend.ops.is_empty());
        assert!(!e.armed);
    }

    #[test]
    fn shared_queue_and_native_executor_follow_report_rr73_and_farewell() {
        use wsjt_receiver::contact_queue::StationQueue;
        let payload = |text: &str| {
            wsjt_rusty::ft8::transmit::pack(text)
                .unwrap()
                .iter()
                .map(|b| char::from(b'0' + b))
                .collect::<String>()
        };
        let mut q = StationQueue::default();
        q.require_receive_progress();
        q.add("K1ABC".into());
        q.enabled = true;
        q.hear(
            "W9XYZ",
            500.0,
            "CQ K1ABC FN42",
            &payload("CQ K1ABC FN42"),
            -12.0,
            1200.0,
        );
        q.receive_slot_completed(0.0);
        let (mut engine, events) = reserved();
        engine.frame = None;
        for (time, expected) in [
            (14900.0, "K1ABC W9XYZ EN37"),
            (44900.0, "K1ABC W9XYZ R-08"),
            (74900.0, "K1ABC W9XYZ 73"),
        ] {
            let plan = q.plan(time, "W9XYZ", "EN37").unwrap();
            assert_eq!(plan.message, expected);
            assert!(q.can_transmit(plan.id));
            if time > 15000.0 {
                assert_eq!(plan.frequency_hz, 1400.0);
            }
            engine.authority.approve(plan.id);
            engine.backend.end = None;
            engine.backend.prepare(&plan, 20.0).unwrap();
            engine.frame = Some(Reservation {
                id: plan.id,
                start: Instant::now() + Duration::from_millis(400),
                use_radio: true,
                keyed: false,
                submitted: false,
                playing: false,
            });
            engine.tick();
            engine.backend.end = Some(Instant::now() - Duration::from_millis(201));
            engine.tick();
            assert!(
                events
                    .try_iter()
                    .any(|event| matches!(event,Event::Complete(id) if id==plan.id))
            );
            q.complete(plan.id, true);
            if time == 14900.0 {
                assert_eq!(q.items[0].contact_tries, 1);
                let text = "W9XYZ K1ABC -08";
                q.hear("W9XYZ", 30500.0, text, &payload(text), -8.0, 1400.0);
                q.receive_slot_completed(30000.0);
            } else if time == 44900.0 {
                let text = "W9XYZ K1ABC RR73";
                q.hear("W9XYZ", 60500.0, text, &payload(text), -7.0, 1400.0);
                q.receive_slot_completed(60000.0);
            }
        }
        assert!(q.items.is_empty());
        let record = q.log_pending.values().next().unwrap();
        assert_eq!(record.outcome, "Completed");
        assert_eq!(
            record
                .evidence
                .iter()
                .filter(|e| e.direction == "TX audio")
                .count(),
            3
        );
        assert_eq!(
            record
                .evidence
                .iter()
                .filter(|e| e.direction == "RX")
                .count(),
            3
        );
    }
}
