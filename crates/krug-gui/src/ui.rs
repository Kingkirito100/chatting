//! Интерфейс в стилистике Telegram Desktop: список чатов слева, облака
//! сообщений справа, обои и синий акцент.
//!
//! Всё считается в логических пикселях, рендер умножает на масштаб экрана.

use std::time::{Duration, Instant};

use chrono::{Datelike, Local, TimeZone};
use krug_client::{State, Status};
use krug_core::model::Message;

use crate::render::{Rect, Renderer};

pub const TEXT: u32 = 0x000000;
pub const MUTED: u32 = 0x999999;
pub const FAINT: u32 = 0xaaaaaa;
pub const ACCENT: u32 = 0x419fd9;
pub const ACCENT_DARK: u32 = 0x2f8fce;
pub const GREEN: u32 = 0x4dcd5e;
pub const RED: u32 = 0xe53935;
pub const CHAT_BG: u32 = 0xe6ebee;
pub const BUBBLE_IN: u32 = 0xffffff;
pub const BUBBLE_OUT: u32 = 0xeffdde;

const SIDEBAR: f32 = 330.0;
const HEADER: f32 = 56.0;
const INPUT_BAR: f32 = 56.0;

const AVATARS: [u32; 8] = [
    0xe17076, 0x7bc862, 0x6ec9cb, 0x65aadd, 0xa695e7, 0xee7aae, 0xf5a623, 0x4dcd5e,
];

const NAMES: [u32; 8] = [
    0xe17076, 0x7bc862, 0x6ec9cb, 0x65aadd, 0xa695e7, 0xee7aae, 0x4f9d56, 0xc75e8a,
];

fn avatar_color(id: u64) -> u32 {
    AVATARS[(id as usize) % AVATARS.len()]
}

fn name_color(id: u64) -> u32 {
    NAMES[(id as usize) % NAMES.len()]
}

fn status_color(presence: u8) -> u32 {
    match presence {
        1 => 0xf5a623,
        2 => 0xe53935,
        3 => 0x9e9e9e,
        _ => 0x4dcd5e,
    }
}

fn local(ts_ms: u64) -> chrono::DateTime<Local> {
    Local
        .timestamp_millis_opt(ts_ms as i64)
        .single()
        .unwrap_or_else(Local::now)
}

fn hhmm(ts_ms: u64) -> String {
    local(ts_ms).format("%H:%M").to_string()
}

const MONTHS: [&str; 12] = [
    "января",
    "февраля",
    "марта",
    "апреля",
    "мая",
    "июня",
    "июля",
    "августа",
    "сентября",
    "октября",
    "ноября",
    "декабря",
];

fn day_key(ts_ms: u64) -> (i32, u32, u32) {
    let date = local(ts_ms).date_naive();
    (date.year(), date.month(), date.day())
}

fn day_label(ts_ms: u64, now_ms: u64) -> String {
    let date = local(ts_ms).date_naive();
    let today = local(now_ms).date_naive();
    let diff = (today - date).num_days();
    if diff == 0 {
        "Сегодня".to_string()
    } else if diff == 1 {
        "Вчера".to_string()
    } else {
        format!("{} {}", date.day(), MONTHS[(date.month() - 1) as usize])
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn avatar(r: &mut Renderer, rect: Rect, id: u64, name: &str) {
    let radius = rect.w / 2.0;
    r.circle(rect.x + radius, rect.y + radius, radius, avatar_color(id));
    let initial = name.chars().next().unwrap_or('?').to_uppercase().to_string();
    let size = (rect.w * 0.42).max(11.0);
    r.text_centered(rect, &initial, size, 0xffffff, true);
}

fn plane(r: &mut Renderer, cx: f32, cy: f32, size: f32, color: u32, notch: u32) {
    r.triangle(
        (cx - size * 0.55, cy - size * 0.55),
        (cx - size * 0.55, cy + size * 0.55),
        (cx + size * 0.62, cy),
        color,
    );
    r.triangle(
        (cx - size * 0.55, cy - size * 0.55),
        (cx + size * 0.05, cy),
        (cx - size * 0.55, cy + size * 0.08),
        notch,
    );
}

/// Рисует стикер 1..=6 примитивами в квадрате `rect` (эталон — 64×64).
pub fn draw_sticker(r: &mut Renderer, id: u8, rect: Rect) {
    let u = rect.w.min(rect.h) / 64.0;
    let px = |x: f32| rect.x + x * u;
    let py = |y: f32| rect.y + y * u;
    let s = |v: f32| v * u;
    match id {
        // Смайл: жёлтый круг, глаза и дуга улыбки из точек.
        1 => {
            r.circle(px(32.0), py(32.0), s(28.0), 0xf7c948);
            r.circle(px(22.0), py(25.0), s(3.2), 0x5b4513);
            r.circle(px(42.0), py(25.0), s(3.2), 0x5b4513);
            for i in 0..5 {
                let t = i as f32 / 4.0;
                let angle = 0.55 + t * (std::f32::consts::PI - 1.1);
                r.circle(
                    px(32.0 + 13.0 * angle.cos()),
                    py(30.0 + 13.0 * angle.sin()),
                    s(1.9),
                    0x5b4513,
                );
            }
        }
        // Сердце: два круга и треугольник.
        2 => {
            r.circle(px(23.0), py(24.0), s(12.0), 0xe53960);
            r.circle(px(41.0), py(24.0), s(12.0), 0xe53960);
            r.triangle(
                (px(11.5), py(28.0)),
                (px(52.5), py(28.0)),
                (px(32.0), py(55.0)),
                0xe53960,
            );
            r.circle(px(24.0), py(21.0), s(3.5), 0xff7a95);
        }
        // Огонь: оранжевые треугольники и круги с жёлтой серединой.
        3 => {
            r.triangle(
                (px(32.0), py(4.0)),
                (px(10.0), py(38.0)),
                (px(54.0), py(38.0)),
                0xff7a18,
            );
            r.circle(px(32.0), py(40.0), s(21.0), 0xff7a18);
            r.triangle(
                (px(32.0), py(19.0)),
                (px(19.0), py(44.0)),
                (px(45.0), py(44.0)),
                0xffcf3b,
            );
            r.circle(px(32.0), py(44.0), s(11.0), 0xffcf3b);
            r.circle(px(32.0), py(44.0), s(4.0), 0xffe89a);
        }
        // Палец вверх: скруглённые прямоугольники и кружок.
        4 => {
            r.rounded(
                Rect::new(px(16.0), py(13.0), s(11.0), s(20.0)),
                s(5.5),
                0xf0b56b,
            );
            r.circle(px(21.5), py(14.0), s(5.5), 0xf0b56b);
            r.rounded(
                Rect::new(px(21.0), py(26.0), s(26.0), s(24.0)),
                s(7.0),
                0xf0b56b,
            );
            r.rounded(
                Rect::new(px(20.0), py(47.0), s(26.0), s(11.0)),
                s(4.0),
                0x4a90d9,
            );
            for y in [31.0, 36.0, 41.0] {
                r.rounded(
                    Rect::new(px(26.0), py(y), s(16.0), s(1.8)),
                    s(0.9),
                    0xd8994f,
                );
            }
        }
        // Звезда: треугольники-лучи вокруг центра.
        5 => {
            let mut points = [(0.0f32, 0.0f32); 10];
            for (i, point) in points.iter_mut().enumerate() {
                let angle = -std::f32::consts::FRAC_PI_2
                    + i as f32 * std::f32::consts::PI / 5.0;
                let radius = if i % 2 == 0 { 28.0 } else { 12.5 };
                *point = (32.0 + radius * angle.cos(), 33.0 + radius * angle.sin());
            }
            for i in 0..10 {
                let a = points[i];
                let b = points[(i + 1) % 10];
                r.triangle(
                    (px(32.0), py(33.0)),
                    (px(a.0), py(a.1)),
                    (px(b.0), py(b.1)),
                    0xffc93a,
                );
            }
            r.circle(px(32.0), py(33.0), s(4.5), 0xffb300);
        }
        // Вечеринка: лицо в колпаке и конфетти.
        6 => {
            let confetti = [
                (6.0, 10.0, 3.0, 6.0, 0x4a90d9),
                (54.0, 8.0, 3.0, 6.0, 0xe53960),
                (5.0, 35.0, 6.0, 3.0, 0x4dcd5e),
                (55.0, 33.0, 3.0, 6.0, 0xf5a623),
                (10.0, 55.0, 6.0, 3.0, 0x9b51e0),
                (50.0, 55.0, 6.0, 3.0, 0x6ec9cb),
                (44.0, 4.0, 3.0, 5.0, 0xf5a623),
            ];
            for (x, y, w, h, color) in confetti {
                r.rounded(
                    Rect::new(px(x), py(y), s(w), s(h)),
                    s(1.2),
                    color,
                );
            }
            r.circle(px(30.0), py(37.0), s(21.0), 0xffd93b);
            r.triangle(
                (px(18.0), py(23.0)),
                (px(38.0), py(23.0)),
                (px(28.0), py(4.0)),
                0x9b51e0,
            );
            r.circle(px(28.0), py(4.0), s(3.0), 0xffd93b);
            r.circle(px(23.0), py(33.0), s(2.8), 0x5b4513);
            r.circle(px(37.0), py(33.0), s(2.8), 0x5b4513);
            for i in 0..5 {
                let t = i as f32 / 4.0;
                let angle = 0.55 + t * (std::f32::consts::PI - 1.1);
                r.circle(
                    px(30.0 + 11.0 * angle.cos()),
                    py(38.0 + 11.0 * angle.sin()),
                    s(1.7),
                    0x5b4513,
                );
            }
        }
        _ => {
            r.rounded(rect, s(10.0), 0xe0e0e0);
            r.text_centered(rect, "?", s(28.0), 0xffffff, true);
        }
    }
}

// ---------------------------------------------------------------------------
// Вход

#[derive(Debug, Clone)]
pub struct LoginForm {
    pub server: String,
    pub name: String,
    pub password: String,
    pub invite: String,
    pub focus: usize,
    pub error: Option<String>,
    pub busy: bool,
}

impl LoginForm {
    pub fn new(server: &str, name: &str, invite: &str) -> LoginForm {
        LoginForm {
            server: server.to_string(),
            name: name.to_string(),
            password: String::new(),
            invite: invite.to_string(),
            focus: 1,
            error: None,
            busy: false,
        }
    }

    pub fn field(&self, index: usize) -> &str {
        match index {
            0 => &self.server,
            1 => &self.name,
            2 => &self.password,
            _ => &self.invite,
        }
    }

    pub fn field_mut(&mut self, index: usize) -> &mut String {
        match index {
            0 => &mut self.server,
            1 => &mut self.name,
            2 => &mut self.password,
            _ => &mut self.invite,
        }
    }

    pub fn push(&mut self, text: &str) {
        let focus = self.focus.min(3);
        self.field_mut(focus).push_str(text);
    }

    pub fn backspace(&mut self) {
        let focus = self.focus.min(3);
        self.field_mut(focus).pop();
    }

    pub fn next(&mut self) {
        self.focus = (self.focus + 1) % 4;
    }

    pub fn validate(&self) -> Option<String> {
        if self.server.trim().is_empty() {
            return Some("Укажи адрес сервера".into());
        }
        let name = self.name.trim();
        if name.chars().count() < 2 {
            return Some("Ник — хотя бы два символа".into());
        }
        if !krug_core::valid_name(name) {
            return Some("В нике можно буквы, цифры, пробел, _ - .".into());
        }
        if self.password.len() < 4 {
            return Some("Пароль — не короче 4 символов".into());
        }
        None
    }
}

#[derive(Debug, Clone, Copy)]
pub struct LoginLayout {
    pub fields: [Rect; 4],
    pub button: Rect,
}

impl LoginLayout {
    pub fn field_at(&self, x: f32, y: f32) -> Option<usize> {
        self.fields.iter().position(|f| f.contains(x, y))
    }
}

pub fn draw_login(
    r: &mut Renderer,
    form: &LoginForm,
    width: f32,
    height: f32,
    mouse: (f32, f32),
    time: f32,
) -> LoginLayout {
    r.background();
    r.pattern(
        Rect::new(0.0, 0.0, width, height),
        26.0,
        0xffffff,
        60,
    );

    let card_w = 400.0f32.min(width - 40.0);
    let card_h = 520.0f32.min(height - 36.0);
    let card = Rect::new((width - card_w) / 2.0, (height - card_h) / 2.0, card_w, card_h);
    r.shadow(card, 12.0, 30.0, 34);
    r.rounded(card, 12.0, 0xffffff);

    let logo = Rect::new(card.x + card.w / 2.0 - 32.0, card.y + 36.0, 64.0, 64.0);
    r.circle(logo.x + 32.0, logo.y + 32.0, 32.0, ACCENT);
    plane(r, logo.x + 32.0, logo.y + 32.0, 26.0, 0xffffff, ACCENT);

    r.text_centered(
        Rect::new(card.x, card.y + 112.0, card.w, 32.0),
        "Pakost Chatting",
        24.0,
        TEXT,
        true,
    );
    r.text_centered(
        Rect::new(card.x, card.y + 144.0, card.w, 20.0),
        "свой мессенджер для своих",
        13.0,
        MUTED,
        false,
    );

    let pad = 36.0;
    let field_w = card.w - pad * 2.0;
    let labels = ["Сервер", "Имя", "Пароль", "Код приглашения"];
    let placeholders = [
        "127.0.0.1:7777 или wss://…",
        "как тебя звать",
        "минимум 4 символа",
        "если сервер закрыт",
    ];
    let mut fields = [Rect::new(0.0, 0.0, 0.0, 0.0); 4];

    for i in 0..4 {
        let y = card.y + 184.0 + i as f32 * 62.0;
        r.text(card.x + pad, y, labels[i], 11.0, MUTED, false);
        let field = Rect::new(card.x + pad, y + 16.0, field_w, 42.0);
        fields[i] = field;
        let focused = form.focus == i;
        r.rounded(field, 8.0, 0xf1f3f5);
        if focused {
            r.stroke_rounded(field, 8.0, 1.5, ACCENT, 190);
        }

        let raw = form.field(i);
        let shown = if i == 2 {
            "•".repeat(raw.chars().count().min(32))
        } else {
            raw.to_string()
        };
        let text_x = field.x + 12.0;
        if shown.is_empty() {
            r.text(text_x, field.y + 11.0, placeholders[i], 14.0, FAINT, false);
        } else {
            let available = field.w - 24.0;
            let text_w = r.text_width(&shown, 14.0, false);
            let shift = if text_w > available { available - text_w } else { 0.0 };
            r.set_clip(Some(field.inset(1.0)));
            r.text(text_x + shift, field.y + 11.0, &shown, 14.0, TEXT, false);
            if focused && (time * 1.6) as i64 % 2 == 0 {
                let cursor_x = text_x + shift + text_w.min(available);
                r.rect(
                    Rect::new(cursor_x + 1.0, field.y + 11.0, 1.5, 19.0),
                    ACCENT,
                );
            }
            r.set_clip(None);
        }
    }

    let button = Rect::new(
        card.x + pad,
        card.y + 184.0 + 4.0 * 62.0 + 6.0,
        field_w,
        44.0,
    );
    let hover = button.contains(mouse.0, mouse.1) && !form.busy;
    let color = if form.busy {
        0xa9cfe6
    } else if hover {
        ACCENT_DARK
    } else {
        ACCENT
    };
    r.rounded(button, 8.0, color);
    if form.busy {
        let dots = ".".repeat((time * 2.0) as usize % 4);
        r.text_centered(button, &format!("Подключаюсь{dots}"), 15.0, 0xffffff, true);
    } else {
        r.text_centered(button, "Войти", 15.0, 0xffffff, true);
    }

    if let Some(error) = &form.error {
        let lines = r.wrap(error, 12.0, false, field_w);
        let mut y = button.bottom() + 8.0;
        for line in lines.iter().take(2) {
            r.text(card.x + pad, y, line, 12.0, RED, false);
            y += 16.0;
        }
    }

    r.text_centered(
        Rect::new(card.x, card.bottom() - 30.0, card.w, 18.0),
        "бесплатно · без рекламы · данные у вас",
        10.5,
        FAINT,
        false,
    );

    LoginLayout { fields, button }
}

// ---------------------------------------------------------------------------
// Чат

#[derive(Debug)]
struct Row {
    msg: Message,
    grouped: bool,
    day: Option<String>,
    lines: Vec<String>,
    height: f32,
    own: bool,
    bubble_w: f32,
    bubble_h: f32,
    name_h: f32,
}

#[derive(Debug)]
pub struct ChatUi {
    pub input: String,
    pub scroll: f32,
    pub typing_on: bool,
    pub creating: bool,
    pub room_input: String,
    pub room_error: Option<String>,
    pub search: String,
    pub search_focus: bool,
    pub settings_open: bool,
    pub handle_input: String,
    pub about_input: String,
    pub settings_focus: usize,
    pub presence: u8,
    pub settings_error: Option<String>,
    pub recording_started: Option<Instant>,
    /// Открыта ли панель выбора стикера.
    pub stickers_open: bool,
    last_typing_ping: Option<Instant>,
    rows: Vec<Row>,
    cache: (u64, u64, u32, u32),
    content_h: f32,
}

impl Default for ChatUi {
    fn default() -> Self {
        ChatUi {
            input: String::new(),
            scroll: 0.0,
            typing_on: false,
            creating: false,
            room_input: String::new(),
            room_error: None,
            search: String::new(),
            search_focus: false,
            settings_open: false,
            handle_input: String::new(),
            about_input: String::new(),
            settings_focus: 0,
            presence: 0,
            settings_error: None,
            recording_started: None,
            stickers_open: false,
            last_typing_ping: None,
            rows: Vec::new(),
            cache: (u64::MAX, u64::MAX, 0, 0),
            content_h: 0.0,
        }
    }
}

impl ChatUi {
    pub fn push(&mut self, text: &str) {
        self.input.push_str(text);
    }

    pub fn backspace(&mut self) {
        self.input.pop();
    }

    pub fn typing_due(&mut self) -> bool {
        if self.input.trim().is_empty() {
            return false;
        }
        let now = Instant::now();
        let due = self
            .last_typing_ping
            .map(|t| now.duration_since(t) >= Duration::from_secs(2))
            .unwrap_or(true);
        if due {
            self.last_typing_ping = Some(now);
        }
        due
    }

    pub fn take_input(&mut self) -> String {
        self.typing_on = false;
        self.last_typing_ping = None;
        self.stickers_open = false;
        std::mem::take(&mut self.input)
    }

    /// Закрыть панель стикеров (Esc, смена чата, настройки).
    pub fn close_stickers(&mut self) {
        self.stickers_open = false;
    }

    fn rebuild(&mut self, r: &mut Renderer, state: &State, max_bubble: f32, now: u64) {
        let key = (
            state.current.unwrap_or(0),
            state.last_seq,
            state.messages.len() as u32,
            max_bubble as u32,
        );
        if self.cache.0 != u64::MAX && self.cache.0 != key.0 {
            self.stickers_open = false;
        }
        if self.cache == key {
            return;
        }
        self.cache = key;
        self.rows.clear();
        let mut prev: Option<Message> = None;
        for msg in &state.messages {
            let new_day = prev
                .as_ref()
                .map(|p| day_key(p.ts_ms) != day_key(msg.ts_ms))
                .unwrap_or(true);
            let day = if new_day {
                Some(day_label(msg.ts_ms, now))
            } else {
                None
            };
            let grouped = !new_day
                && prev
                    .as_ref()
                    .map(|p| {
                        p.author_id == msg.author_id && msg.ts_ms.saturating_sub(p.ts_ms) < 300_000
                    })
                    .unwrap_or(false);
            let own = Some(msg.author_id) == state.me;
            let sticker = msg.sticker > 0;
            let lines = if sticker {
                Vec::new()
            } else {
                r.wrap(&msg.text, 14.5, false, max_bubble - 24.0)
            };
            let mut text_w = 0.0f32;
            for line in &lines {
                text_w = text_w.max(r.text_width(line, 14.5, false));
            }
            let (bubble_w, bubble_h, name_h) = if sticker {
                (72.0, 72.0, 0.0)
            } else {
                let bubble_w = (text_w + 24.0).clamp(72.0, max_bubble);
                let name_h = if !own && !grouped { 18.0 } else { 0.0 };
                let bubble_h = 8.0 + name_h + lines.len() as f32 * 19.0 + 16.0;
                (bubble_w, bubble_h, name_h)
            };
            let mut height = if sticker {
                84.0
            } else {
                bubble_h + if grouped { 3.0 } else { 8.0 }
            };
            if day.is_some() {
                height += 30.0;
            }
            self.rows.push(Row {
                msg: msg.clone(),
                grouped,
                day,
                lines,
                height,
                own,
                bubble_w,
                bubble_h,
                name_h,
            });
            prev = Some(msg.clone());
        }
        self.content_h = self.rows.iter().map(|row| row.height).sum::<f32>() + 8.0;
    }
}

#[derive(Debug, Default, Clone)]
pub struct ChatLayout {
    pub people: Vec<(u64, u64, Rect)>,
    pub input: Rect,
    pub send: Rect,
    pub add: Rect,
    pub search: Rect,
    pub modal_input: Rect,
    pub modal_ok: Rect,
    pub settings: Rect,
    pub handle_field: Rect,
    pub about_field: Rect,
    pub status_chips: [Rect; 4],
    pub save: Rect,
    pub call: Rect,
    pub accept: Rect,
    pub decline: Rect,
    pub mute: Rect,
    pub mic: Rect,
    pub voice: Vec<(u64, Rect)>,
    pub sticker_btn: Rect,
    pub sticker_cells: [Rect; 6],
}

pub fn draw_chat(
    r: &mut Renderer,
    state: &State,
    ui: &mut ChatUi,
    width: f32,
    height: f32,
    mouse: (f32, f32),
    time: f32,
) -> ChatLayout {
    let now = now_ms();
    r.background();
    let mut layout = ChatLayout::default();

    // Сайдбар
    r.rect(Rect::new(0.0, 0.0, SIDEBAR, height), 0xffffff);
    r.rect_alpha(Rect::new(SIDEBAR - 1.0, 0.0, 1.0, height), 0x000000, 10);
    r.text(16.0, 16.0, "Pakost Chatting", 17.0, TEXT, true);

    let add = Rect::new(0.0, 0.0, 0.0, 0.0);
    layout.add = add;

    // Поиск
    let search = Rect::new(12.0, 54.0, SIDEBAR - 24.0, 36.0);
    layout.search = search;
    r.rounded(search, 8.0, 0xf1f3f5);
    if ui.search_focus {
        r.stroke_rounded(search, 8.0, 1.5, ACCENT, 180);
    }
    let icon_x = search.x + 14.0;
    let icon_y = search.y + 18.0;
    r.circle(icon_x, icon_y, 6.0, 0x9aa6ad);
    r.circle(icon_x, icon_y, 4.2, 0xf1f3f5);
    r.rect(Rect::new(icon_x + 4.5, icon_y + 4.5, 6.0, 2.0), 0x9aa6ad);
    if ui.search.is_empty() {
        r.text(search.x + 30.0, search.y + 10.0, "Поиск", 13.5, 0x9aa6ad, false);
    } else {
        let text_w = r.text_width(&ui.search, 13.5, false);
        let available = search.w - 40.0;
        let shift = if text_w > available { available - text_w } else { 0.0 };
        r.set_clip(Some(search.inset(1.0)));
        r.text(
            search.x + 30.0 + shift,
            search.y + 10.0,
            &ui.search,
            13.5,
            TEXT,
            false,
        );
        r.set_clip(None);
    }

    // Список людей
    let query = ui.search.trim().to_lowercase();
    let mut people: Vec<&krug_core::model::User> = state
        .users
        .iter()
        .filter(|user| Some(user.id) != state.me)
        .filter(|user| query.is_empty() || user.name.to_lowercase().contains(&query))
        .collect();
    people.sort_by(|a, b| {
        b.status
            .cmp(&a.status)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    let mut y = 102.0;
    if people.is_empty() {
        r.text(24.0, y + 12.0, "Никого нет", 13.0, FAINT, false);
    }
    for user in &people {
        let row = Rect::new(0.0, y, SIDEBAR - 1.0, 64.0);
        let room = state.rooms.iter().find(|room| room.peer_id == user.id);
        let active = room.map(|room| Some(room.id) == state.current).unwrap_or(false);
        let hovered = row.contains(mouse.0, mouse.1);
        if active {
            r.rect(row, ACCENT);
        } else if hovered {
            r.rect(row, 0xf4f6f7);
        }
        avatar(
            r,
            Rect::new(10.0, y + 7.0, 50.0, 50.0),
            user.id,
            &user.name,
        );
        let name_color = if active { 0xffffff } else { TEXT };
        let sub_color = if active {
            0xd7ecf7
        } else if user.status == 0 {
            status_color(user.presence)
        } else {
            MUTED
        };
        r.text(72.0, y + 11.0, &user.name, 14.0, name_color, true);
        let status = if !user.about.is_empty() {
            user.about.clone()
        } else if user.status == 0 {
            match user.presence {
                1 => "не активен".to_string(),
                2 => "не беспокоить".to_string(),
                3 => "невидимый".to_string(),
                _ => "в сети".to_string(),
            }
        } else {
            "не в сети".to_string()
        };
        r.circle(77.0, y + 39.0, 3.5, status_color(user.presence));
        r.text(86.0, y + 32.0, &status, 13.0, sub_color, false);

        if let Some(room) = room {
            let unread = state.unread.get(&room.id).copied().unwrap_or(0);
            if unread > 0 {
                let badge = Rect::new(SIDEBAR - 38.0, y + 34.0, 22.0, 20.0);
                if active {
                    r.rounded(badge, 10.0, 0xffffff);
                    r.text_centered(badge, &unread.to_string(), 11.5, ACCENT, true);
                } else {
                    r.rounded(badge, 10.0, ACCENT);
                    r.text_centered(badge, &unread.to_string(), 11.5, 0xffffff, true);
                }
            }
        }
        layout.people.push((user.id, room.map(|room| room.id).unwrap_or(0), row));
        y += 64.0;
    }

    // Карточка себя
    let me = state
        .users
        .iter()
        .find(|user| Some(user.id) == state.me);
    let card_top = height - 72.0;
    r.rect_alpha(
        Rect::new(0.0, card_top, SIDEBAR - 1.0, 1.0),
        0x000000,
        10,
    );
    if let Some(me) = me {
        avatar(
            r,
            Rect::new(12.0, card_top + 17.0, 38.0, 38.0),
            me.id,
            &me.name,
        );
        r.circle(12.0 + 33.0, card_top + 50.0, 5.5, 0xffffff);
        r.circle(
            12.0 + 33.0,
            card_top + 50.0,
            4.0,
            status_color(me.presence),
        );
        r.text(58.0, card_top + 12.0, &me.name, 13.5, TEXT, true);
        r.text(
            58.0,
            card_top + 31.0,
            &format!("@{}", me.handle),
            11.0,
            MUTED,
            false,
        );
        let settings = Rect::new(SIDEBAR - 44.0, card_top + 19.0, 30.0, 30.0);
        layout.settings = settings;
        if settings.contains(mouse.0, mouse.1) {
            r.rounded(settings, 15.0, 0xf1f3f5);
        }
        for i in 0..3 {
            r.circle(
                settings.x + 15.0,
                settings.y + 9.0 + i as f32 * 6.0,
                1.6,
                0x8e8e93,
            );
        }
    }

    // Обои чата
    let chat = Rect::new(SIDEBAR, 0.0, width - SIDEBAR, height);
    r.rect(chat, CHAT_BG);
    r.pattern(chat, 28.0, 0xffffff, 70);

    // Шапка
    r.rect(Rect::new(SIDEBAR, 0.0, width - SIDEBAR, HEADER), 0xffffff);
    r.rect_alpha(
        Rect::new(SIDEBAR, HEADER - 1.0, width - SIDEBAR, 1.0),
        0x000000,
        10,
    );
    let peer = state
        .current_room()
        .and_then(|room| state.users.iter().find(|user| user.id == room.peer_id));
    let room_name = peer.map(|p| p.name.clone()).unwrap_or_else(|| "Pakost Chatting".into());
    let room_id = peer.map(|p| p.id).unwrap_or(1);
    if peer.is_some() {
        avatar(
            r,
            Rect::new(SIDEBAR + 12.0, 8.0, 40.0, 40.0),
            room_id,
            &room_name,
        );
    }
    r.text(SIDEBAR + 64.0, 11.0, &room_name, 15.0, TEXT, true);
    let typing: Vec<String> = state
        .typing_names()
        .iter()
        .take(2)
        .map(|s| s.to_string())
        .collect();
    let status = if !typing.is_empty() {
        format!("{} печатает…", typing[0])
    } else if peer.is_none() {
        "выбери собеседника".to_string()
    } else if peer.map(|p| p.status == 0).unwrap_or(false) {
        "в сети".to_string()
    } else {
        "не в сети".to_string()
    };
    let status_tint = if !typing.is_empty() {
        ACCENT
    } else if peer.map(|p| p.status == 0).unwrap_or(false) {
        GREEN
    } else {
        MUTED
    };
    r.text(SIDEBAR + 64.0, 31.0, &status, 12.0, status_tint, false);
    if peer.is_some() && state.call.is_none() {
        let call = Rect::new(width - 104.0, 12.0, 92.0, 32.0);
        layout.call = call;
        let hover = call.contains(mouse.0, mouse.1);
        r.rounded(call, 16.0, if hover { 0x3fae70 } else { 0x4dcd5e });
        r.text_centered(call, "Звонок", 12.5, 0xffffff, true);
    }

    // Поле ввода
    let input_bar = Rect::new(SIDEBAR, height - INPUT_BAR, width - SIDEBAR, INPUT_BAR);
    r.rect(input_bar, 0xffffff);
    r.rect_alpha(
        Rect::new(SIDEBAR, height - INPUT_BAR - 1.0, width - SIDEBAR, 1.0),
        0x000000,
        10,
    );
    let sticker_btn = Rect::new(SIDEBAR + 12.0, height - 44.0, 36.0, 36.0);
    layout.sticker_btn = sticker_btn;
    if ui.stickers_open {
        r.rounded(sticker_btn, 18.0, 0xeaf4fa);
    } else if sticker_btn.contains(mouse.0, mouse.1) {
        r.rounded(sticker_btn, 18.0, 0xf1f3f5);
    }
    let face_x = sticker_btn.x + 18.0;
    let face_y = sticker_btn.y + 18.0;
    r.circle(face_x, face_y, 11.0, 0xf5c542);
    r.circle(face_x - 4.0, face_y - 3.0, 1.6, 0x5b4513);
    r.circle(face_x + 4.0, face_y - 3.0, 1.6, 0x5b4513);
    for i in 0..3 {
        let t = i as f32 / 2.0;
        let angle = 0.7 + t * (std::f32::consts::PI - 1.4);
        r.circle(
            face_x + 5.5 * angle.cos(),
            face_y - 1.0 + 5.5 * angle.sin(),
            1.15,
            0x5b4513,
        );
    }

    let input = Rect::new(
        SIDEBAR + 54.0,
        height - 44.0,
        width - SIDEBAR - 118.0,
        36.0,
    );
    layout.input = input;
    r.rounded(input, 18.0, 0xf1f3f5);
    let text_x = input.x + 14.0;
    if ui.input.is_empty() {
        r.text(text_x, input.y + 9.0, "Сообщение", 14.5, 0x9aa6ad, false);
    } else {
        let available = input.w - 28.0;
        let text_w = r.text_width(&ui.input, 14.5, false);
        let shift = if text_w > available { available - text_w } else { 0.0 };
        r.set_clip(Some(input.inset(2.0)));
        r.text(text_x + shift, input.y + 9.0, &ui.input, 14.5, TEXT, false);
        if (time * 1.6) as i64 % 2 == 0 {
            r.rect(
                Rect::new(text_x + shift + text_w + 1.0, input.y + 9.0, 1.5, 19.0),
                ACCENT,
            );
        }
        r.set_clip(None);
    }
    let send = Rect::new(
        width - 52.0,
        height - 46.0,
        38.0,
        38.0,
    );
    layout.send = send;
    layout.mic = send;
    if !ui.input.is_empty() {
        r.circle(send.x + 19.0, send.y + 19.0, 19.0, ACCENT);
        r.triangle(
            (send.x + 12.0, send.y + 10.0),
            (send.x + 12.0, send.y + 28.0),
            (send.x + 29.0, send.y + 19.0),
            0xffffff,
        );
    } else if let Some(started) = ui.recording_started {
        r.circle(send.x + 19.0, send.y + 19.0, 19.0, 0xe53935);
        r.rounded(Rect::new(send.x + 14.0, send.y + 14.0, 10.0, 10.0), 2.0, 0xffffff);
        let secs = started.elapsed().as_secs_f32().min(1.5);
        r.text(input.x + 14.0, input.y + 9.0, &format!("Запись… {secs:.1}"), 14.0, 0xe53935, false);
    } else {
        r.circle(send.x + 19.0, send.y + 19.0, 19.0, 0x8e8e93);
        r.rounded(Rect::new(send.x + 16.0, send.y + 10.0, 6.0, 11.0), 3.0, 0xffffff);
        r.rounded(Rect::new(send.x + 13.0, send.y + 19.0, 12.0, 2.0), 1.0, 0xffffff);
        r.rounded(Rect::new(send.x + 18.0, send.y + 21.0, 2.0, 5.0), 1.0, 0xffffff);
    }

    // Сообщения
    let area = Rect::new(
        SIDEBAR + 10.0,
        HEADER + 6.0,
        width - SIDEBAR - 20.0,
        height - INPUT_BAR - HEADER - 12.0,
    );
    ui.rebuild(r, state, (area.w * 0.68).min(520.0), now);
    let max_scroll = (ui.content_h - area.h).max(0.0);
    ui.scroll = ui.scroll.clamp(0.0, max_scroll);
    let start_y = area.bottom() - ui.content_h + ui.scroll;
    r.set_clip(Some(Rect::new(SIDEBAR, HEADER, width - SIDEBAR, height - INPUT_BAR - HEADER)));

    if state.current.is_none() {
        r.text_centered(
            Rect::new(SIDEBAR, area.y, width - SIDEBAR, area.h),
            "Выбери человека слева",
            14.0,
            MUTED,
            false,
        );
    } else if state.messages.is_empty() {
        let title = if state.joined {
            "Здесь пока пусто"
        } else {
            "Загружаю историю…"
        };
        let pill = Rect::new(
            SIDEBAR + (width - SIDEBAR) / 2.0 - 90.0,
            area.y + area.h / 2.0 - 14.0,
            180.0,
            28.0,
        );
        r.rounded_alpha(pill, 14.0, 0x000000, 26);
        r.text_centered(pill, title, 12.5, 0xffffff, false);
    } else {
        let bubble_room = state.current.unwrap_or(0);
        let mut y = start_y;
        for row in &ui.rows {
            if y + row.height < area.y {
                y += row.height;
                continue;
            }
            if y > area.bottom() {
                break;
            }
            let mut top = y;
            if let Some(day) = &row.day {
                let w = r.text_width(day, 11.5, false) + 22.0;
                let pill = Rect::new(area.x + area.w / 2.0 - w / 2.0, y + 4.0, w, 22.0);
                r.rounded_alpha(pill, 11.0, 0x000000, 30);
                r.text_centered(pill, day, 11.5, 0xffffff, false);
                top = y + 30.0;
            }
            let sticker = row.msg.sticker > 0;
            top += if sticker {
                0.0
            } else if row.grouped {
                1.0
            } else {
                4.0
            };
            let bubble = if row.own {
                Rect::new(
                    area.right() - row.bubble_w,
                    top,
                    row.bubble_w,
                    row.bubble_h,
                )
            } else {
                Rect::new(area.x + 44.0, top, row.bubble_w, row.bubble_h)
            };
            if !row.own && !row.grouped {
                avatar(
                    r,
                    Rect::new(area.x + 6.0, bubble.y + 1.0, 30.0, 30.0),
                    row.msg.author_id,
                    &row.msg.author,
                );
            }
            if row.msg.voice {
                layout.voice.push((row.msg.seq, bubble));
            }
            if sticker {
                draw_sticker(r, row.msg.sticker, bubble);
                let time = hhmm(row.msg.ts_ms);
                let time_w = r.text_width(&time, 9.0, false);
                let mut right = bubble.right();
                if row.own {
                    let read = row.msg.seq <= state.peer_read(bubble_room);
                    let mark = if read { "✓✓" } else { "✓" };
                    let mark_w = r.text_width(mark, 9.0, false);
                    r.text(
                        right - mark_w,
                        bubble.bottom() + 1.0,
                        mark,
                        9.0,
                        if read { ACCENT } else { 0x999999 },
                        false,
                    );
                    right -= mark_w + 3.0;
                }
                r.text(
                    right - time_w,
                    bubble.bottom() + 1.0,
                    &time,
                    9.0,
                    if row.own { 0x6f9d55 } else { 0x999999 },
                    false,
                );
                y += row.height;
                continue;
            }
            let bubble_color = if row.own { BUBBLE_OUT } else { BUBBLE_IN };
            r.rounded(bubble, 10.0, bubble_color);
            if row.own {
                r.triangle(
                    (bubble.right() - 11.0, bubble.bottom() - 1.0),
                    (bubble.right() - 1.0, bubble.bottom() - 1.0),
                    (bubble.right() - 1.0, bubble.bottom() + 7.0),
                    bubble_color,
                );
            } else {
                r.triangle(
                    (bubble.x + 1.0, bubble.bottom() - 1.0),
                    (bubble.x + 11.0, bubble.bottom() - 1.0),
                    (bubble.x + 1.0, bubble.bottom() + 7.0),
                    bubble_color,
                );
            }
            let text_x = bubble.x + 12.0;
            if !row.own && !row.grouped {
                r.text(
                    text_x,
                    bubble.y + 7.0,
                    &row.msg.author,
                    13.0,
                    name_color(row.msg.author_id),
                    true,
                );
            }
            let text_y = bubble.y + 8.0 + row.name_h;
            for (i, line) in row.lines.iter().enumerate() {
                r.text(
                    text_x,
                    text_y + i as f32 * 19.0,
                    line,
                    14.5,
                    TEXT,
                    false,
                );
            }
            let time = hhmm(row.msg.ts_ms);
            let time_w = r.text_width(&time, 11.0, false);
            let mut right = bubble.right() - 10.0;
            if row.own {
                let read = row.msg.seq <= state.peer_read(bubble_room);
                let mark = if read { "✓✓" } else { "✓" };
                let mark_w = r.text_width(mark, 11.0, false);
                r.text(
                    right - mark_w,
                    bubble.bottom() - 16.0,
                    mark,
                    11.0,
                    if read { ACCENT } else { 0x999999 },
                    false,
                );
                right -= mark_w + 4.0;
            }
            r.text(
                right - time_w,
                bubble.bottom() - 16.0,
                &time,
                11.0,
                if row.own { 0x6f9d55 } else { 0x999999 },
                false,
            );
            y += row.height;
        }
    }
    r.set_clip(None);

    // Ошибка
    if state.status == Status::Online {
        if let Some(error) = &state.error {
            if now.saturating_sub(state.error_at) < 5000 {
                let text_w = r.text_width(error, 11.5, false).min(width - SIDEBAR - 40.0);
                let pill = Rect::new(
                    SIDEBAR + (width - SIDEBAR - text_w - 28.0) / 2.0,
                    HEADER + 10.0,
                    text_w + 28.0,
                    28.0,
                );
                r.rounded_alpha(pill, 14.0, 0x000000, 40);
                r.text_centered(pill, error, 11.5, 0xffffff, false);
            }
        }
    }

    // Панель стикеров над полем ввода: 2 ряда по 3.
    if ui.stickers_open {
        if state.current.is_none() || ui.settings_open || state.call.is_some() {
            ui.stickers_open = false;
        } else {
            let panel = Rect::new(
                SIDEBAR + 12.0,
                height - INPUT_BAR - 168.0,
                232.0,
                160.0,
            );
            r.shadow(panel, 12.0, 18.0, 30);
            r.rounded(panel, 12.0, 0xffffff);
            r.stroke_rounded(panel, 12.0, 1.0, 0x000000, 14);
            let mut cells = [Rect::new(0.0, 0.0, 0.0, 0.0); 6];
            for (i, cell) in cells.iter_mut().enumerate() {
                let col = i % 3;
                let row = i / 3;
                *cell = Rect::new(
                    panel.x + 12.0 + col as f32 * 72.0,
                    panel.y + 12.0 + row as f32 * 72.0,
                    64.0,
                    64.0,
                );
                let hovered = cell.contains(mouse.0, mouse.1);
                r.rounded(*cell, 10.0, if hovered { 0xf1f3f5 } else { 0xfafbfc });
                draw_sticker(r, (i + 1) as u8, *cell);
            }
            layout.sticker_cells = cells;
        }
    }

    // Экран звонка
    if let Some(call) = &state.call {
        r.rect_alpha(Rect::new(0.0, 0.0, width, height), 0x000000, 90);
        let card = Rect::new(width / 2.0 - 180.0, height / 2.0 - 150.0, 360.0, 300.0);
        r.shadow(card, 12.0, 26.0, 40);
        r.rounded(card, 12.0, 0xffffff);
        avatar(
            r,
            Rect::new(card.x + card.w / 2.0 - 36.0, card.y + 28.0, 72.0, 72.0),
            call.peer.id,
            &call.peer.name,
        );
        let name = &call.peer.name;
        let name_w = r.text_width(name, 18.0, true);
        r.text(
            card.x + (card.w - name_w) / 2.0,
            card.y + 112.0,
            name,
            18.0,
            TEXT,
            true,
        );
        let (label, color) = if call.active {
            let secs = now.saturating_sub(call.started_ms) / 1000;
            (format!("{:02}:{:02}", secs / 60, secs % 60), 0x4dcd5e)
        } else if call.incoming {
            ("входящий звонок".to_string(), MUTED)
        } else {
            ("звоним…".to_string(), MUTED)
        };
        let lw = r.text_width(&label, 13.0, false);
        r.text(
            card.x + (card.w - lw) / 2.0,
            card.y + 140.0,
            &label,
            13.0,
            color,
            false,
        );
        if call.active {
            let mute = Rect::new(card.x + 34.0, card.y + 210.0, 136.0, 44.0);
            let end = Rect::new(card.x + 190.0, card.y + 210.0, 136.0, 44.0);
            r.rounded(mute, 22.0, if call.muted { 0xe53935 } else { 0xf1f3f5 });
            r.text_centered(
                mute,
                if call.muted { "Микрофон выкл" } else { "Микрофон" },
                12.0,
                if call.muted { 0xffffff } else { TEXT },
                true,
            );
            r.rounded(end, 22.0, 0xe53935);
            r.text_centered(end, "Завершить", 12.5, 0xffffff, true);
            layout.mute = mute;
            layout.decline = end;
        } else if call.incoming {
            let accept = Rect::new(card.x + 34.0, card.y + 210.0, 136.0, 44.0);
            let decline = Rect::new(card.x + 190.0, card.y + 210.0, 136.0, 44.0);
            r.rounded(accept, 22.0, 0x4dcd5e);
            r.text_centered(accept, "Принять", 13.0, 0xffffff, true);
            r.rounded(decline, 22.0, 0xe53935);
            r.text_centered(decline, "Отклонить", 13.0, 0xffffff, true);
            layout.accept = accept;
            layout.decline = decline;
        } else {
            let end = Rect::new(card.x + card.w / 2.0 - 56.0, card.y + 210.0, 112.0, 40.0);
            r.rounded(end, 20.0, 0xe53935);
            r.text_centered(end, "Отменить", 13.0, 0xffffff, true);
            layout.decline = end;
        }
    }

    // Настройки профиля
    if ui.settings_open {
        r.rect_alpha(Rect::new(0.0, 0.0, width, height), 0x000000, 70);
        let card = Rect::new(width / 2.0 - 210.0, height / 2.0 - 225.0, 420.0, 450.0);
        r.shadow(card, 10.0, 26.0, 40);
        r.rounded(card, 10.0, 0xffffff);
        r.text(card.x + 22.0, card.y + 18.0, "Настройки", 17.0, TEXT, true);
        r.text(card.x + 22.0, card.y + 54.0, "Ник", 11.0, MUTED, false);
        let field = Rect::new(card.x + 22.0, card.y + 70.0, card.w - 44.0, 42.0);
        r.rounded(field, 8.0, 0xf1f3f5);
        let text_x = field.x + 26.0;
        r.text(field.x + 12.0, field.y + 11.0, "@", 14.0, MUTED, false);
        if ui.handle_input.is_empty() {
            r.text(text_x, field.y + 11.0, "username", 14.0, FAINT, false);
        } else {
            r.text(text_x, field.y + 11.0, &ui.handle_input, 14.0, TEXT, false);
        }
        r.text(
            card.x + 22.0,
            card.y + 116.0,
            "3–20 символов: латиница, цифры, _",
            10.5,
            FAINT,
            false,
        );
        r.text(card.x + 22.0, card.y + 138.0, "О себе", 11.0, MUTED, false);
        let about = Rect::new(card.x + 22.0, card.y + 154.0, card.w - 44.0, 42.0);
        r.rounded(about, 8.0, 0xf1f3f5);
        if ui.settings_focus == 1 {
            r.stroke_rounded(about, 8.0, 1.5, ACCENT, 160);
        }
        if ui.about_input.is_empty() {
            r.text(about.x + 12.0, about.y + 11.0, "пара слов о себе", 14.0, FAINT, false);
        } else {
            r.text(about.x + 12.0, about.y + 11.0, &ui.about_input, 14.0, TEXT, false);
        }
        r.text(card.x + 22.0, card.y + 206.0, "Статус", 11.0, MUTED, false);
        let chips = ["В сети", "Не активен", "Не беспокоить", "Невидимый"];
        let mut status_chips = [Rect::new(0.0, 0.0, 0.0, 0.0); 4];
        for i in 0..4 {
            let col = i % 2;
            let row = i / 2;
            let chip = Rect::new(
                card.x + 22.0 + col as f32 * 190.0,
                card.y + 224.0 + row as f32 * 40.0,
                178.0,
                32.0,
            );
            status_chips[i] = chip;
            let selected = ui.presence == i as u8;
            r.rounded(chip, 8.0, if selected { 0xeaf4fa } else { 0xf1f3f5 });
            if selected {
                r.stroke_rounded(chip, 8.0, 1.5, ACCENT, 160);
            }
            r.circle(chip.x + 12.0, chip.y + 16.0, 4.0, status_color(i as u8));
            r.text(chip.x + 22.0, chip.y + 9.0, chips[i], 12.5, TEXT, false);
        }
        if let Some(error) = &ui.settings_error {
            r.text(card.x + 22.0, card.y + 314.0, error, 11.0, RED, false);
        }
        let save = Rect::new(card.right() - 130.0, card.y + 382.0, 108.0, 40.0);
        let hover = save.contains(mouse.0, mouse.1);
        r.rounded(save, 8.0, if hover { ACCENT_DARK } else { ACCENT });
        r.text_centered(save, "Сохранить", 13.0, 0xffffff, true);
        r.text(
            card.x + 22.0,
            card.y + 392.0,
            "Tab — поле · Esc — закрыть",
            10.5,
            FAINT,
            false,
        );
        layout.handle_field = field;
        layout.about_field = about;
        if ui.settings_focus == 0 {
            r.stroke_rounded(field, 8.0, 1.5, ACCENT, 160);
        }
        layout.status_chips = status_chips;
        layout.save = save;
    }

    // Модалка новой комнаты
    if ui.creating {
        r.rect_alpha(Rect::new(0.0, 0.0, width, height), 0x000000, 60);
        let card = Rect::new(width / 2.0 - 200.0, height / 2.0 - 100.0, 400.0, 200.0);
        r.shadow(card, 10.0, 26.0, 40);
        r.rounded(card, 10.0, 0xffffff);
        r.text(card.x + 20.0, card.y + 16.0, "Новая комната", 16.0, TEXT, true);
        r.text(card.x + 20.0, card.y + 48.0, "Название", 11.0, MUTED, false);
        let field = Rect::new(card.x + 20.0, card.y + 64.0, card.w - 40.0, 42.0);
        r.rounded(field, 8.0, 0xf1f3f5);
        r.stroke_rounded(field, 8.0, 1.5, ACCENT, 160);
        let text_x = field.x + 12.0;
        if ui.room_input.is_empty() {
            r.text(text_x, field.y + 11.0, "например, мемы", 14.0, FAINT, false);
        } else {
            let available = field.w - 24.0;
            let text_w = r.text_width(&ui.room_input, 14.0, false);
            let shift = if text_w > available { available - text_w } else { 0.0 };
            r.set_clip(Some(field.inset(2.0)));
            r.text(text_x + shift, field.y + 11.0, &ui.room_input, 14.0, TEXT, false);
            if (time * 1.6) as i64 % 2 == 0 {
                r.rect(
                    Rect::new(text_x + shift + text_w + 1.0, field.y + 11.0, 1.5, 19.0),
                    ACCENT,
                );
            }
            r.set_clip(None);
        }
        if let Some(error) = &ui.room_error {
            r.text(card.x + 20.0, card.y + 112.0, error, 11.0, RED, false);
        }
        let ok = Rect::new(card.right() - 120.0, card.y + 142.0, 100.0, 38.0);
        let hover = ok.contains(mouse.0, mouse.1);
        r.rounded(ok, 8.0, if hover { ACCENT_DARK } else { ACCENT });
        r.text_centered(ok, "Создать", 13.0, 0xffffff, true);
        r.text(
            card.x + 20.0,
            card.y + 152.0,
            "Enter — создать · Esc — отмена",
            10.5,
            FAINT,
            false,
        );
        layout.modal_input = field;
        layout.modal_ok = ok;
    }

    let _ = time;
    layout
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_sticker_renders_inside_its_rect() {
        let mut r = Renderer::new(100, 100, 1.0);
        for id in 1..=6 {
            r.buf.fill(0);
            draw_sticker(&mut r, id, Rect::new(10.0, 10.0, 64.0, 64.0));
            assert!(
                r.buf.iter().any(|pixel| *pixel != 0),
                "стикер {id} ничего не нарисовал"
            );
            for y in 0..100 {
                for x in 0..100 {
                    if !(10..74).contains(&x) || !(10..74).contains(&y) {
                        assert_eq!(r.buf[y * 100 + x], 0, "стикер {id} вышел за рамки");
                    }
                }
            }
        }
    }
}
