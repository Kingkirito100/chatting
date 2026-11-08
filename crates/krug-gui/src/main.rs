//! Нативное окно «Круга»: winit + softbuffer + свой рендер.
//!
//! Никакого WebView: окно, ввод, шрифты и отрисовка — всё на Rust.

mod audio;
mod demo;
mod render;
mod ui;

use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::{Duration, Instant};

use krug_client::{Client, ClientConfig};
use krug_core::ClientKind;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowId};

use render::{write_png, Renderer};
use ui::{draw_chat, draw_login, ChatLayout, ChatUi, LoginForm, LoginLayout};

struct Args {
    server: String,
    name: String,
    invite: String,
    screenshot: Option<String>,
    screen: String,
    width: f32,
    height: f32,
    scale: f32,
    demo: bool,
    password: Option<String>,
    auto: bool,
    modal: bool,
    settings: bool,
    call: bool,
    audio_probe: bool,
}

fn parse_args() -> Args {
    let mut args = Args {
        server: "127.0.0.1:7777".into(),
        name: std::env::var("USER").unwrap_or_else(|_| "игрок".into()),
        invite: String::new(),
        screenshot: None,
        screen: "chat".into(),
        width: 1200.0,
        height: 800.0,
        scale: 2.0,
        demo: false,
        password: None,
        auto: false,
        modal: false,
        settings: false,
        call: false,
        audio_probe: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut take = || it.next().unwrap_or_default();
        match arg.as_str() {
            "--server" | "-s" => args.server = take(),
            "--name" | "-n" => args.name = take(),
            "--invite" | "-i" => args.invite = take(),
            "--screenshot" => args.screenshot = Some(take()),
            "--screen" => args.screen = take(),
            "--width" => args.width = take().parse().unwrap_or(1200.0),
            "--height" => args.height = take().parse().unwrap_or(800.0),
            "--scale" => args.scale = take().parse().unwrap_or(2.0),
            "--demo" => args.demo = true,
            "--password" | "-p" => args.password = Some(take()),
            "--auto" => args.auto = true,
            "--modal" => args.modal = true,
            "--settings" => args.settings = true,
            "--call" => args.call = true,
            "--audio-probe" => args.audio_probe = true,
            "-h" | "--help" => {
                println!(
                    "\
Круг — нативный клиент.

  krug-gui [--server АДРЕС] [--name НИК] [--invite КОД]

  --server  127.0.0.1:7777, wss://krug.onrender.com
  --name    ник
  --invite  код приглашения
  --password ПАРОЛЬ  подставить пароль
  --auto             сразу войти (удобно для проверки)
  --demo    окно с примером без сервера
  --screenshot файл.png [--screen chat|login] [--scale 2]

Горячие клавиши: Enter — отправить, Ctrl+Q — выйти."
                );
                std::process::exit(0);
            }
            _ => {}
        }
    }
    args
}

fn main() {
    let args = parse_args();
    if args.audio_probe {
        match audio::AudioEngine::start() {
            Ok(engine) => {
                std::thread::sleep(std::time::Duration::from_millis(900));
                println!("audio ok, rms={:.4}", engine.capture_rms());
            }
            Err(e) => {
                eprintln!("audio error: {e}");
                std::process::exit(1);
            }
        }
        return;
    }
    if let Some(path) = &args.screenshot {
        screenshot(&args, path);
        return;
    }
    let event_loop = EventLoop::new().expect("событийный цикл");
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut app = App::new(args);
    event_loop.run_app(&mut app).expect("окно");
}

fn screenshot(args: &Args, path: &str) {
    let scale = args.scale.max(1.0);
    let width = (args.width * scale) as usize;
    let height = (args.height * scale) as usize;
    let mut renderer = Renderer::new(width, height, scale);
    if args.screen == "login" {
        let mut form = LoginForm::new(&args.server, &args.name, &args.invite);
        form.password = "krug2026".into();
        form.focus = 1;
        draw_login(
            &mut renderer,
            &form,
            args.width,
            args.height,
            (0.0, 0.0),
            0.4,
        );
    } else {
        let mut state = demo::state();
        if args.call {
            if let Some(peer) = state.users.iter().find(|u| u.id == 2).cloned() {
                state.call = Some(krug_client::CallState {
                    peer,
                    incoming: true,
                    active: false,
                    started_ms: 0,
                    muted: false,
                });
            }
        }
        let mut ui = ChatUi::default();
        if args.modal {
            ui.creating = true;
            ui.room_input = "мемы".into();
        }
        if args.settings {
            ui.settings_open = true;
            ui.handle_input = "anya".into();
            ui.presence = 1;
        }
        draw_chat(
            &mut renderer,
            &state,
            &mut ui,
            args.width,
            args.height,
            (args.width * 0.4, args.height * 0.4),
            0.8,
        );
    }
    let path = std::path::Path::new(path);
    match write_png(path, width, height, &renderer.buf) {
        Ok(()) => println!("скриншот: {} ({width}×{height})", path.display()),
        Err(e) => {
            eprintln!("не смог сохранить скриншот: {e}");
            std::process::exit(1);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    Login,
    Chat,
}

type Surface = softbuffer::Surface<Arc<Window>, Arc<Window>>;
type Context = softbuffer::Context<Arc<Window>>;

struct App {
    auto: bool,
    window: Option<Arc<Window>>,
    surface: Option<Surface>,
    _context: Option<Context>,
    renderer: Renderer,
    screen: Screen,
    login: LoginForm,
    login_layout: LoginLayout,
    chat: ChatUi,
    chat_layout: ChatLayout,
    client: Option<Client>,
    demo: Option<krug_client::State>,
    mouse: (f32, f32),
    modifiers: ModifiersState,
    started: Instant,
    last_tick: Instant,
    audio: Option<audio::AudioEngine>,
    audio_error: Option<String>,
    record_buf: Vec<i16>,
    record_acc: f32,
}

impl App {
    fn new(args: Args) -> App {
        let mut login = LoginForm::new(&args.server, &args.name, &args.invite);
        if let Some(password) = &args.password {
            login.password = password.clone();
        }
        let demo = if args.demo { Some(demo::state()) } else { None };
        let screen = if demo.is_some() { Screen::Chat } else { Screen::Login };
        App {
            renderer: Renderer::new(1200, 800, 1.0),
            login_layout: LoginLayout {
                fields: [render::Rect::new(0.0, 0.0, 0.0, 0.0); 4],
                button: render::Rect::new(0.0, 0.0, 0.0, 0.0),
            },
            chat_layout: ChatLayout::default(),
            login,
            chat: ChatUi::default(),
            screen,
            client: None,
            demo,
            mouse: (0.0, 0.0),
            modifiers: ModifiersState::empty(),
            started: Instant::now(),
            last_tick: Instant::now(),
            audio: None,
            audio_error: None,
            record_buf: Vec::new(),
            record_acc: 0.0,
            auto: args.auto,
            window: None,
            surface: None,
            _context: None,
        }
    }

    fn submit_login(&mut self) {
        if self.login.busy {
            return;
        }
        if let Some(error) = self.login.validate() {
            self.login.error = Some(error);
            return;
        }
        self.login.error = None;
        self.login.busy = true;
        let mut cfg = ClientConfig::new(
            self.login.server.trim().to_string(),
            self.login.name.trim().to_string(),
            self.login.password.clone(),
        );
        cfg.invite = self.login.invite.trim().to_string();
        cfg.kind = ClientKind::Desktop;
        self.client = Some(Client::connect(cfg));
    }

    fn poll_client(&mut self) {
        let status = self
            .client
            .as_ref()
            .map(|client| client.read(|s| (s.status, s.error.clone())));
        match status {
            Some((krug_client::Status::Online, _)) => {
                if self.screen != Screen::Chat {
                    self.screen = Screen::Chat;
                }
                self.login.busy = false;
                if let Some(client) = &self.client {
                    client.mark_read();
                }
            }
            Some((krug_client::Status::Rejected, error)) => {
                self.login.error = error.or_else(|| Some("Сервер отклонил вход".into()));
                self.login.busy = false;
                self.client = None;
                self.screen = Screen::Login;
            }
            Some((krug_client::Status::Offline, error)) => {
                self.login.error = error;
            }
            _ => {}
        }
    }

    fn send_chat(&mut self) {
        let text = self.chat.take_input();
        if text.trim().is_empty() {
            return;
        }
        if let Some(client) = &self.client {
            client.send_message(&text);
            client.typing(false);
        }
        self.chat.typing_on = false;
    }

    fn open_settings(&mut self) {
        if let Some(client) = &self.client {
            let me = client.read(|state| {
                state
                    .users
                    .iter()
                    .find(|u| Some(u.id) == state.me)
                    .map(|u| (u.handle.clone(), u.about.clone(), u.presence))
            });
            if let Some((handle, about, presence)) = me {
                self.chat.handle_input = handle;
                self.chat.about_input = about;
                self.chat.presence = presence;
            }
        }
        self.chat.settings_error = None;
        self.chat.settings_open = true;
        self.chat.close_stickers();
    }

    fn save_settings(&mut self) {
        let handle = self
            .chat
            .handle_input
            .trim()
            .trim_start_matches('@')
            .to_lowercase();
        let valid = (3..=20).contains(&handle.chars().count())
            && handle
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
        if !valid {
            self.chat.settings_error =
                Some("Ник: 3–20 символов, латиница, цифры и _".into());
            return;
        }
        let about: String = self
            .chat
            .about_input
            .chars()
            .take(80)
            .collect::<String>();
        if let Some(client) = &self.client {
            client.set_profile(&handle, &about, self.chat.presence);
        }
        self.chat.settings_open = false;
        self.chat.settings_error = None;
    }

    fn redraw(&mut self) {
        let Some(window) = self.window.clone() else {
            return;
        };
        let size = window.inner_size();
        if size.width == 0 || size.height == 0 {
            return;
        }
        let scale = window.scale_factor() as f32;
        self.renderer.set_scale(scale);
        if self.renderer.width != size.width as usize || self.renderer.height != size.height as usize {
            self.renderer.resize(size.width as usize, size.height as usize);
        }
        let width = size.width as f32 / scale;
        let height = size.height as f32 / scale;
        let time = self.started.elapsed().as_secs_f32();

        match self.screen {
            Screen::Login => {
                self.login_layout = draw_login(
                    &mut self.renderer,
                    &self.login,
                    width,
                    height,
                    self.mouse,
                    time,
                );
            }
            Screen::Chat => {
                let owned;
                let state: &krug_client::State = match &self.client {
                    Some(client) => {
                        owned = client.snapshot();
                        &owned
                    }
                    None => match &self.demo {
                        Some(demo) => demo,
                        None => return,
                    },
                };
                self.chat_layout = draw_chat(
                    &mut self.renderer,
                    state,
                    &mut self.chat,
                    width,
                    height,
                    self.mouse,
                    time,
                );
            }
        }

        if let Some(surface) = &mut self.surface {
            let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
            else {
                return;
            };
            if let Err(e) = surface.resize(w, h) {
                eprintln!("surface: {e}");
                return;
            }
            match surface.buffer_mut() {
                Ok(mut buffer) => {
                    let n = buffer.len().min(self.renderer.buf.len());
                    buffer[..n].copy_from_slice(&self.renderer.buf[..n]);
                    if let Err(e) = buffer.present() {
                        eprintln!("present: {e}");
                    }
                }
                Err(e) => eprintln!("buffer: {e}"),
            }
        }
    }

    fn click(&mut self, x: f32, y: f32) {
        match self.screen {
            Screen::Login => {
                if let Some(index) = self.login_layout.field_at(x, y) {
                    self.login.focus = index;
                    self.login.error = None;
                } else if self.login_layout.button.contains(x, y) {
                    self.submit_login();
                }
            }
            Screen::Chat => {
                let in_call = self
                    .client
                    .as_ref()
                    .map(|c| c.read(|s| s.call.is_some()))
                    .unwrap_or(false);
                if in_call {
                    if self.chat_layout.accept.contains(x, y) {
                        if let Some(client) = &self.client {
                            client.accept_call();
                        }
                    } else if self.chat_layout.mute.contains(x, y) && self.chat_layout.mute.w > 0.0 {
                        if let Some(client) = &self.client {
                            client.toggle_mute();
                        }
                    } else if self.chat_layout.decline.contains(x, y) {
                        if let Some(client) = &self.client {
                            client.end_call();
                        }
                    }
                    return;
                }
                if self.chat_layout.call.contains(x, y) && self.chat_layout.call.w > 0.0 {
                    let peer = self
                        .client
                        .as_ref()
                        .and_then(|c| c.read(|s| s.current_room().and_then(|r| s.peer_of(r.id).map(|u| u.id))));
                    if let (Some(client), Some(peer)) = (&self.client, peer) {
                        client.call(peer);
                    }
                    return;
                }
                if self.chat.settings_open {
                    if self.chat_layout.handle_field.contains(x, y) {
                        self.chat.settings_focus = 0;
                        return;
                    }
                    if self.chat_layout.about_field.contains(x, y) {
                        self.chat.settings_focus = 1;
                        return;
                    }
                    for (i, chip) in self.chat_layout.status_chips.iter().enumerate() {
                        if chip.contains(x, y) {
                            self.chat.presence = i as u8;
                            return;
                        }
                    }
                    if self.chat_layout.save.contains(x, y) {
                        self.save_settings();
                    }
                    return;
                }
                if self.chat_layout.settings.contains(x, y)
                    && self.chat_layout.settings.w > 0.0
                {
                    self.open_settings();
                    return;
                }
                if self.chat_layout.sticker_btn.contains(x, y)
                    && self.chat_layout.sticker_btn.w > 0.0
                {
                    self.chat.stickers_open = !self.chat.stickers_open;
                    return;
                }
                if self.chat.stickers_open {
                    for (i, cell) in self.chat_layout.sticker_cells.iter().enumerate() {
                        if cell.contains(x, y) {
                            if let Some(client) = &self.client {
                                client.send_sticker((i + 1) as u8);
                                client.typing(false);
                            }
                            self.chat.close_stickers();
                            return;
                        }
                    }
                    self.chat.close_stickers();
                }
                if self.chat_layout.search.contains(x, y) {
                    self.chat.search_focus = true;
                    return;
                }
                for (seq, rect) in self.chat_layout.voice.clone() {
                    if rect.contains(x, y) {
                        let audio = self.client.as_ref().and_then(|c| {
                            c.read(|s| {
                                s.messages
                                    .iter()
                                    .find(|m| m.seq == seq)
                                    .map(|m| m.audio.clone())
                            })
                        });
                        if let Some(data) = audio {
                            if !data.is_empty() {
                                if self.audio.is_none() {
                                    self.audio = audio::AudioEngine::start().ok();
                                }
                                if let Some(engine) = &self.audio {
                                    engine.play_pcm(&data);
                                }
                            }
                        }
                        return;
                    }
                }
                if self.chat_layout.mic.contains(x, y)
                    && self.chat.input.is_empty()
                    && self.chat.recording_started.is_none()
                {
                    if self.audio.is_none() {
                        self.audio = audio::AudioEngine::start().ok();
                    }
                    if self.audio.is_some() {
                        self.record_buf.clear();
                        self.record_acc = 0.0;
                        self.chat.recording_started = Some(Instant::now());
                    }
                    return;
                }
                if self.chat_layout.send.contains(x, y) && !self.chat.input.is_empty() {
                    self.send_chat();
                    return;
                }
                if self.chat_layout.input.contains(x, y) {
                    self.chat.search_focus = false;
                    return;
                }
                for (user, room, rect) in &self.chat_layout.people {
                    if rect.contains(x, y) {
                        let user = *user;
                        let room = *room;
                        self.chat.search_focus = false;
                        self.chat.close_stickers();
                        if let Some(client) = &self.client {
                            if room != 0 {
                                client.join(room);
                            } else {
                                client.open_direct(user);
                            }
                        }
                        return;
                    }
                }
            }
        }
    }

    fn key(&mut self, event: &winit::event::KeyEvent) {
        let named = event.logical_key.clone();
        if self.screen == Screen::Chat && self.chat.settings_open {
            match &named {
                Key::Named(NamedKey::Enter) => self.save_settings(),
                Key::Named(NamedKey::Escape) => {
                    self.chat.settings_open = false;
                    self.chat.settings_error = None;
                }
                Key::Named(NamedKey::Backspace) => {
                    if self.chat.settings_focus == 0 {
                        self.chat.handle_input.pop();
                    } else {
                        self.chat.about_input.pop();
                    }
                }
                Key::Named(NamedKey::Tab) => {
                    self.chat.settings_focus = 1 - self.chat.settings_focus.min(1);
                }
                _ => {
                    if let Some(text) = &event.text {
                        if self.chat.settings_focus == 0 {
                            for ch in text.chars() {
                                if ch.is_ascii_alphanumeric() || ch == '_' || ch == '@' {
                                    self.chat.handle_input.push(ch.to_ascii_lowercase());
                                }
                            }
                        } else if text.chars().any(|c| !c.is_control()) {
                            self.chat.about_input.push_str(text);
                        }
                    }
                }
            }
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }
        if self.screen == Screen::Chat && self.chat.search_focus {
            match &named {
                Key::Named(NamedKey::Backspace) => {
                    self.chat.search.pop();
                }
                Key::Named(NamedKey::Escape) => {
                    self.chat.search.clear();
                    self.chat.search_focus = false;
                }
                Key::Named(NamedKey::Enter) => {
                    self.chat.search_focus = false;
                }
                _ => {
                    if let Some(text) = &event.text {
                        if text.chars().any(|c| !c.is_control()) {
                            self.chat.search.push_str(text);
                        }
                    }
                }
            }
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }
        let mods = self.modifiers;
        let command = mods.control_key() || mods.super_key();
        if command {
            if let Key::Character(ch) = &named {
                if ch.eq_ignore_ascii_case("q") {
                    std::process::exit(0);
                }
            }
            return;
        }
        match named {
            Key::Named(NamedKey::Backspace) => match self.screen {
                Screen::Login => {
                    self.login.backspace();
                    self.login.error = None;
                }
                Screen::Chat => self.chat.backspace(),
            },
            Key::Named(NamedKey::Enter) => match self.screen {
                Screen::Login => self.submit_login(),
                Screen::Chat => self.send_chat(),
            },
            Key::Named(NamedKey::Tab) => {
                if self.screen == Screen::Login {
                    self.login.next();
                }
            }
            Key::Named(NamedKey::Escape) => {
                if self.screen == Screen::Chat && self.chat.stickers_open {
                    self.chat.close_stickers();
                } else {
                    if self.screen == Screen::Chat {
                        if let Some(client) = &self.client {
                            if client.read(|s| s.call.is_some()) {
                                client.end_call();
                                return;
                            }
                        }
                    }
                    if self.screen == Screen::Login && self.login.busy {
                        self.client = None;
                        self.login.busy = false;
                        self.login.error = Some("Подключение отменено".into());
                    } else if self.screen == Screen::Chat {
                        self.chat.input.clear();
                    }
                }
            }
            Key::Named(NamedKey::ArrowUp) => {
                if self.screen == Screen::Chat {
                    self.chat.scroll += 60.0;
                }
            }
            Key::Named(NamedKey::ArrowDown) => {
                if self.screen == Screen::Chat {
                    self.chat.scroll -= 60.0;
                }
            }
            Key::Named(NamedKey::PageUp) => {
                if self.screen == Screen::Chat {
                    self.chat.scroll += 480.0;
                }
            }
            Key::Named(NamedKey::PageDown) => {
                if self.screen == Screen::Chat {
                    self.chat.scroll -= 480.0;
                }
            }
            _ => {
                if let Some(text) = &event.text {
                    if text.chars().any(|c| !c.is_control()) {
                        match self.screen {
                            Screen::Login => {
                                self.login.push(text);
                                self.login.error = None;
                            }
                            Screen::Chat => {
                                self.chat.push(text);
                                if self.chat.typing_due() {
                                    if let Some(client) = &self.client {
                                        client.typing(true);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title("Pakost Chatting — мессенджер")
            .with_inner_size(LogicalSize::new(1200.0, 800.0))
            .with_min_inner_size(LogicalSize::new(900.0, 600.0));
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(e) => {
                eprintln!("не смог создать окно: {e}");
                event_loop.exit();
                return;
            }
        };
        let context = match Context::new(window.clone()) {
            Ok(context) => context,
            Err(e) => {
                eprintln!("softbuffer: {e}");
                event_loop.exit();
                return;
            }
        };
        let surface = match Surface::new(&context, window.clone()) {
            Ok(surface) => surface,
            Err(e) => {
                eprintln!("softbuffer surface: {e}");
                event_loop.exit();
                return;
            }
        };
        self.surface = Some(surface);
        self._context = Some(context);
        self.window = Some(window);
        self.last_tick = Instant::now();
        if self.auto {
            self.submit_login();
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::Resized(_) => {
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
            }
            WindowEvent::CursorMoved { position, .. } => {
                let scale = self
                    .window
                    .as_ref()
                    .map(|w| w.scale_factor() as f32)
                    .unwrap_or(1.0);
                self.mouse = (position.x as f32 / scale, position.y as f32 / scale);
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                if self.screen == Screen::Chat {
                    let dy = match delta {
                        MouseScrollDelta::LineDelta(_, y) => y * 62.0,
                        MouseScrollDelta::PixelDelta(pos) => pos.y as f32,
                    };
                    self.chat.scroll += dy;
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                let (x, y) = self.mouse;
                self.click(x, y);
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::KeyboardInput { ref event, .. } => {
                if event.state == ElementState::Pressed {
                    self.key(event);
                }
            }
            WindowEvent::Ime(winit::event::Ime::Commit(text)) => {
                if self.screen == Screen::Chat {
                    self.chat.push(&text);
                } else {
                    self.login.push(&text);
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.poll_client();
        if let Some(started) = self.chat.recording_started {
            if let Some(engine) = &self.audio {
                let rate = engine.in_rate() as f32;
                let raw = engine.take_capture();
                let step = rate / 16_000.0;
                for sample in raw {
                    self.record_acc += 1.0;
                    while self.record_acc >= step {
                        self.record_acc -= step;
                        self.record_buf.push((sample.clamp(-1.0, 1.0) * 32767.0) as i16);
                    }
                }
            }
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            if started.elapsed() >= Duration::from_millis(1400) {
                let mut bytes = Vec::with_capacity(self.record_buf.len() * 2);
                for sample in &self.record_buf {
                    bytes.extend_from_slice(&sample.to_le_bytes());
                }
                if let Some(client) = &self.client {
                    client.send_voice_message(bytes);
                }
                self.chat.recording_started = None;
                self.record_buf.clear();
            }
        }
        let active = self
            .client
            .as_ref()
            .map(|c| c.read(|s| s.call.as_ref().map(|c| c.active).unwrap_or(false)))
            .unwrap_or(false);
        if active {
            if self.audio.is_none() {
                match audio::AudioEngine::start() {
                    Ok(engine) => self.audio = Some(engine),
                    Err(e) => self.audio_error = Some(e),
                }
            }
            let muted = self
                .client
                .as_ref()
                .and_then(|c| c.read(|s| s.call.as_ref().map(|c| c.muted)))
                .unwrap_or(false);
            if let Some(engine) = &mut self.audio {
                engine.muted = muted;
            }
            if let (Some(engine), Some(client)) = (&mut self.audio, &self.client) {
                engine.pump(client);
            }
        }
        let now = Instant::now();
        if now.duration_since(self.last_tick) >= Duration::from_millis(250) {
            self.last_tick = now;
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
        let step = if active {
            Duration::from_millis(20)
        } else {
            Duration::from_millis(250)
        };
        event_loop.set_control_flow(ControlFlow::WaitUntil(now + step));
    }
}
