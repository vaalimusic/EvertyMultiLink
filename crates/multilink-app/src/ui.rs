use super::*;

const BG: Color = Color::rgba(13, 17, 23, 255);
const SIDE: Color = Color::rgba(16, 21, 28, 255);
const PANEL: Color = Color::rgba(22, 28, 36, 255);
const LINE: Color = Color::rgba(39, 47, 59, 255);
const TEXT: Color = Color::rgba(235, 241, 247, 255);
const MUTED: Color = Color::rgba(131, 145, 162, 255);
const ACCENT: Color = Color::rgba(111, 230, 180, 255);
const SOFT: Color = Color::rgba(28, 52, 45, 255);
const DANGER: Color = Color::rgba(240, 80, 80, 255);
const WARNING: Color = Color::rgba(240, 180, 0, 255);

#[derive(Default)]
pub(super) struct TextCache(std::collections::HashMap<(String, u64, bool), Arc<TextLayout>>);

#[derive(Default)]
pub(super) struct InputView {
    value: String,
    layout: Option<Arc<TextLayout>>,
    start: usize,
}
impl InputView {
    pub fn index_at(&self, x: f64) -> usize {
        self.start
            + self
                .layout
                .as_ref()
                .map(|l| l.index_at_x(x, self.value.len()))
                .unwrap_or(0)
    }
    fn paint(
        &mut self,
        edit: &TextEdit,
        scroll: &mut f64,
        rect: LogicalRect,
        focused: bool,
        caret_on: bool,
        hovered: bool,
        placeholder: &str,
        c: &mut Canvas<'_>,
    ) {
        // Shape only a small UTF-8 window, even when pasting a multi-MB payload.
        let raw = edit.text();
        let mut start = edit.caret().saturating_sub(768);
        while !raw.is_char_boundary(start) {
            start += 1;
        }
        let mut end = (edit.caret() + 768).min(raw.len());
        while !raw.is_char_boundary(end) {
            end -= 1;
        }
        let value = raw[start..end].replace(['\n', '\r', '\t'], " ");
        if self.layout.is_none() || self.value != value {
            self.layout = Some(Arc::new(c.text.shape(
                &value,
                &TextStyle {
                    font_size: LogicalPx(14.0),
                    line_height: LogicalPx(20.0),
                    ..TextStyle::default()
                },
                None,
            )));
            self.value = value;
        }
        self.start = start;
        let layout = self.layout.as_ref().unwrap().clone();
        let caret = layout.caret_x(edit.caret() - start);
        let inner = rect.size.width.0 - 28.0;
        *scroll = (*scroll).clamp((caret - inner + 2.0).max(0.0), caret.max(0.0));
        *scroll = (*scroll).min((layout.metrics.size.width.0 - inner + 4.0).max(0.0));
        c.b.fill_rect(rect, BG, 8.0);
        c.b.stroke_rect(
            rect,
            if focused {
                ACCENT
            } else if hovered {
                MUTED
            } else {
                LINE
            },
            if focused { 1.5 } else { 1.0 },
            8.0,
        );
        c.b.push_clip(rect.inset(Insets::all(2.0)));
        let left = rect.left() + 14.0;
        let top = rect.top() + (rect.size.height.0 - 20.0) / 2.0;
        if raw.is_empty() {
            c.text(left, top, placeholder, 14.0, MUTED, false);
        } else {
            if let Some((a, b)) = edit.selection() {
                let a = a.clamp(start, end) - start;
                let b = b.clamp(start, end) - start;
                let xa = layout.caret_x(a);
                let xb = layout.caret_x(b);
                c.rect(
                    left + xa - *scroll,
                    top,
                    (xb - xa).max(0.0),
                    20.0,
                    SOFT,
                    2.0,
                );
            }
            c.b.text(LogicalPoint::new(left - *scroll, top), layout, TEXT);
        }
        if focused && caret_on {
            c.rect(left + caret - *scroll, top + 1.0, 1.5, 18.0, ACCENT, 0.0);
        }
        let _ = c.b.pop_clip();
    }
}

struct Canvas<'a> {
    language: i18n::Language,
    tooltip: Option<(LogicalRect, String)>,
    b: PaintBuilder,
    text: &'a mut TextSystem,
    cache: &'a mut TextCache,
    hover: Option<Action>,
    pressed: Option<Action>,
    locked: bool,
    busy: bool,
    hits: Vec<(LogicalRect, Action)>,
    disabled_hits: Vec<(LogicalRect, String)>,
}
impl Canvas<'_> {
    fn disabled_reason(&self, action: Action) -> String {
        if self.busy {
            return "Подождите…".into();
        }
        match action {
            Action::ToggleSystemProxy | Action::ToggleTun | Action::ToggleIsolateLocalProxy => {
                "Отключите VPN для смены сетевого режима".into()
            }
            Action::Policy => "Отключите VPN для смены алгоритма выбора".into(),
            Action::ClearServers => "Отключите VPN для очистки списка серверов".into(),
            Action::Import => "Отключите VPN для добавления новых серверов".into(),
            Action::EditSubscriptionUrl(_)
            | Action::RenameSubscription(_)
            | Action::DeleteSubscription(_) => "Отключите VPN для изменения подписки".into(),
            Action::DisableAllServers => "Нельзя выключить все серверы при активном VPN".into(),
            _ => "Недоступно при активном VPN. Отключите VPN для изменения.".into(),
        }
    }
    fn icon_button(&mut self, x: f64, y: f64, icon: u32, action: Action, hint: &str) {
        self.button(x, y, 38.0, "", action, false);
        self.b.icon(
            IconId(icon),
            LogicalRect::from_xywh(x + 9.0, y + 9.0, 20.0, 20.0),
            if self.busy { MUTED } else { TEXT },
        );
        if self.hover == Some(action) {
            self.tooltip = Some((LogicalRect::from_xywh(x, y, 38.0, 38.0), hint.into()));
        }
    }
    fn toggle(&mut self, x: f64, y: f64, on: bool, action: Action) {
        self.button(x, y, 54.0, "", action, false);
        let enabled = match action {
            Action::ToggleAutostart
            | Action::ToggleAutoconnect
            | Action::ToggleRulesOnly
            | Action::ToggleIsolateLocalProxy => true,
            Action::Toggle(_) => !self.busy,
            _ => !self.locked,
        };
        self.rect(
            x + 5.0,
            y + 7.0,
            44.0,
            24.0,
            if on {
                if enabled {
                    ACCENT
                } else {
                    ACCENT.mix(PANEL, 0.5)
                }
            } else {
                LINE
            },
            12.0,
        );
        self.rect(
            x + if on { 28.0 } else { 8.0 },
            y + 10.0,
            18.0,
            18.0,
            if on { BG } else { MUTED },
            9.0,
        );
    }
    fn paint_tooltip(&mut self, w: f64, h: f64) {
        if let Some((anchor, label)) = self.tooltip.take() {
            let label = i18n::tr(self.language, &label);
            let width = (label.chars().count() as f64 * 6.6 + 24.0).clamp(120.0, 420.0);
            let x = anchor.left().min(w - width - 12.0).max(12.0);
            let y = if anchor.bottom() + 48.0 > h {
                anchor.top() - 42.0
            } else {
                anchor.bottom() + 7.0
            };
            self.b.set_layer(PaintLayer::Tooltip);
            self.card(x, y, width, 34.0);
            self.text(x + 12.0, y + 7.0, &label, 12.0, TEXT, false);
        }
    }
    fn rect(&mut self, x: f64, y: f64, w: f64, h: f64, color: Color, radius: f64) {
        self.b.fill_rect(
            LogicalRect::from_xywh(x, y, w.max(0.0), h.max(0.0)),
            color,
            radius,
        );
    }
    fn line(&mut self, x: f64, y: f64, w: f64) {
        self.rect(x, y, w, 1.0, LINE, 0.0);
    }
    fn card(&mut self, x: f64, y: f64, w: f64, h: f64) {
        let rect = LogicalRect::from_xywh(x, y, w, h);
        self.b.fill_rect(rect, PANEL, 12.0);
        self.b.stroke_rect(rect, LINE, 1.0, 12.0);
    }
    fn text(&mut self, x: f64, y: f64, value: &str, size: f64, color: Color, bold: bool) {
        let translated = i18n::tr(self.language, value);
        let value = translated.as_ref();
        self.raw_text(x, y, value, size, color, bold);
    }
    fn raw_text(&mut self, x: f64, y: f64, value: &str, size: f64, color: Color, bold: bool) {
        let style = TextStyle {
            family: FontFamily::Named("Segoe UI".into()),
            weight: if bold {
                FontWeight::Semibold
            } else {
                FontWeight::Regular
            },
            font_size: LogicalPx(size),
            line_height: LogicalPx(size * 1.4),
            ..TextStyle::default()
        };
        let key = (value.to_owned(), size.to_bits(), bold);
        let layout = if let Some(layout) = self.cache.0.get(&key) {
            layout.clone()
        } else {
            let layout = Arc::new(self.text.shape(value, &style, None));
            if self.cache.0.len() >= 512 {
                self.cache.0.clear();
            }
            self.cache.0.insert(key, layout.clone());
            layout
        };
        self.b.text(LogicalPoint::new(x, y), layout, color);
    }
    fn centered_text(&mut self, cx: f64, cy: f64, value: &str, size: f64, color: Color) {
        let translated = i18n::tr(self.language, value);
        let value = translated.as_ref();
        let style = TextStyle {
            family: FontFamily::Named("Segoe UI".into()),
            weight: FontWeight::Regular,
            font_size: LogicalPx(size),
            line_height: LogicalPx(size * 1.4),
            ..TextStyle::default()
        };
        let metrics = self.text.shape(value, &style, None).metrics;
        self.text(
            cx - metrics.size.width.0 / 2.0,
            cy - metrics.size.height.0 / 2.0,
            value,
            size,
            color,
            false,
        );
    }
    fn button(&mut self, x: f64, y: f64, w: f64, label: &str, action: Action, primary: bool) {
        let r = LogicalRect::from_xywh(x, y, w, 38.0);
        let enabled = match action {
            Action::Nav(_)
            | Action::Language
            | Action::SelectLanguage(_)
            | Action::HelpSection(_) => true,
            Action::OpenAboutSite | Action::OpenGitHub | Action::OpenDonate | Action::OpenTelegram | Action::CopyProxy => {
                true
            }
            Action::ShareNode(_) | Action::ShareSubscription(_) => true,
            Action::OpenLanSync | Action::CopyQrContent => !self.busy,
            Action::ToggleLanGateway => !self.locked,
            Action::ImportNodeFile => !self.locked,
            Action::ShareLanGateway | Action::CopyLanGatewayKey => !self.busy,
            Action::RankServers => true,
            Action::Compact
            | Action::Expanded
            | Action::ToggleAutostart
            | Action::ToggleAutoconnect
            | Action::ToggleRulesOnly
            | Action::OpenSetPin
            | Action::RemovePin
            | Action::LockPin
            | Action::CheckUpdate
            | Action::OpenUpdateLink
            | Action::ShowAntigravityPatches(_)
            | Action::CloseServerDiagnostics
            | Action::CloseAntigravityServersCheck
            | Action::ShowProxyIsolationDetails => true,
            Action::Connect
            | Action::Close
            | Action::Paste
            | Action::ChooseApplication
            | Action::Submit => !self.busy,
            Action::SelectRuleTarget(_) | Action::TargetPage(_) => !self.busy,
            Action::RefreshSubscription(_) => !self.busy,
            Action::ExportSettings | Action::ImportSettings => !self.busy,
            Action::Toggle(_) | Action::EnableAllServers | Action::DisableAllServers => !self.busy,
            Action::AddRule
            | Action::AddRuDomains
            | Action::AddProcessRule
            | Action::AddAntigravity
            | Action::RuleField(_)
            | Action::DeleteRule(_)
            | Action::EditRule(_)
            | Action::RuleNote
            | Action::ImportRoutes
            | Action::ExportRoutes
            | Action::ReplaceRoutes
            | Action::Policy
            | Action::RuleTarget
            | Action::ScanAntigravityPatches
            | Action::TogglePatchComponent(_)
            | Action::ApplyAntigravityPatches
            | Action::RestoreAntigravityPatches
            | Action::ToggleAntigravityAutoSelect
            | Action::OpenServerDiagnostics(_)
            | Action::RunServerDiagnostics(_)
            | Action::SetDiagnosticsPreset(_)
            | Action::OpenAntigravityServersCheck
            | Action::RunAntigravityServersCheck
            | Action::PingServer(_)
            | Action::CheckSingleAntigravityServer(_)
            | Action::SelectAntigravityServer(_)
            | Action::AntigravityCheckPage(_) => !self.busy,
            _ => !self.locked,
        };
        let hovered = enabled && self.hover == Some(action);
        let pressed = hovered && self.pressed == Some(action);
        let base = if primary {
            ACCENT
        } else {
            Color::rgba(32, 40, 50, 255)
        };
        let color = if !enabled {
            base.mix(PANEL, 0.65)
        } else if pressed {
            base.mix(BG, 0.22)
        } else if hovered {
            base.mix(Color::rgba(255, 255, 255, 255), 0.12)
        } else {
            base
        };
        self.b.fill_rect(r, color, 7.0);
        if hovered {
            self.b.stroke_rect(
                r,
                if primary {
                    ACCENT
                } else {
                    MUTED.with_opacity(0.6)
                },
                1.0,
                7.0,
            );
        }
        self.b.push_clip(r.inset(Insets::all(3.0)));
        self.text(
            x + 14.0,
            y + 8.0 + if pressed { 1.0 } else { 0.0 },
            label,
            14.0,
            if !enabled {
                MUTED
            } else if primary {
                BG
            } else {
                TEXT
            },
            true,
        );
        let _ = self.b.pop_clip();
        if enabled {
            self.hits.push((r, action));
        } else {
            let reason = self.disabled_reason(action);
            self.disabled_hits.push((r, reason));
        }
    }
    fn text_width(&mut self, value: &str, size: f64, bold: bool) -> f64 {
        let style = TextStyle {
            family: FontFamily::Named("Segoe UI".into()),
            weight: if bold {
                FontWeight::Semibold
            } else {
                FontWeight::Regular
            },
            font_size: LogicalPx(size),
            line_height: LogicalPx(size * 1.4),
            ..TextStyle::default()
        };
        self.text.shape(value, &style, None).metrics.size.width.0
    }
    fn badge_sized(&mut self, x: f64, y: f64, label: &str, active: bool, width: f64) {
        self.rect(
            x,
            y,
            width,
            26.0,
            if active {
                SOFT
            } else {
                Color::rgba(37, 43, 54, 255)
            },
            13.0,
        );
        self.text(
            x + 8.0,
            y + 4.5,
            label,
            11.5,
            if active { ACCENT } else { MUTED },
            false,
        );
    }
    fn badge(&mut self, x: f64, y: f64, label: &str, active: bool) {
        self.badge_sized(x, y, label, active, if active { 110.0 } else { 116.0 });
    }
    fn draw_availability_bars(&mut self, x: f64, y: f64, node: &multilink_core::Node, connected: bool) {
        let bar_count = 6;
        let bar_w = 3.5;
        let bar_gap = 2.0;
        let max_h = 13.0;
        let base_y = y + max_h;

        let samples: Vec<Option<u16>> = if !node.quality.history.is_empty() {
            let skip = node.quality.history.len().saturating_sub(bar_count);
            node.quality
                .history
                .iter()
                .skip(skip)
                .map(|&d| if d > 0 { Some(d) } else { None })
                .collect()
        } else if let Some(d) = node.delay.filter(|_| connected && node.enabled) {
            vec![Some(d as u16); bar_count.min(3)]
        } else {
            Vec::new()
        };

        for i in 0..bar_count {
            let bx = x + i as f64 * (bar_w + bar_gap);
            let sample = if samples.len() < bar_count {
                let pad = bar_count - samples.len();
                if i < pad {
                    None
                } else {
                    Some(samples[i - pad])
                }
            } else {
                Some(samples[i])
            };

            let (bh, color) = match sample {
                Some(Some(delay)) => {
                    if delay < 120 {
                        (13.0, ACCENT)
                    } else if delay < 250 {
                        (10.0, ACCENT)
                    } else if delay < 450 {
                        (7.0, WARNING)
                    } else {
                        (5.0, DANGER)
                    }
                }
                Some(None) => (3.0, DANGER),
                None => (2.5, Color::rgba(50, 60, 75, 255)),
            };

            self.rect(bx, base_y - bh, bar_w, bh, color, 1.0);
        }
    }
}
pub(crate) fn shortened(s: &str, n: usize) -> String {
    if s.chars().count() > n {
        format!("{}…", s.chars().take(n).collect::<String>())
    } else {
        s.to_owned()
    }
}
fn bytes(value: f64) -> String {
    if value >= 1024.0 * 1024.0 * 1024.0 {
        format!("{:.2} GB", value / (1024.0 * 1024.0 * 1024.0))
    } else if value >= 1024.0 * 1024.0 {
        format!("{:.1} MB", value / (1024.0 * 1024.0))
    } else {
        format!("{:.1} KB", value / 1024.0)
    }
}

fn draw_qr(c: &mut Canvas<'_>, x: f64, y: f64, size: f64, data: &str) {
    if let Ok(code) = qrcode::QrCode::new(data.as_bytes()) {
        let width = code.width();
        if width > 0 {
            let padding = 10.0;
            let qr_size = size - padding * 2.0;
            let cell = qr_size / width as f64;
            c.rect(x, y, size, size, Color::rgba(255, 255, 255, 255), 8.0);
            for cy in 0..width {
                for cx in 0..width {
                    if code[(cx, cy)] == qrcode::Color::Dark {
                        let px = x + padding + cx as f64 * cell;
                        let py = y + padding + cy as f64 * cell;
                        c.rect(
                            px,
                            py,
                            cell.ceil(),
                            cell.ceil(),
                            Color::rgba(0, 0, 0, 255),
                            0.0,
                        );
                    }
                }
            }
            return;
        }
    }
    c.rect(x, y, size, size, SOFT, 8.0);
    c.text(x + 20.0, y + size / 2.0, "QR Code", 14.0, MUTED, false);
}

impl App {
    pub(super) fn paint(&mut self, w: f64, h: f64) -> PaintList {
        let language = self.language();
        let route_rows: Vec<_> = if self.page == Page::Routing {
            let hidden: std::collections::HashSet<_> = self
                .settings
                .rule_groups
                .iter()
                .flat_map(|group| group.iter().skip(1))
                .collect();
            self.settings
                .rules
                .iter()
                .enumerate()
                .filter(|(_, rule)| !hidden.contains(&multilink_core::rule_key(rule)))
                .collect()
        } else {
            Vec::new()
        };
        let count = match self.page {
            Page::Servers | Page::Dashboard => self.settings.nodes.len(),
            Page::Routing => route_rows.len(),
            Page::Subscriptions => self.settings.subscriptions.len(),
            Page::Help => help::COUNT,
            Page::Settings => {
                let total_content_height: f64 = 1010.0;
                let step: f64 = 35.0;
                (total_content_height / step).ceil() as usize
            }
            _ => 0,
        };
        let (bar_x, top, height, visible) = match self.page {
            Page::Servers => (
                w - 48.0,
                265.0,
                (h - 368.0).max(40.0),
                ((h - 357.0) / 59.0).floor().max(1.0) as usize,
            ),
            Page::Routing => (
                w - 48.0,
                354.0,
                (h - 460.0).max(40.0),
                ((h - 458.0) / 58.0).floor().max(1.0) as usize,
            ),
            Page::Subscriptions => (
                w - 48.0,
                323.0,
                (h - 426.0).max(40.0),
                ((h - 424.0) / 116.0).floor().max(1.0) as usize,
            ),
            Page::Dashboard => (
                w - 48.0,
                548.0,
                (h - 652.0).max(35.0),
                (((h - 595.0).max(100.0) - 47.0) / 59.0).floor().max(1.0) as usize,
            ),
            Page::Help => (w - 48.0, 154.0, (h - 330.0).max(40.0), 1),
            Page::Settings => {
                let step: f64 = 35.0;
                let visible_height = (h - 200.0).max(80.0);
                (
                    w - 24.0,
                    120.0,
                    (h - 205.0).max(40.0),
                    (visible_height / step).floor().max(1.0) as usize,
                )
            }
            _ => (w - 48.0, 0.0, 0.0, 1),
        };
        self.offset = self.offset.min(count.saturating_sub(visible));
        self.scroll_bar = if !self.compact && self.modal.is_none() {
            scrolling::ScrollBar::new(
                LogicalRect::from_xywh(bar_x, top, 6.0, height),
                count,
                visible,
                self.offset,
            )
        } else {
            None
        };
        let rule_targets = if self
            .modal
            .as_ref()
            .is_some_and(|modal| *modal != Modal::Import)
        {
            self.rule_targets()
        } else {
            Vec::new()
        };
        let mut c = Canvas {
            language,
            tooltip: None,
            b: PaintBuilder::with_viewport(LogicalRect::from_xywh(0.0, 0.0, w, h)),
            text: &mut self.text,
            cache: &mut self.text_cache,
            hover: self.hover,
            pressed: self.pressed,
            locked: self.connected || self.busy,
            busy: self.busy,
            hits: Vec::new(),
            disabled_hits: Vec::new(),
        };
        if self.compact {
            let tun_active = self.connected && self.settings.tun;
            let mode_accent = if tun_active {
                Color::rgba(241, 194, 92, 255)
            } else {
                ACCENT
            };
            let mode_soft = if tun_active {
                Color::rgba(52, 43, 25, 255)
            } else {
                SOFT
            };
            c.rect(0.0, 0.0, w, h, BG, 0.0);
            c.b.image(
                LogicalRect::from_xywh(21.0, 16.0, 36.0, 36.0),
                64,
                64,
                self.logo.clone(),
            );
            c.text(67.0, 23.0, "EVERTY  /  MULTILINK", 12.0, MUTED, true);
            c.text(
                24.0,
                66.0,
                if self.connected {
                    "Подключено"
                } else {
                    "Не подключено"
                },
                24.0,
                TEXT,
                true,
            );
            c.text(
                24.0,
                104.0,
                if self.connected {
                    "Ваши маршруты активны"
                } else {
                    "Безопасный доступ в один клик"
                },
                13.0,
                MUTED,
                false,
            );
            let size = 154.0;
            let x = (w - size) / 2.0;
            let y = 155.0;
            let circle = LogicalRect::from_xywh(x, y, size, size);
            if tun_active {
                let elapsed = self
                    .started
                    .map(|time| time.elapsed().as_secs_f64())
                    .unwrap_or(0.0);
                for offset in [0.0, 0.5] {
                    let phase = (elapsed / 2.8 + offset).fract();
                    let spread = 4.0 + phase * 14.0;
                    c.b.stroke_rect(
                        LogicalRect::from_xywh(
                            x - spread,
                            y - spread,
                            size + spread * 2.0,
                            size + spread * 2.0,
                        ),
                        mode_accent.with_opacity(((1.0 - phase) * 0.25) as f32),
                        1.5,
                        size / 2.0 + spread,
                    );
                }
                c.rect(w - 96.0, 73.0, 72.0, 25.0, mode_soft, 12.5);
                c.text(w - 76.0, 77.0, "TUN", 12.0, mode_accent, true);
            }
            let hovered = c.hover == Some(Action::Connect);
            c.b.fill_rect(
                circle,
                if self.connected { mode_soft } else { PANEL },
                size / 2.0,
            );
            c.b.stroke_rect(
                circle,
                if self.connected || hovered {
                    mode_accent
                } else {
                    LINE
                },
                2.0,
                size / 2.0,
            );
            c.centered_text(
                x + size / 2.0,
                y + size / 2.0,
                "⏻",
                48.0,
                if self.connected || hovered {
                    mode_accent
                } else {
                    TEXT
                },
            );
            if !self.busy {
                c.hits.push((circle, Action::Connect));
            }
            c.text(
                w / 2.0 - 57.0,
                330.0,
                if self.busy {
                    "Подождите…"
                } else if self.connected {
                    "Отключиться"
                } else {
                    "Подключиться"
                },
                15.0,
                TEXT,
                true,
            );
            let elapsed = self.started.map(|time| time.elapsed().as_secs());
            let timer = elapsed
                .map(|seconds| {
                    format!(
                        "Время подключения  {:02}:{:02}:{:02}",
                        seconds / 3600,
                        (seconds / 60) % 60,
                        seconds % 60
                    )
                })
                .unwrap_or_else(|| "Время подключения  —".into());
            c.text(w / 2.0 - 92.0, 360.0, &timer, 12.0, MUTED, false);
            c.text(
                24.0,
                h - 116.0,
                &shortened(i18n::tr(language, &self.notice).as_ref(), 42),
                11.0,
                MUTED,
                false,
            );
            c.button(
                24.0,
                h - 76.0,
                w - 48.0,
                "Расширенные настройки  →",
                Action::Expanded,
                false,
            );
            self.hits = c.hits;
            return c.b.finish();
        }
        c.rect(0.0, 0.0, w, h, BG, 0.0);
        c.rect(0.0, 0.0, 226.0, h, SIDE, 0.0);
        c.b.image(
            LogicalRect::from_xywh(24.0, 29.0, 36.0, 36.0),
            64,
            64,
            self.logo.clone(),
        );
        c.text(72.0, 27.0, "EVERTY", 11.0, MUTED, true);
        c.text(71.0, 42.0, "MultiLink", 20.0, TEXT, true);
        c.text(26.0, 105.0, "РАБОЧЕЕ ПРОСТРАНСТВО", 10.0, MUTED, true);
        for (i, (page, icon, label)) in [
            (Page::Dashboard, "◈", "Обзор"),
            (Page::Access, "", "Доступ"),
            (Page::Servers, "≡", "Серверы"),
            (Page::Subscriptions, "↗", "Подписки"),
            (Page::Routing, "⇄", "Маршрутизация"),
            (Page::Statistics, "⌁", "Статистика"),
            (Page::Settings, "⚙", "Настройки"),
            (Page::About, "ⓘ", "О нас"),
            (Page::Help, "", "Инструкция"),
        ]
        .iter()
        .enumerate()
        {
            let y = 136.0 + i as f64 * 48.0;
            let active = self.page == *page;
            let hovered = c.hover == Some(Action::Nav(*page));
            if hovered && !active {
                c.rect(14.0, y, 198.0, 41.0, Color::rgba(27, 35, 44, 255), 7.0);
            }
            if active {
                c.rect(14.0, y, 198.0, 41.0, SOFT, 7.0);
                c.rect(14.0, y + 12.0, 3.0, 17.0, ACCENT, 1.5);
            }
            let _ = icon;
            c.b.icon(
                IconId(match page {
                    Page::Dashboard => 17,
                    Page::Access => 18,
                    Page::Servers => 16,
                    Page::Subscriptions => 4,
                    Page::Routing => 14,
                    Page::Statistics => 19,
                    Page::Settings => 15,
                    Page::About => 13,
                    Page::Help => 20,
                }),
                LogicalRect::from_xywh(28.0, y + 11.0, 19.0, 19.0),
                if active { ACCENT } else { MUTED },
            );
            c.text(
                62.0,
                y + 10.0,
                label,
                14.0,
                if active { ACCENT } else { MUTED },
                active,
            );
            c.hits.push((
                LogicalRect::from_xywh(14.0, y, 198.0, 41.0),
                Action::Nav(*page),
            ));
        }
        c.text(
            26.0,
            h - 43.0,
            concat!("Everty · ", env!("CARGO_PKG_VERSION")),
            11.0,
            MUTED,
            false,
        );
        let x = 260.0;
        let width = w - x - 34.0;
        let (title, sub) = match self.page {
            Page::Dashboard => ("Обзор сети", "Все маршруты под контролем."),
            Page::Access => ("Мой доступ", "Активация и срок подписки Everty."),
            Page::Servers => ("Серверы", "Управляйте узлами и доступностью маршрутов."),
            Page::Subscriptions => ("Подписки", "Одна ссылка — весь ваш серверный пул."),
            Page::Routing => (
                "Маршрутизация",
                "Удобные исключения для доменов, IP и подсетей.",
            ),
            Page::Statistics => (
                "Статистика",
                "Скорость, расход трафика и события подключения.",
            ),
            Page::Settings => ("Настройки", "Параметры подключения и состояние приложения."),
            Page::About => (
                "О нас",
                "Everty MultiLink · один клиент для ваших подключений.",
            ),
            Page::Help => ("Инструкция", "Пошаговые ответы: что включить и зачем."),
        };
        c.text(x, 28.0, title, 27.0, TEXT, true);
        if let Some(ver) = &self.update_available {
            c.button(
                w - 510.0,
                32.0,
                165.0,
                &format!("⚡ Доступно {ver}"),
                Action::OpenUpdateLink,
                true,
            );
        }
        c.button(w - 335.0, 32.0, 148.0, "Компактно", Action::Compact, false);
        c.text(x, 69.0, sub, 13.0, MUTED, false);
        c.badge(
            w - 167.0,
            37.0,
            if self.connected {
                "● Подключено"
            } else {
                "○ Отключено"
            },
            self.connected,
        );
        c.line(x, 105.0, width);
        match self.page {
            Page::Help => {
                let (title, body) = help::section(language, self.offset);
                c.card(x, 131.0, width, h - 225.0);
                c.text(x + 24.0, 155.0, title, 20.0, TEXT, true);
                let style = TextStyle {
                    family: FontFamily::Named("Segoe UI".into()),
                    font_size: LogicalPx(14.0),
                    line_height: LogicalPx(22.0),
                    ..TextStyle::default()
                };
                let layout = Arc::new(c.text.shape(body, &style, Some(LogicalPx(width - 80.0))));
                c.b.push_clip(LogicalRect::from_xywh(
                    x + 24.0,
                    201.0,
                    width - 80.0,
                    h - 390.0,
                ));
                c.b.text(LogicalPoint::new(x + 24.0, 201.0), layout, MUTED);
                let _ = c.b.pop_clip();
                c.text(
                    x + 24.0,
                    h - 135.0,
                    &format!("Раздел {} / {}", self.offset + 1, help::COUNT),
                    12.0,
                    MUTED,
                    false,
                );
                if self.offset > 0 {
                    c.button(
                        w - 328.0,
                        h - 149.0,
                        130.0,
                        "Назад",
                        Action::HelpSection(self.offset - 1),
                        false,
                    );
                }
                if self.offset + 1 < help::COUNT {
                    c.button(
                        w - 186.0,
                        h - 149.0,
                        130.0,
                        "Далее",
                        Action::HelpSection(self.offset + 1),
                        true,
                    );
                }
            }
            Page::Access => {
                c.card(x, 131.0, width, 228.0);
                c.b.icon(
                    IconId(18),
                    LogicalRect::from_xywh(x + 24.0, 157.0, 32.0, 32.0),
                    ACCENT,
                );
                let active = self
                    .settings
                    .access
                    .as_ref()
                    .filter(|a| !a.blocked && a.expires_at > multilink_core::unix_now());
                c.text(
                    x + 76.0,
                    157.0,
                    if active.is_some() {
                        "Подписка активна"
                    } else if self.settings.access.is_some() {
                        "Доступ приостановлен"
                    } else {
                        "Ваш личный доступ"
                    },
                    23.0,
                    TEXT,
                    true,
                );
                c.text(
                    x + 24.0,
                    215.0,
                    if let Some(a) = active {
                        &a.name
                    } else {
                        "Введите ключ из 6 символов, полученный от администратора."
                    },
                    14.0,
                    MUTED,
                    false,
                );
                if let Some(a) = &self.settings.access {
                    let date = chrono::DateTime::from_timestamp(a.expires_at as i64, 0)
                        .map(|d| {
                            d.with_timezone(&chrono::FixedOffset::east_opt(10800).unwrap())
                                .format("%d.%m.%Y · %H:%M UTC+3")
                                .to_string()
                        })
                        .unwrap_or_else(|| "—".into());
                    c.text(
                        x + 24.0,
                        248.0,
                        &format!("Доступ до {date}"),
                        14.0,
                        if active.is_some() { ACCENT } else { MUTED },
                        true,
                    );
                } else {
                    c.text(
                        x + 24.0,
                        248.0,
                        "Срок подписки начнётся с момента активации.",
                        13.0,
                        MUTED,
                        false,
                    );
                }
                c.button(
                    x + 24.0,
                    297.0,
                    220.0,
                    if self.settings.access.is_some() {
                        "Активировать новый ключ"
                    } else {
                        "Ввести ключ"
                    },
                    Action::ActivateAccess,
                    true,
                );
                if self.settings.access.is_some() {
                    c.icon_button(
                        x + 258.0,
                        297.0,
                        3,
                        Action::RefreshAccess,
                        "Обновить доступ и серверы",
                    );
                }
                c.card(x, 379.0, width, 146.0);
                c.text(
                    x + 24.0,
                    400.0,
                    "multilink.tolkovmeste.com",
                    17.0,
                    TEXT,
                    true,
                );
                c.text(
                    x + 24.0,
                    437.0,
                    "Ключ активируется один раз. Повторно вводить его при запуске не нужно.",
                    13.0,
                    MUTED,
                    false,
                );
                c.text(
                    x + 24.0,
                    470.0,
                    "Есть своя ссылка или сервер? Добавьте их во вкладке «Подписки».",
                    13.0,
                    MUTED,
                    false,
                );
                if self.settings.pending_activation.is_some() {
                    c.text(
                        x,
                        548.0,
                        "Активация не завершена. Откройте ввод ключа и повторите запрос.",
                        13.0,
                        ACCENT,
                        false,
                    );
                }
            }
            Page::Dashboard => {
                c.card(x, 130.0, width, 152.0);
                c.rect(x + 22.0, 151.0, 42.0, 42.0, SOFT, 12.0);
                c.text(x + 34.0, 157.0, "⇄", 24.0, ACCENT, true);
                c.text(
                    x + 78.0,
                    151.0,
                    if self.connected {
                        "Маршруты активны"
                    } else {
                        "Готов к подключению"
                    },
                    19.0,
                    TEXT,
                    true,
                );
                c.text(
                    x + 78.0,
                    181.0,
                    if self.connected {
                        "Трафик распределяется между доступными узлами"
                    } else if !self.settings.nodes.is_empty() {
                        "Нажмите «Подключиться», чтобы включить VPN"
                    } else {
                        "Добавьте серверы и запустите защищённый маршрут"
                    },
                    13.0,
                    MUTED,
                    false,
                );
                c.button(
                    w - 259.0,
                    157.0,
                    169.0,
                    if self.busy {
                        "Подождите…"
                    } else if self.connected {
                        "Отключиться"
                    } else {
                        "Подключиться →"
                    },
                    Action::Connect,
                    true,
                );
                c.line(x + 22.0, 216.0, width - 44.0);
                c.text(
                    x + 24.0,
                    237.0,
                    if self.settings.tun {
                        "TUN · IPv4"
                    } else if self.settings.system_proxy {
                        "СИСТЕМНЫЙ ПРОКСИ"
                    } else {
                        "ЛОКАЛЬНЫЙ ПРОКСИ"
                    },
                    10.0,
                    MUTED,
                    true,
                );
                c.text(
                    x + 175.0,
                    233.0,
                    if self.connected {
                        "Подключение активно"
                    } else {
                        "Ожидание подключения"
                    },
                    14.0,
                    TEXT,
                    false,
                );
                c.text(x + width - 295.0, 237.0, "", 11.0, MUTED, false);
                c.text(
                    x + width - 137.0,
                    237.0,
                    if self.connected && self.settings.tun && self.system_proxy_active {
                        "● TUN + Proxy"
                    } else if self.connected && self.settings.tun {
                        "● TUN"
                    } else if self.system_proxy_active {
                        "● Windows Proxy"
                    } else {
                        ""
                    },
                    11.0,
                    ACCENT,
                    false,
                );
                let gap = 14.0;
                let cw = (width - 2.0 * gap) / 3.0;
                for (i, (label, value, detail)) in [
                    (
                        "СЕРВЕРЫ В ПУЛЕ",
                        format!(
                            "{} / {}",
                            self.settings.nodes.iter().filter(|n| n.enabled).count(),
                            self.settings.nodes.len()
                        ),
                        "включено / импортировано".to_owned(),
                    ),
                    (
                        "↓  ЗАГРУЗКА",
                        if self.connected {
                            format!("{}/s", bytes(self.metrics.down_rate))
                        } else {
                            "—".into()
                        },
                        "текущая скорость".into(),
                    ),
                    (
                        "↑  ОТДАЧА",
                        if self.connected {
                            format!("{}/s", bytes(self.metrics.up_rate))
                        } else {
                            "—".into()
                        },
                        "текущая скорость".into(),
                    ),
                ]
                .iter()
                .enumerate()
                {
                    let cx = x + i as f64 * (cw + gap);
                    c.card(cx, 300.0, cw, 125.0);
                    c.text(cx + 19.0, 317.0, label, 11.0, MUTED, true);
                    c.text(
                        cx + 19.0,
                        343.0,
                        value,
                        29.0,
                        if i == 1 { ACCENT } else { TEXT },
                        true,
                    );
                    c.text(cx + 19.0, 393.0, detail, 11.0, MUTED, false);
                }
                c.text(x, 450.0, "Пул серверов", 18.0, TEXT, true);
                c.text(
                    x + 170.0,
                    455.0,
                    &format!("{} узлов", self.settings.nodes.len()),
                    12.0,
                    MUTED,
                    false,
                );
                c.button(
                    w - 190.0,
                    444.0,
                    156.0,
                    "Все серверы →",
                    Action::Nav(Page::Servers),
                    false,
                );
                server_list(
                    &mut c,
                    &self.settings,
                    self.connected,
                    self.offset,
                    x,
                    500.0,
                    width,
                    (h - 595.0).max(100.0),
                );
            }
            Page::Servers => {
                c.text(
                    x,
                    137.0,
                    &format!(
                        "{} серверов · {} включено",
                        self.settings.nodes.len(),
                        self.settings.nodes.iter().filter(|n| n.enabled).count()
                    ),
                    15.0,
                    TEXT,
                    true,
                );
                c.button(w - 159.0, 128.0, 125.0, "+ Добавить", Action::Import, true);
                c.button(
                    w - 292.0,
                    128.0,
                    125.0,
                    "📥 Принять QR",
                    Action::OpenLanSync,
                    false,
                );
                c.icon_button(
                    w - 342.0,
                    128.0,
                    10,
                    Action::RankServers,
                    if self.settings.rank_servers {
                        "Сортировка по оценке включена"
                    } else {
                        "Сортировать по оценке"
                    },
                );
                c.icon_button(
                    w - 386.0,
                    128.0,
                    9,
                    Action::DisableAllServers,
                    "Выключить все серверы",
                );
                c.icon_button(
                    w - 430.0,
                    128.0,
                    8,
                    Action::EnableAllServers,
                    "Включить все серверы",
                );
                c.text(
                    x,
                    177.0,
                    if self.connected {
                        "Чтобы изменить серверы, сначала отключитесь."
                    } else {
                        "Нажмите на сервер, чтобы включить или выключить его."
                    },
                    12.0,
                    MUTED,
                    false,
                );
                server_list(
                    &mut c,
                    &self.settings,
                    self.connected,
                    self.offset,
                    x,
                    217.0,
                    width,
                    h - 310.0,
                );
            }
            Page::Subscriptions => {
                c.card(x, 131.0, width, 118.0);
                c.text(x + 22.0, 150.0, "Источники серверов", 20.0, TEXT, true);
                c.text(
                    x + 22.0,
                    190.0,
                    "Ваши ссылки подписок и отдельные серверы.",
                    12.0,
                    MUTED,
                    false,
                );
                c.text(
                    x + 22.0,
                    218.0,
                    if self.subscription_changes_pending {
                        "Новый список серверов применится при следующем подключении."
                    } else {
                        "Автообновление при запуске и каждые 6 часов."
                    },
                    12.0,
                    MUTED,
                    false,
                );
                c.button(w - 159.0, 149.0, 125.0, "+ Добавить", Action::Import, true);
                c.button(
                    w - 292.0,
                    149.0,
                    125.0,
                    "📥 Принять QR",
                    Action::OpenLanSync,
                    false,
                );
                let manual = self
                    .settings
                    .nodes
                    .iter()
                    .filter(|n| n.manually_added)
                    .count();
                c.text(
                    x,
                    273.0,
                    &format!(
                        "Подписок: {} · Серверов вручную: {manual}",
                        self.settings.subscriptions.len()
                    ),
                    14.0,
                    TEXT,
                    true,
                );
                c.card(x, 309.0, width, h - 403.0);
                if self.settings.subscriptions.is_empty() {
                    c.text(
                        x + 22.0,
                        335.0,
                        "Добавьте HTTPS-ссылку подписки",
                        19.0,
                        TEXT,
                        true,
                    );
                    c.text(x + 22.0, 375.0, "У старых импортов ссылка не сохранялась. Добавьте её заново для обновления.", 12.0, MUTED, false);
                    c.text(
                        x + 22.0,
                        408.0,
                        "Существующие серверы останутся в библиотеке.",
                        12.0,
                        MUTED,
                        false,
                    );
                }
                for (position, source) in self
                    .settings
                    .subscriptions
                    .iter()
                    .enumerate()
                    .skip(self.offset)
                    .take(((h - 424.0) / 116.0).max(1.0) as usize)
                {
                    let y = 323.0 + (position - self.offset) as f64 * 116.0;
                    let count = self
                        .settings
                        .nodes
                        .iter()
                        .filter(|n| n.subscription_ids.contains(&source.id))
                        .count();
                    c.raw_text(x + 22.0, y, &shortened(&source.name, 60), 16.0, TEXT, true);
                    let age = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs()
                        .saturating_sub(source.updated_at);
                    let refreshed = if self.refreshing_subscription == Some(source.id) {
                        "обновляется…".into()
                    } else if self.subscription_errors.contains(&source.id) {
                        "ошибка обновления".into()
                    } else if source.updated_at == 0 {
                        "ещё не обновлялась".into()
                    } else if age < 60 {
                        "обновлено только что".into()
                    } else if age < 3600 {
                        format!("обновлено {} мин. назад", age / 60)
                    } else {
                        format!("обновлено {} ч. назад", age / 3600)
                    };
                    c.text(
                        x + 22.0,
                        y + 26.0,
                        &shortened(
                            &format!(
                                "{} · {count} серверов · {refreshed}",
                                multilink_core::subscription_host(&source.url),
                            ),
                            95,
                        ),
                        12.0,
                        MUTED,
                        false,
                    );
                    if self
                        .settings
                        .access
                        .as_ref()
                        .is_some_and(|a| a.subscription_id == source.id)
                        && self.connected
                    {
                        c.icon_button(
                            x + width - 70.0,
                            y,
                            18,
                            Action::Nav(Page::Access),
                            "Открыть мой доступ",
                        );
                    } else if self.refreshing_subscription != Some(source.id) {
                        c.icon_button(
                            x + width - 70.0,
                            y,
                            3,
                            Action::RefreshSubscription(source.id),
                            "Обновить подписку",
                        );
                    } else {
                        c.text(x + width - 65.0, y + 10.0, "…", 16.0, MUTED, true);
                    }
                    c.button(
                        x + 22.0,
                        y + 53.0,
                        110.0,
                        "Название",
                        Action::RenameSubscription(source.id),
                        false,
                    );
                    let managed = self
                        .settings
                        .access
                        .as_ref()
                        .is_some_and(|a| a.subscription_id == source.id);
                    if let Some(expiry) = source.expires_at {
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs();
                        let label = if expiry <= now {
                            "⚠️ Срок истёк".into()
                        } else {
                            let days = (expiry - now).div_ceil(86400);
                            let date_str = chrono::DateTime::from_timestamp(expiry as i64, 0)
                                .map(|dt| dt.format("%d.%m.%Y").to_string())
                                .unwrap_or_default();
                            if !date_str.is_empty() {
                                format!("⏳ {days} дн. (до {date_str})")
                            } else {
                                format!("⏳ Осталось: {days} дн.")
                            }
                        };
                        c.text(
                            x + width - 250.0,
                            y + 65.0,
                            &label,
                            12.0,
                            if expiry <= now { MUTED } else { ACCENT },
                            false,
                        );
                    }
                    let (label, action) = if managed {
                        ("Мой доступ", Action::Nav(Page::Access))
                    } else {
                        ("Изменить URL", Action::EditSubscriptionUrl(source.id))
                    };
                    c.button(x + 140.0, y + 53.0, 130.0, label, action, false);
                    c.button(
                        x + 278.0,
                        y + 53.0,
                        126.0,
                        "Поделиться",
                        Action::ShareSubscription(source.id),
                        false,
                    );
                    c.icon_button(
                        x + 412.0,
                        y + 53.0,
                        2,
                        Action::DeleteSubscription(source.id),
                        "Удалить",
                    );
                    c.line(x + 18.0, y + 104.0, width - 36.0);
                }
            }
            Page::About => {
                c.card(x, 131.0, width, 410.0);
                c.b.image(
                    LogicalRect::from_xywh(x + 28.0, 161.0, 72.0, 72.0),
                    64,
                    64,
                    self.logo.clone(),
                );
                c.text(x + 125.0, 166.0, "Everty MultiLink", 28.0, TEXT, true);
                c.text(
                    x + 125.0,
                    209.0,
                    concat!("Версия ", env!("CARGO_PKG_VERSION")),
                    13.0,
                    MUTED,
                    false,
                );
                c.line(x + 28.0, 261.0, width - 56.0);
                c.text(x + 28.0, 285.0, "АВТОР", 11.0, MUTED, true);
                c.text(x + 28.0, 310.0, "Arthur Valiev", 23.0, TEXT, true);
                c.text(
                    x + 28.0,
                    350.0,
                    "Проект, открытые разработки и экосистема Everty",
                    14.0,
                    MUTED,
                    false,
                );
                c.button(
                    x + 28.0,
                    388.0,
                    216.0,
                    "♥ Поддержать ↗",
                    Action::OpenDonate,
                    true,
                );
                c.button(
                    x + 252.0,
                    388.0,
                    150.0,
                    "Everty ↗",
                    Action::OpenAboutSite,
                    false,
                );
                c.button(
                    x + 410.0,
                    388.0,
                    120.0,
                    "GitHub ↗",
                    Action::OpenGitHub,
                    false,
                );
                c.button(
                    x + 538.0,
                    388.0,
                    150.0,
                    "✈ Telegram ↗",
                    Action::OpenTelegram,
                    false,
                );
                c.text(
                    x + 28.0,
                    455.0,
                    if cfg!(target_os = "macos") {
                        "Rust + EvertyUI · сетевое ядро Mihomo · macOS"
                    } else {
                        "Rust + EvertyUI · сетевое ядро Mihomo · Windows, Android & TV"
                    },
                    12.0,
                    MUTED,
                    false,
                );
            }
            Page::Routing => {
                let card_w = width - 26.0;
                c.card(x, 131.0, card_w, 110.0);
                c.text(x + 22.0, 150.0, "ПОЛИТИКА МАРШРУТИЗАЦИИ", 10.0, MUTED, true);
                c.text(
                    x + 22.0,
                    174.0,
                    self.settings.policy.label(),
                    22.0,
                    TEXT,
                    true,
                );
                c.text(
                    x + 22.0,
                    209.0,
                    policy_detail(self.settings.policy),
                    12.0,
                    MUTED,
                    false,
                );
                c.icon_button(
                    w - 100.0,
                    166.0,
                    14,
                    Action::Policy,
                    "Сменить алгоритм выбора сервера",
                );
                c.icon_button(
                    w - 192.0,
                    166.0,
                    7,
                    Action::ImportRoutes,
                    "Импортировать правила из файла",
                );
                c.icon_button(
                    w - 146.0,
                    166.0,
                    6,
                    Action::ExportRoutes,
                    "Экспортировать правила в файл",
                );
                c.button(
                    x,
                    264.0,
                    115.0,
                    "Правила",
                    Action::ShowAntigravityPatches(false),
                    !self.show_antigravity_patches,
                );
                c.button(
                    x + 123.0,
                    264.0,
                    105.0,
                    "Патчи",
                    Action::ShowAntigravityPatches(true),
                    self.show_antigravity_patches,
                );
                if !self.show_antigravity_patches {
                    c.button(w - 195.0, 264.0, 135.0, "+ Правило", Action::AddRule, true);
                    c.button(
                        w - 310.0,
                        264.0,
                        105.0,
                        "RU сайты",
                        Action::AddRuDomains,
                        false,
                    );
                    c.icon_button(
                        w - 356.0,
                        264.0,
                        12,
                        Action::AddProcessRule,
                        "Добавить правило для приложения",
                    );
                    c.button(
                        w - 495.0,
                        264.0,
                        130.0,
                        "Antigravity",
                        Action::AddAntigravity,
                        false,
                    );
                }
                c.card(x, 341.0, card_w, h - 435.0);
                if self.show_antigravity_patches {
                    c.text(x + 24.0, 362.0, "Antigravity — управление патчами", 18.0, TEXT, true);
                    c.text(
                        x + 24.0,
                        388.0,
                        "Обнаружение установленных компонентов и снятие региональных ограничений",
                        12.0,
                        MUTED,
                        false,
                    );
                    c.button(
                        w - 200.0,
                        360.0,
                        140.0,
                        if self.antigravity_patch_busy { "Проверка…" } else { "Проверить ↻" },
                        Action::ScanAntigravityPatches,
                        false,
                    );
                    let has_backup = self.antigravity_patches.as_ref().map_or(false, |r| r.has_backup);
                    if has_backup {
                        c.button(
                            w - 395.0,
                            360.0,
                            185.0,
                            "Восстановить оригинал",
                            Action::RestoreAntigravityPatches,
                            false,
                        );
                    }

                    // Restart Tip Box
                    let notice_y = 415.0;
                    c.rect(x + 20.0, notice_y, card_w - 40.0, 52.0, BG, 8.0);
                    c.b.stroke_rect(
                        LogicalRect::from_xywh(x + 20.0, notice_y, card_w - 40.0, 52.0),
                        WARNING.with_opacity(0.35),
                        1.0,
                        8.0,
                    );
                    c.text(x + 32.0, notice_y + 8.0, "💡 Подсказка: Перезапуск Antigravity при смене состояния VPN", 12.0, WARNING, true);
                    c.text(
                        x + 32.0,
                        notice_y + 24.0,
                        "После включения или отключения VPN приложение Antigravity необходимо перезапустить.",
                        11.0,
                        TEXT,
                        false,
                    );
                    c.text(
                        x + 32.0,
                        notice_y + 37.0,
                        "Если в чате «Error Agent execution terminated due to error» — запрос пошёл мимо VPN. Перезапустите Antigravity!",
                        10.0,
                        MUTED,
                        false,
                    );

                    let comp_rows = [
                        (
                            multilink_core::antigravity_patch::ComponentKind::Desktop,
                            "Antigravity Desktop",
                            "language_server.exe",
                        ),
                        (
                            multilink_core::antigravity_patch::ComponentKind::Ide,
                            "Antigravity IDE",
                            "agy.exe / main.js / extension.js / language_server_windows_x64.exe",
                        ),
                        (
                            multilink_core::antigravity_patch::ComponentKind::Cli,
                            "Antigravity CLI",
                            "agy.exe (консольный агент)",
                        ),
                    ];

                    let mut cy = 480.0;
                    for (comp_type, comp_name, comp_files) in comp_rows {
                        let comp_info = self
                            .antigravity_patches
                            .as_ref()
                            .and_then(|r| r.components.iter().find(|c| c.kind == comp_type));
                        let state = comp_info.map(|c| c.state).unwrap_or(multilink_core::antigravity_patch::PatchState::Unknown);

                        let (state_label, state_color, can_patch) = match state {
                            multilink_core::antigravity_patch::PatchState::Patched => {
                                ("✓ Патч обнаружен", ACCENT, false)
                            }
                            multilink_core::antigravity_patch::PatchState::NotPatched => {
                                ("⚠ Патч отсутствует", WARNING, true)
                            }
                            multilink_core::antigravity_patch::PatchState::Partial
                            | multilink_core::antigravity_patch::PatchState::Error => {
                                ("! Частичная модификация", DANGER, true)
                            }
                            multilink_core::antigravity_patch::PatchState::Unknown
                            | multilink_core::antigravity_patch::PatchState::NotInstalled => {
                                ("○ Не установлен", MUTED, false)
                            }
                        };

                        c.text(x + 28.0, cy + 5.0, "●", 14.0, state_color, false);
                        c.text(x + 48.0, cy + 4.0, comp_name, 15.0, TEXT, true);
                        c.text(x + 48.0, cy + 24.0, comp_files, 11.0, MUTED, false);
                        c.text(w - 370.0, cy + 14.0, state_label, 13.0, state_color, true);

                        if can_patch {
                            let is_checked = self.antigravity_selected_patches.contains(&comp_type);
                            c.button(
                                w - 200.0,
                                cy + 5.0,
                                140.0,
                                if is_checked { "☑ Выбрано" } else { "☐ Применить" },
                                Action::TogglePatchComponent(comp_type),
                                is_checked,
                            );
                        }
                        cy += 55.0;
                        c.line(x + 24.0, cy, card_w - 48.0);
                        cy += 10.0;
                    }

                    let selected_count = self.antigravity_selected_patches.len();
                    c.text(x + 28.0, cy + 18.0, &format!("Выбрано компонентов: {selected_count}"), 13.0, TEXT, true);
                    if let Some(status) = &self.antigravity_patch_status {
                        c.text(x + 220.0, cy + 18.0, status, 12.0, ACCENT, false);
                    }
                    c.button(
                        w - 245.0,
                        cy + 8.0,
                        185.0,
                        "Применить выбранное",
                        Action::ApplyAntigravityPatches,
                        selected_count > 0 && !self.antigravity_patch_busy,
                    );
                    c.text(
                        x + 28.0,
                        cy + 45.0,
                        "Безопасность: Резервная копия создаётся автоматически. Системные переменные https_proxy не изменяются.",
                        11.0,
                        MUTED,
                        false,
                    );
                } else if self.settings.rules.is_empty() {
                    c.text(x + 26.0, 370.0, "Пока без исключений", 20.0, TEXT, true);
                    c.text(
                        x + 26.0,
                        407.0,
                        "Добавьте example.com, 192.168.1.1 или 10.0.0.0/8.",
                        14.0,
                        MUTED,
                        false,
                    );
                    c.text(
                        x + 26.0,
                        438.0,
                        "Добавлять и удалять исключения можно без отключения VPN.",
                        13.0,
                        MUTED,
                        false,
                    );
                } else {
                    for (row, (i, rule)) in route_rows
                        .into_iter()
                        .enumerate()
                        .skip(self.offset)
                        .take(((h - 458.0) / 58.0).max(1.0) as usize)
                    {
                        let y = 355.0 + (row - self.offset) as f64 * 58.0;
                        let parts: Vec<_> = rule.split(',').collect();
                        let key = multilink_core::rule_key(rule);
                        let members = self
                            .settings
                            .rule_groups
                            .iter()
                            .find(|group| group.first() == Some(&key))
                            .map_or(1, Vec::len);
                        let value = parts.get(1).copied().unwrap_or_default();
                        let label = if members > 1 {
                            format!("{value}  +{}", members - 1)
                        } else {
                            value.to_owned()
                        };
                        c.b.push_clip(LogicalRect::from_xywh(
                            x + 20.0,
                            y,
                            (card_w - 330.0).max(1.0),
                            54.0,
                        ));
                        c.text(
                            x + 20.0,
                            y + 9.0,
                            if parts.get(1) == Some(&"RU-SITES") {
                                "Российские сайты"
                            } else {
                                &label
                            },
                            15.0,
                            TEXT,
                            true,
                        );
                        let target = parts.get(2).copied().unwrap_or("DIRECT");
                        if let Some(note) = self
                            .settings
                            .rule_notes
                            .get(&multilink_core::rule_key(rule))
                        {
                            c.raw_text(
                                x + 20.0,
                                y + 32.0,
                                &shortened(&note.replace(['\n', '\r', '\t'], " "), 43),
                                10.0,
                                MUTED,
                                false,
                            );
                        }
                        let _ = c.b.pop_clip();
                        let server_name = multilink_core::server_route_name(target);
                        let label = if target == "DIRECT" {
                            "Напрямую"
                        } else if target == "MULTILINK" {
                            "Через VPN"
                        } else {
                            server_name.as_deref().unwrap_or_else(|| {
                                target.strip_prefix("COUNTRY-").unwrap_or(target)
                            })
                        };
                        c.text(
                            w - 310.0,
                            y + 12.0,
                            &shortened(label, 16),
                            11.0,
                            ACCENT,
                            true,
                        );
                        c.icon_button(w - 150.0, y, 1, Action::EditRule(i), "Изменить правило");
                        c.icon_button(w - 104.0, y, 2, Action::DeleteRule(i), "Удалить правило");
                    }
                }
            }
            Page::Statistics => {
                c.card(x, 131.0, width, 244.0);
                c.text(
                    x + 22.0,
                    151.0,
                    "Трафик с начала подключения",
                    18.0,
                    TEXT,
                    true,
                );
                c.text(x + 22.0, 185.0, "↓ Загрузка", 12.0, ACCENT, true);
                c.text(x + 135.0, 185.0, "↑ Отдача", 12.0, MUTED, false);
                for i in 0..4 {
                    c.line(x + 22.0, 230.0 + i as f64 * 39.0, width - 44.0);
                }
                if self.history.is_empty() {
                    c.text(
                        x + width / 2.0 - 119.0,
                        267.0,
                        "Ожидание подключения",
                        16.0,
                        MUTED,
                        false,
                    );
                } else {
                    let max = self
                        .history
                        .iter()
                        .map(|(d, u)| d.max(*u))
                        .fold(1024.0, f64::max);
                    let bar = (width - 44.0) / 90.0;
                    for (i, (d, u)) in self.history.iter().enumerate() {
                        let px = x + 22.0 + i as f64 * bar;
                        let dh = (d / max * 130.0).max(1.0);
                        let uh = (u / max * 130.0).max(1.0);
                        c.rect(px, 358.0 - dh, bar * 0.48, dh, ACCENT, 1.0);
                        c.rect(px + bar * 0.5, 358.0 - uh, bar * 0.35, uh, MUTED, 1.0);
                    }
                }
                let values = [
                    ("ПОЛУЧЕНО", bytes(self.metrics.download as f64)),
                    ("ОТПРАВЛЕНО", bytes(self.metrics.upload as f64)),
                    ("СОЕДИНЕНИЯ", self.metrics.connections.to_string()),
                ];
                let cw = (width - 28.0) / 3.0;
                for (i, (label, value)) in values.iter().enumerate() {
                    let cx = x + i as f64 * (cw + 14.0);
                    c.card(cx, 395.0, cw, 108.0);
                    c.text(cx + 20.0, 413.0, label, 11.0, MUTED, true);
                    c.text(cx + 20.0, 442.0, value, 28.0, TEXT, true);
                }
                c.text(x, 527.0, "Журнал событий", 18.0, TEXT, true);
                for (i, log) in self
                    .logs
                    .iter()
                    .take(((h - 620.0) / 30.0).max(1.0) as usize)
                    .enumerate()
                {
                    c.text(
                        x,
                        566.0 + i as f64 * 30.0,
                        &shortened(i18n::tr(language, log).as_ref(), 100),
                        12.0,
                        MUTED,
                        false,
                    );
                }
            }
            Page::Settings => {
                let page_start_hits = c.hits.len();
                let page_start_disabled = c.disabled_hits.len();
                let card_w = width - 26.0;
                let scroll_y = self.offset as f64 * 35.0;
                let clip_top = 115.0;
                let clip_bottom = (h - 75.0).max(clip_top + 10.0);
                c.b.push_clip(LogicalRect::from_xywh(
                    0.0,
                    clip_top,
                    w,
                    clip_bottom - clip_top,
                ));

                // Card 1: Network Connection Modes
                let y1 = 125.0 - scroll_y;
                c.card(x, y1, card_w, 280.0);
                c.text(x + 24.0, y1 + 15.0, "Подключение", 17.0, TEXT, true);
                c.text(
                    x + 24.0,
                    y1 + 44.0,
                    if cfg!(target_os = "macos") {
                        "Системный прокси"
                    } else {
                        "Системный прокси"
                    },
                    14.0,
                    TEXT,
                    true,
                );
                c.text(
                    x + 24.0,
                    y1 + 65.0,
                    if cfg!(target_os = "macos") {
                        "Автоматически настраивать прокси браузеров и приложений."
                    } else {
                        "Для браузеров и приложений с поддержкой прокси."
                    },
                    11.0,
                    MUTED,
                    false,
                );
                c.toggle(
                    x + card_w - 68.0,
                    y1 + 41.0,
                    self.settings.system_proxy,
                    Action::ToggleSystemProxy,
                );
                c.line(x + 24.0, y1 + 88.0, card_w - 48.0);
                c.text(x + 24.0, y1 + 100.0, "TUN — весь трафик", 14.0, TEXT, true);
                c.text(
                    x + 24.0,
                    y1 + 121.0,
                    if cfg!(target_os = "macos") {
                        "Весь трафик Mac, включая DNS. macOS запросит доступ при подключении."
                    } else {
                        "Перехват сетевого стека (требуются права администратора)."
                    },
                    11.0,
                    MUTED,
                    false,
                );
                c.toggle(
                    x + card_w - 68.0,
                    y1 + 97.0,
                    self.settings.tun,
                    Action::ToggleTun,
                );
                c.line(x + 24.0, y1 + 144.0, card_w - 48.0);
                c.text(
                    x + 24.0,
                    y1 + 156.0,
                    "Режим «Только правила»",
                    14.0,
                    TEXT,
                    true,
                );
                c.text(
                    x + 24.0,
                    y1 + 177.0,
                    "Обычный трафик напрямую, VPN — только для сайтов/игр из правил.",
                    11.0,
                    MUTED,
                    false,
                );
                c.toggle(
                    x + card_w - 68.0,
                    y1 + 153.0,
                    self.settings.rules_only,
                    Action::ToggleRulesOnly,
                );
                c.line(x + 24.0, y1 + 200.0, card_w - 48.0);
                c.text(
                    x + 24.0,
                    y1 + 212.0,
                    "Изолировать локальный прокси",
                    14.0,
                    TEXT,
                    true,
                );
                c.text(
                    x + 24.0,
                    y1 + 233.0,
                    "Ограничить доступ сторонних приложений к SOCKS/HTTP и API.",
                    11.0,
                    MUTED,
                    false,
                );
                let (iso_active, iso_desc) =
                    multilink_mihomo::proxy_isolation_status(&self.settings, self.connected);
                c.text(
                    x + 24.0,
                    y1 + 254.0,
                    iso_desc,
                    10.5,
                    if iso_active {
                        ACCENT
                    } else if self.settings.isolate_local_proxy {
                        Color::rgba(241, 194, 92, 255)
                    } else {
                        MUTED
                    },
                    false,
                );
                c.button(
                    x + card_w - 188.0,
                    y1 + 216.0,
                    108.0,
                    "Подробнее",
                    Action::ShowProxyIsolationDetails,
                    false,
                );
                c.toggle(
                    x + card_w - 68.0,
                    y1 + 219.0,
                    self.settings.isolate_local_proxy,
                    Action::ToggleIsolateLocalProxy,
                );

                // Card 2: Autostart & Autoconnect
                let y2 = 417.0 - scroll_y;
                c.card(x, y2, card_w, 148.0);
                c.text(x + 24.0, y2 + 15.0, "Запуск программы", 16.0, TEXT, true);
                c.text(
                    x + 24.0,
                    y2 + 44.0,
                    if cfg!(target_os = "macos") {
                        "Запускать при входе в macOS"
                    } else {
                        "Запускать вместе с Windows"
                    },
                    14.0,
                    TEXT,
                    true,
                );
                c.text(
                    x + 24.0,
                    y2 + 65.0,
                    if cfg!(target_os = "macos") {
                        "Работать в фоне со значком в строке меню."
                    } else {
                        "Приложение запускается в фоновом режиме в системном трее."
                    },
                    11.0,
                    MUTED,
                    false,
                );
                c.toggle(
                    x + card_w - 68.0,
                    y2 + 41.0,
                    self.autostart,
                    Action::ToggleAutostart,
                );
                c.line(x + 24.0, y2 + 88.0, card_w - 48.0);
                c.text(
                    x + 24.0,
                    y2 + 100.0,
                    "Автоподключение при старте",
                    14.0,
                    TEXT,
                    true,
                );
                c.text(
                    x + 24.0,
                    y2 + 121.0,
                    "Автоматически включать VPN-соединение при запуске программы.",
                    11.0,
                    MUTED,
                    false,
                );
                c.toggle(
                    x + card_w - 68.0,
                    y2 + 97.0,
                    self.settings.autoconnect,
                    Action::ToggleAutoconnect,
                );

                // Card 3: PIN Protection
                let y3 = 577.0 - scroll_y;
                c.card(x, y3, card_w, 70.0);
                let pin_active = self.settings.pin_code.is_some();
                c.text(
                    x + 24.0,
                    y3 + 15.0,
                    "Защита настроек PIN-кодом",
                    14.0,
                    TEXT,
                    true,
                );
                c.text(
                    x + 24.0,
                    y3 + 39.0,
                    if !pin_active {
                        "PIN не задан · изменение параметров доступно всем"
                    } else if self.pin_unlocked {
                        "Разблокировано · изменения параметров временно разрешены"
                    } else {
                        "Заблокировано · для любых изменений требуется ввести PIN"
                    },
                    11.0,
                    if pin_active { ACCENT } else { MUTED },
                    false,
                );
                if !pin_active {
                    c.button(
                        x + card_w - 144.0,
                        y3 + 16.0,
                        130.0,
                        "Задать PIN",
                        Action::OpenSetPin,
                        false,
                    );
                } else {
                    c.button(
                        x + card_w - 264.0,
                        y3 + 16.0,
                        82.0,
                        "Сменить",
                        Action::OpenSetPin,
                        false,
                    );
                    c.button(
                        x + card_w - 176.0,
                        y3 + 16.0,
                        72.0,
                        "Снять",
                        Action::RemovePin,
                        false,
                    );
                    c.button(
                        x + card_w - 98.0,
                        y3 + 16.0,
                        84.0,
                        if self.pin_unlocked {
                            "Заблокир."
                        } else {
                            "Открыть"
                        },
                        if self.pin_unlocked {
                            Action::LockPin
                        } else {
                            Action::OpenSetPin
                        },
                        false,
                    );
                }

                // Card 4: Server Selection Policy
                let y4 = 659.0 - scroll_y;
                c.card(x, y4, card_w, 70.0);
                c.text(
                    x + 24.0,
                    y4 + 15.0,
                    &format!("Выбор сервера: {}", self.settings.policy.label()),
                    14.0,
                    TEXT,
                    true,
                );
                c.text(
                    x + 24.0,
                    y4 + 39.0,
                    policy_detail(self.settings.policy),
                    11.0,
                    MUTED,
                    false,
                );
                c.icon_button(
                    x + card_w - 52.0,
                    y4 + 16.0,
                    14,
                    Action::Policy,
                    "Сменить алгоритм выбора сервера",
                );

                // Card 5: App Updates (GitHub Releases)
                let y5 = 741.0 - scroll_y;
                c.card(x, y5, card_w, 70.0);
                c.text(
                    x + 24.0,
                    y5 + 15.0,
                    &format!("Версия приложения: v{}", env!("CARGO_PKG_VERSION")),
                    14.0,
                    TEXT,
                    true,
                );
                let update_sub = if self.checking_update {
                    "Проверка обновлений на GitHub…".to_string()
                } else if let Some(ver) = &self.update_available {
                    format!("Доступна новая версия {ver} на GitHub!")
                } else {
                    "У вас установлена последняя официальная версия.".to_string()
                };
                c.text(
                    x + 24.0,
                    y5 + 39.0,
                    &update_sub,
                    11.0,
                    if self.update_available.is_some() {
                        ACCENT
                    } else {
                        MUTED
                    },
                    false,
                );
                if let Some(ver) = &self.update_available {
                    c.button(
                        x + card_w - 150.0,
                        y5 + 16.0,
                        136.0,
                        &format!("Скачать {ver}"),
                        Action::OpenUpdateLink,
                        true,
                    );
                } else {
                    c.button(
                        x + card_w - 130.0,
                        y5 + 16.0,
                        116.0,
                        if self.checking_update {
                            "Проверка…"
                        } else {
                            "Проверить"
                        },
                        Action::CheckUpdate,
                        false,
                    );
                }

                // Card 6: Language
                let y6 = 823.0 - scroll_y;
                c.card(x, y6, card_w, 64.0);
                c.text(x + 24.0, y6 + 22.0, "Язык интерфейса:", 14.0, TEXT, true);
                c.text(
                    x + 162.0,
                    y6 + 22.0,
                    i18n::label(&self.settings.language),
                    14.0,
                    ACCENT,
                    true,
                );
                c.icon_button(
                    x + card_w - 52.0,
                    y6 + 13.0,
                    14,
                    Action::Language,
                    "Изменить язык",
                );

                // Card 7: Backup & Restore
                let y7 = 899.0 - scroll_y;
                c.card(x, y7, card_w, 70.0);
                c.text(
                    x + 24.0,
                    y7 + 15.0,
                    "Резервная копия настроек",
                    14.0,
                    TEXT,
                    true,
                );
                c.text(
                    x + 24.0,
                    y7 + 39.0,
                    "Экспорт всех параметров, правил и PIN в файл или импорт из него.",
                    11.0,
                    MUTED,
                    false,
                );
                c.button(
                    x + card_w - 216.0,
                    y7 + 16.0,
                    98.0,
                    "Экспорт",
                    Action::ExportSettings,
                    false,
                );
                c.button(
                    x + card_w - 108.0,
                    y7 + 16.0,
                    98.0,
                    "Импорт",
                    Action::ImportSettings,
                    false,
                );

                // Bottom notes & actions
                let gateway_y = 983.0 - scroll_y;
                c.card(x, gateway_y, card_w, 90.0);
                c.text(
                    x + 24.0,
                    gateway_y + 15.0,
                    "LAN-хост Everty",
                    16.0,
                    TEXT,
                    true,
                );
                c.text(
                    x + 24.0,
                    gateway_y + 46.0,
                    "Устройства с ключом используют правила этого клиента.",
                    11.0,
                    MUTED,
                    false,
                );
                c.toggle(
                    x + card_w - 68.0,
                    gateway_y + 14.0,
                    self.settings.lan_gateway.is_some(),
                    Action::ToggleLanGateway,
                );
                if self.settings.lan_gateway.is_some() {
                    c.button(
                        x + card_w - 226.0,
                        gateway_y + 14.0,
                        140.0,
                        "QR и ссылка",
                        Action::ShareLanGateway,
                        false,
                    );
                    c.button(
                        x + card_w - 226.0,
                        gateway_y + 53.0,
                        140.0,
                        "Копировать ключ",
                        Action::CopyLanGatewayKey,
                        false,
                    );
                }
                let y8 = 1088.0 - scroll_y;
                c.text(
                    x,
                    y8 + 8.0,
                    if self.connected {
                        "Перед изменением сетевого режима отключитесь от VPN."
                    } else if cfg!(target_os = "macos") {
                        "TUN защищает весь Mac. Системный прокси можно использовать отдельно."
                    } else {
                        "Режимы TUN и прокси можно включать вместе."
                    },
                    12.0,
                    MUTED,
                    false,
                );
                c.icon_button(
                    x + card_w - 42.0,
                    y8,
                    2,
                    Action::ClearServers,
                    "Очистить библиотеку серверов",
                );

                let _ = c.b.pop_clip();
                let mut i = page_start_hits;
                while i < c.hits.len() {
                    let (rect, _) = &c.hits[i];
                    if rect.bottom() <= clip_top || rect.top() >= clip_bottom {
                        c.hits.remove(i);
                    } else {
                        i += 1;
                    }
                }
                let mut j = page_start_disabled;
                while j < c.disabled_hits.len() {
                    let (rect, _) = &c.disabled_hits[j];
                    if rect.bottom() <= clip_top || rect.top() >= clip_bottom {
                        c.disabled_hits.remove(j);
                    } else {
                        j += 1;
                    }
                }
            }
        }
        c.line(x, h - 70.0, width);
        c.rect(
            x,
            h - 46.0,
            6.0,
            6.0,
            if self.connected { ACCENT } else { MUTED },
            3.0,
        );
        c.text(
            x + 17.0,
            h - 54.0,
            &shortened(i18n::tr(language, &self.notice).as_ref(), 105),
            11.0,
            MUTED,
            false,
        );
        if let Some(bar) = self.scroll_bar {
            c.b.fill_rect(bar.track, LINE, 4.0);
            c.b.fill_rect(
                bar.thumb,
                if self.scroll_drag.is_some()
                    || bar.track.inset(Insets::all(-5.0)).contains(self.pointer)
                {
                    ACCENT
                } else {
                    MUTED.with_opacity(0.65)
                },
                4.0,
            );
        }
        if self.modal.is_none() {
            if c.tooltip.is_none() {
                if let Some((rect, hint)) = c
                    .disabled_hits
                    .iter()
                    .rev()
                    .find(|(r, _)| r.contains(self.pointer))
                {
                    c.tooltip = Some((*rect, hint.clone()));
                }
            }
            c.paint_tooltip(w, h);
        }
        if let Some(ref modal) = self.modal {
            c.hits.clear();
            c.b.set_layer(PaintLayer::Overlay);
            c.rect(0.0, 0.0, w, h, Color::rgba(0, 0, 0, 180), 0.0);
            let mx = (w - 660.0) / 2.0;
            let modal_height = match modal {
                Modal::Rule | Modal::ProcessRule | Modal::RuRule => 360.0,
                Modal::ShareQr { .. } | Modal::LanSync { .. } => 420.0,
                Modal::ProxyIsolationDetails => 360.0,
                _ => 320.0,
            };
            let my = (h - modal_height) / 2.0;
            c.card(mx, my, 660.0, modal_height);
            if matches!(modal, Modal::ProxyIsolationDetails) {
                c.text(
                    mx + 28.0,
                    my + 24.0,
                    "Изоляция локального прокси",
                    21.0,
                    TEXT,
                    true,
                );
                c.text(
                    mx + 28.0,
                    my + 54.0,
                    "Защита от утечек и обхода ограничений Split Tunneling",
                    12.0,
                    MUTED,
                    false,
                );
                c.text(
                    mx + 28.0,
                    my + 86.0,
                    "1. Зачем это нужно?",
                    14.0,
                    TEXT,
                    true,
                );
                c.text(
                    mx + 28.0,
                    my + 108.0,
                    "Приложения, исключённые из VPN (DIRECT), могут сканировать порт 127.0.0.1:17890\nили API управления, чтобы обойти маршрутизацию и узнать внешний IP VPN-сервера.",
                    12.0,
                    MUTED,
                    false,
                );
                c.text(
                    mx + 28.0,
                    my + 152.0,
                    "2. Как работает изоляция?",
                    14.0,
                    TEXT,
                    true,
                );
                c.text(
                    mx + 28.0,
                    my + 174.0,
                    "В режиме TUN отдельный порт 17890 полностью отключается (нет слушателя на сокете),\nа API управления защищается криптографическим токеном.",
                    12.0,
                    MUTED,
                    false,
                );
                c.text(
                    mx + 28.0,
                    my + 218.0,
                    "3. Текущий статус защиты:",
                    14.0,
                    TEXT,
                    true,
                );
                let (iso_active, iso_desc) =
                    multilink_mihomo::proxy_isolation_status(&self.settings, self.connected);
                c.text(
                    mx + 28.0,
                    my + 242.0,
                    iso_desc,
                    13.0,
                    if iso_active {
                        ACCENT
                    } else if self.settings.isolate_local_proxy {
                        Color::rgba(241, 194, 92, 255)
                    } else {
                        MUTED
                    },
                    true,
                );
                c.button(
                    mx + 483.0,
                    my + 304.0,
                    149.0,
                    "Понятно",
                    Action::Close,
                    true,
                );
                self.input_rect = None;
                self.hits = c.hits;
                return c.b.finish();
            }
            if let Modal::ShareQr {
                title,
                content,
                caption,
            } = modal
            {
                c.text(mx + 28.0, my + 24.0, title, 21.0, TEXT, true);
                c.text(mx + 28.0, my + 54.0, caption, 12.0, MUTED, false);
                draw_qr(&mut c, mx + 28.0, my + 84.0, 240.0, content);
                c.text(
                    mx + 290.0,
                    my + 94.0,
                    "ССЫЛКА ДЛЯ ПОДКЛЮЧЕНИЯ:",
                    11.0,
                    MUTED,
                    true,
                );
                c.card(mx + 290.0, my + 115.0, 342.0, 110.0);
                c.b.push_clip(LogicalRect::from_xywh(mx + 300.0, my + 125.0, 322.0, 90.0));
                c.raw_text(
                    mx + 300.0,
                    my + 125.0,
                    &shortened(content, 180),
                    12.0,
                    TEXT,
                    false,
                );
                let _ = c.b.pop_clip();
                c.button(
                    mx + 290.0,
                    my + 245.0,
                    342.0,
                    "📋 Скопировать ссылку",
                    Action::CopyQrContent,
                    true,
                );
                c.button(
                    mx + 483.0,
                    my + 360.0,
                    149.0,
                    "Закрыть",
                    Action::Close,
                    false,
                );
                self.input_rect = None;
                self.hits = c.hits;
                return c.b.finish();
            }
            if let Modal::LanSync { url, pin } = modal {
                c.text(
                    mx + 28.0,
                    my + 24.0,
                    "Принять по QR / Wi-Fi",
                    21.0,
                    TEXT,
                    true,
                );
                c.text(
                    mx + 28.0,
                    my + 54.0,
                    "Отсканируйте камерой смартфона или перейдите в браузере:",
                    12.0,
                    MUTED,
                    false,
                );
                draw_qr(&mut c, mx + 28.0, my + 84.0, 240.0, url);
                c.text(
                    mx + 290.0,
                    my + 94.0,
                    "КОД ПОДТВЕРЖДЕНИЯ (PIN):",
                    11.0,
                    MUTED,
                    true,
                );
                c.rect(mx + 290.0, my + 114.0, 150.0, 42.0, SOFT, 8.0);
                c.text(mx + 316.0, my + 122.0, pin, 24.0, ACCENT, true);
                c.text(
                    mx + 290.0,
                    my + 175.0,
                    "АДРЕС ВЕБ-ИНТЕРФЕЙСА:",
                    11.0,
                    MUTED,
                    true,
                );
                c.text(mx + 290.0, my + 195.0, url, 13.0, TEXT, false);
                c.button(
                    mx + 290.0,
                    my + 230.0,
                    240.0,
                    "Скопировать адрес",
                    Action::CopyQrContent,
                    false,
                );
                c.text(
                    mx + 290.0,
                    my + 295.0,
                    "Ожидание отправки со смартфона или браузера…",
                    12.0,
                    MUTED,
                    false,
                );
                c.button(
                    mx + 483.0,
                    my + 360.0,
                    149.0,
                    "Закрыть",
                    Action::Close,
                    true,
                );
                self.input_rect = None;
                self.hits = c.hits;
                return c.b.finish();
            }
            if let Modal::ServerDiagnostics(node_idx) = modal {
                let modal_w = 720.0;
                let modal_h = 560.0;
                let sx = (w - modal_w) / 2.0;
                let sy = (h - modal_h) / 2.0;
                c.card(sx, sy, modal_w, modal_h);
                let node_name = self.settings.nodes.get(*node_idx).map(|n| n.name.as_str()).unwrap_or("Сервер");
                c.text(sx + 24.0, sy + 18.0, &format!("Диагностика сервера: {}", shortened(node_name, 35)), 19.0, TEXT, true);
                c.text(sx + 24.0, sy + 44.0, "Глубокая проверка TLS, задержек и доступности ключевых сервисов", 12.0, MUTED, false);

                if let Some(node) = self.settings.nodes.get(*node_idx) {
                    let host = node.proxy.get("server").and_then(|v| v.as_str()).unwrap_or("—");
                    let port = node.proxy.get("port").and_then(|v| v.as_u64()).map(|p| p.to_string()).unwrap_or_else(|| "—".into());
                    let ping_str = node.delay.map(|d| format!("{d} ms")).unwrap_or_else(|| "—".into());
                    let jitter_str = format!("{:.1} ms", node.quality.jitter_ms);
                    let rel_str = format!("{:.0}%", node.quality.reliability * 100.0);
                    let samples_str = format!("{}", node.quality.samples);

                    c.rect(sx + 20.0, sy + 66.0, modal_w - 40.0, 48.0, BG, 8.0);
                    c.b.stroke_rect(LogicalRect::from_xywh(sx + 20.0, sy + 66.0, modal_w - 40.0, 48.0), LINE, 1.0, 8.0);

                    c.text(sx + 32.0, sy + 74.0, &format!("Узел: {host}:{port}"), 11.0, MUTED, false);
                    c.text(sx + 32.0, sy + 90.0, &format!("Пинг: {ping_str} · Jitter: {jitter_str}"), 12.0, ACCENT, true);

                    c.text(sx + 250.0, sy + 74.0, "Стабильность канала", 11.0, MUTED, false);
                    c.text(sx + 250.0, sy + 90.0, &format!("{rel_str} ({samples_str} проверок)"), 12.0, TEXT, true);

                    c.text(sx + 440.0, sy + 74.0, "История доступности", 11.0, MUTED, false);
                    c.draw_availability_bars(sx + 440.0, sy + 88.0, node, true);

                    c.button(
                        sx + modal_w - 130.0,
                        sy + 73.0,
                        100.0,
                        "⚡ Пинг",
                        Action::PingServer(*node_idx),
                        false,
                    );
                }

                // Presets row
                let current_preset = self.diagnostics_preset;
                let presets = [
                    (multilink_mihomo::server_diagnostics::EndpointPreset::All, "Все (8)", 85.0),
                    (multilink_mihomo::server_diagnostics::EndpointPreset::Antigravity, "Antigravity (5)", 130.0),
                    (multilink_mihomo::server_diagnostics::EndpointPreset::GoogleApi, "Google API (6)", 130.0),
                    (multilink_mihomo::server_diagnostics::EndpointPreset::General, "Общий Web (2)", 125.0),
                ];
                let mut bx = sx + 20.0;
                for (preset, label, btn_w) in presets {
                    let is_active = current_preset == preset;
                    c.button(bx, sy + 122.0, btn_w, label, Action::SetDiagnosticsPreset(preset), is_active);
                    bx += btn_w + 10.0;
                }

                // Table Header
                let ty = sy + 162.0;
                c.card(sx + 20.0, ty, modal_w - 40.0, 330.0);
                c.text(sx + 35.0, ty + 12.0, "СЕРВИС / ЭНДПОИНТ", 10.0, MUTED, true);
                c.text(sx + 300.0, ty + 12.0, "МЕТОД", 10.0, MUTED, true);
                c.text(sx + 370.0, ty + 12.0, "HTTP", 10.0, MUTED, true);
                c.text(sx + 440.0, ty + 12.0, "RTT", 10.0, MUTED, true);
                c.text(sx + 530.0, ty + 12.0, "СТАТУС", 10.0, MUTED, true);
                c.line(sx + 25.0, ty + 30.0, modal_w - 50.0);

                if self.diagnostics_checking {
                    c.text(sx + 220.0, ty + 140.0, "Проверка узла (TCP / TLS Handshake)…", 14.0, ACCENT, true);
                } else if let Some(report) = &self.server_diagnostics {
                    for (idx, ep) in report.endpoints.iter().take(6).enumerate() {
                        let ey = ty + 36.0 + idx as f64 * 46.0;
                        c.text(sx + 35.0, ey + 4.0, ep.name.as_str(), 13.0, TEXT, true);
                        c.text(sx + 35.0, ey + 22.0, &shortened(&ep.url, 38), 10.0, MUTED, false);
                        c.text(sx + 300.0, ey + 12.0, ep.method.as_str(), 11.0, MUTED, true);
                        let code_str = ep.status_code.map(|c| c.to_string()).unwrap_or_else(|| "—".into());
                        c.text(sx + 370.0, ey + 12.0, &code_str, 12.0, TEXT, true);
                        c.text(sx + 440.0, ey + 12.0, &format!("{} ms", ep.latency_ms), 12.0, ACCENT, false);

                        let (badge_text, badge_color) = match ep.status {
                            multilink_mihomo::server_diagnostics::EndpointStatus::Ok => ("● Доступен", ACCENT),
                            multilink_mihomo::server_diagnostics::EndpointStatus::Limited => ("▲ Ограничен", WARNING),
                            multilink_mihomo::server_diagnostics::EndpointStatus::Failed => ("✕ Ошибка", DANGER),
                        };
                        c.text(sx + 530.0, ey + 12.0, badge_text, 12.0, badge_color, true);
                    }
                } else {
                    c.text(sx + 240.0, ty + 140.0, "Нажмите «Повторить» для запуска", 13.0, MUTED, false);
                }

                // Bottom summary and buttons
                let bottom_y = sy + modal_h - 52.0;
                if let Some(rep) = &self.server_diagnostics {
                    let exit_text = if let Some(ip) = &rep.exit_ip {
                        format!("Внешний IP: {ip} · {}", shortened(&rep.summary, 45))
                    } else {
                        shortened(&rep.summary, 60).to_string()
                    };
                    c.text(sx + 24.0, bottom_y + 12.0, &exit_text, 11.0, MUTED, false);
                }
                c.button(sx + modal_w - 290.0, bottom_y, 140.0, "Повторить ↻", Action::RunServerDiagnostics(*node_idx), false);
                c.button(sx + modal_w - 140.0, bottom_y, 116.0, "Закрыть", Action::CloseServerDiagnostics, true);

                self.input_rect = None;
                self.hits = c.hits;
                return c.b.finish();
            }
            if *modal == Modal::AntigravityServersCheck {
                let modal_w = 680.0;
                let modal_h = 530.0;
                let sx = (w - modal_w) / 2.0;
                let sy = (h - modal_h) / 2.0;

                c.rect(sx, sy, modal_w, modal_h, PANEL, 12.0);
                c.b.stroke_rect(LogicalRect::from_xywh(sx, sy, modal_w, modal_h), LINE, 1.0, 12.0);

                c.text(sx + 24.0, sy + 20.0, "Проверка серверов для Antigravity", 21.0, TEXT, true);
                c.text(
                    sx + 24.0,
                    sy + 46.0,
                    "Доступность Google API (HTTP 400/404) и региональный статус узлов",
                    12.0,
                    MUTED,
                    false,
                );

                // Restart Tip Box
                let tip_y = sy + 72.0;
                c.rect(sx + 20.0, tip_y, modal_w - 40.0, 56.0, BG, 8.0);
                c.b.stroke_rect(
                    LogicalRect::from_xywh(sx + 20.0, tip_y, modal_w - 40.0, 56.0),
                    WARNING.with_opacity(0.35),
                    1.0,
                    8.0,
                );
                c.text(sx + 32.0, tip_y + 8.0, "💡 Важно: Перезапуск Antigravity при смене состояния VPN", 12.0, WARNING, true);
                c.text(
                    sx + 32.0,
                    tip_y + 24.0,
                    "После включения и отключения VPN приложение Antigravity нужно перезапустить.",
                    11.0,
                    TEXT,
                    false,
                );
                c.text(
                    sx + 32.0,
                    tip_y + 38.0,
                    "Ошибка «Agent execution terminated due to error (Error ID: ...)» = запрос пошёл мимо VPN.",
                    10.0,
                    MUTED,
                    false,
                );

                // Action Bar
                let act_y = tip_y + 68.0;
                c.button(
                    sx + 20.0,
                    act_y,
                    165.0,
                    if self.antigravity_checking_all { "Тестирование… ⟳" } else { "Запустить тест ↻" },
                    Action::RunAntigravityServersCheck,
                    self.antigravity_checking_all,
                );
                c.button(
                    sx + 195.0,
                    act_y,
                    225.0,
                    if self.antigravity_auto_select {
                        "☑ Автовыбор (HTTP 400)"
                    } else {
                        "☐ Автовыбор сервера"
                    },
                    Action::ToggleAntigravityAutoSelect,
                    self.antigravity_auto_select,
                );

                let mut effective = self.settings.clone();
                multilink_core::exclude_expired_access(&mut effective);
                let eligible: Vec<(usize, &multilink_core::Node)> = effective
                    .nodes
                    .iter()
                    .enumerate()
                    .filter(|(_, n)| n.enabled)
                    .collect();

                let page_size = 5;
                let total_pages = (eligible.len() + page_size - 1) / page_size;
                let current_page = self.antigravity_check_page.min(total_pages.saturating_sub(1));

                if total_pages > 1 {
                    c.text(
                        sx + modal_w - 200.0,
                        act_y + 12.0,
                        &format!("Стр. {} / {}", current_page + 1, total_pages),
                        12.0,
                        MUTED,
                        false,
                    );
                    if current_page > 0 {
                        c.button(
                            sx + modal_w - 110.0,
                            act_y,
                            38.0,
                            "←",
                            Action::AntigravityCheckPage(current_page - 1),
                            false,
                        );
                    }
                    if current_page + 1 < total_pages {
                        c.button(
                            sx + modal_w - 62.0,
                            act_y,
                            38.0,
                            "→",
                            Action::AntigravityCheckPage(current_page + 1),
                            false,
                        );
                    }
                }

                // Table Header
                let th_y = act_y + 46.0;
                c.text(sx + 30.0, th_y, "СЕРВЕР", 10.0, MUTED, true);
                c.text(sx + 230.0, th_y, "СТАТУС GOOGLE API", 10.0, MUTED, true);
                c.text(sx + modal_w - 235.0, th_y, "ТЕСТ", 10.0, MUTED, true);
                c.text(sx + modal_w - 130.0, th_y, "МАРШРУТ", 10.0, MUTED, true);
                c.line(sx + 20.0, th_y + 16.0, modal_w - 40.0);

                // Rows
                let start_idx = current_page * page_size;
                for (row_i, (node_idx, node)) in eligible.iter().skip(start_idx).take(page_size).enumerate() {
                    let ry = th_y + 24.0 + row_i as f64 * 46.0;
                    c.line(sx + 20.0, ry + 40.0, modal_w - 40.0);

                    // Row hit target for right-click single check
                    let row_rect = LogicalRect::from_xywh(sx + 20.0, ry, modal_w - 245.0, 42.0);
                    c.hits.push((row_rect, Action::SelectAntigravityServer(*node_idx)));

                    c.text(sx + 30.0, ry + 12.0, &shortened(&node.name, 22), 14.0, TEXT, true);

                    let status = self.antigravity_servers_status.get(&node.name);
                    match status {
                        Some(Ok(multilink_mihomo::AntigravityCheck::Responded(code))) => {
                            c.text(sx + 230.0, ry + 12.0, "✓ Доступен", 13.0, ACCENT, true);
                            c.text(sx + 320.0, ry + 13.0, &format!("HTTP {code}"), 11.0, MUTED, false);
                        }
                        Some(Ok(multilink_mihomo::AntigravityCheck::Blocked(code))) => {
                            c.text(sx + 230.0, ry + 12.0, "✗ Заблокирован", 13.0, DANGER, true);
                            c.text(sx + 340.0, ry + 13.0, &format!("HTTP {code}"), 11.0, MUTED, false);
                        }
                        Some(Ok(multilink_mihomo::AntigravityCheck::Inconclusive(code))) => {
                            c.text(sx + 230.0, ry + 12.0, "✗ Не отвечает", 13.0, DANGER, true);
                            c.text(sx + 330.0, ry + 13.0, &format!("HTTP {code}"), 11.0, MUTED, false);
                        }
                        Some(Err(e)) => {
                            c.text(sx + 230.0, ry + 12.0, "✗ Ошибка", 13.0, DANGER, true);
                            c.text(sx + 305.0, ry + 13.0, &shortened(e, 16), 10.0, MUTED, false);
                        }
                        None => {
                            if self.antigravity_checking_all {
                                c.text(sx + 230.0, ry + 12.0, "⟳ В очереди…", 12.0, WARNING, false);
                            } else {
                                c.text(sx + 230.0, ry + 12.0, "— Не проверялся", 12.0, MUTED, false);
                            }
                        }
                    }

                    // Single test button
                    c.button(
                        sx + modal_w - 235.0,
                        ry + 4.0,
                        88.0,
                        "↻ Тест",
                        Action::CheckSingleAntigravityServer(*node_idx),
                        false,
                    );

                    let target_id = multilink_core::server_route_target(&node.name);
                    let is_selected = rule_targets.get(self.rule_target).map_or(false, |(id, _)| id == &target_id);

                    if is_selected {
                        c.button(
                            sx + modal_w - 140.0,
                            ry + 4.0,
                            116.0,
                            "✓ Выбран",
                            Action::SelectAntigravityServer(*node_idx),
                            true,
                        );
                    } else {
                        c.button(
                            sx + modal_w - 140.0,
                            ry + 4.0,
                            116.0,
                            "Выбрать",
                            Action::SelectAntigravityServer(*node_idx),
                            false,
                        );
                    }
                }

                if eligible.is_empty() {
                    c.text(
                        sx + 30.0,
                        th_y + 40.0,
                        "В библиотеке нет включённых серверов",
                        14.0,
                        MUTED,
                        false,
                    );
                }

                // Bottom Buttons
                let bottom_y = sy + modal_h - 48.0;
                c.button(
                    sx + modal_w - 140.0,
                    bottom_y,
                    116.0,
                    "Закрыть",
                    Action::CloseAntigravityServersCheck,
                    false,
                );

                self.input_rect = None;
                self.hits = c.hits;
                return c.b.finish();
            }
            if *modal == Modal::Language {
                c.text(mx + 28.0, my + 24.0, "Язык интерфейса", 23.0, TEXT, true);
                c.text(
                    mx + 28.0,
                    my + 61.0,
                    "Выберите язык. Изменение применяется сразу.",
                    13.0,
                    MUTED,
                    false,
                );
                for (index, (key, label)) in i18n::OPTIONS.iter().enumerate() {
                    c.button(
                        mx + 28.0 + (index % 2) as f64 * 312.0,
                        my + 98.0 + (index / 2) as f64 * 50.0,
                        292.0,
                        label,
                        Action::SelectLanguage(index),
                        self.settings.language == *key,
                    );
                }
                c.button(
                    mx + 483.0,
                    my + 264.0,
                    149.0,
                    "Готово",
                    Action::Close,
                    false,
                );
                self.input_rect = None;
                self.hits = c.hits;
                return c.b.finish();
            }
            if matches!(modal, Modal::SubscriptionDelete | Modal::ClearLibrary) {
                c.text(
                    mx + 28.0,
                    my + 24.0,
                    if *modal == Modal::ClearLibrary {
                        "Очистить библиотеку?"
                    } else {
                        "Удалить подписку?"
                    },
                    23.0,
                    TEXT,
                    true,
                );
                c.text(
                    mx + 28.0,
                    my + 80.0,
                    &shortened(self.edit.text(), 65),
                    15.0,
                    TEXT,
                    true,
                );
                c.text(
                    mx + 28.0,
                    my + 122.0,
                    if *modal == Modal::ClearLibrary {
                        "Будут удалены все локальные серверы, подписки и сохранённый доступ."
                    } else {
                        "Будут удалены только серверы, принадлежащие одной этой подписке."
                    },
                    13.0,
                    MUTED,
                    false,
                );
                c.text(
                    mx + 28.0,
                    my + 154.0,
                    if *modal == Modal::ClearLibrary {
                        "Восстановить доступ можно обновлением ссылки или новым ключом."
                    } else {
                        "Общие и вручную добавленные серверы останутся."
                    },
                    13.0,
                    MUTED,
                    false,
                );
                c.text(
                    mx + 28.0,
                    my + 186.0,
                    "Привязки маршрутов сохранятся с временным переходом на VPN-пул.",
                    12.0,
                    MUTED,
                    false,
                );
                c.button(
                    mx + 355.0,
                    my + 264.0,
                    115.0,
                    "Отмена",
                    Action::Close,
                    false,
                );
                c.button(
                    mx + 483.0,
                    my + 264.0,
                    149.0,
                    "Удалить",
                    Action::Submit,
                    true,
                );
                self.input_rect = None;
                self.hits = c.hits;
                return c.b.finish();
            }
            if *modal == Modal::RouteImport {
                let count = self
                    .imported_routing
                    .as_ref()
                    .map(|v| v.rules.len())
                    .unwrap_or(0);
                c.text(mx + 28.0, my + 24.0, "Импорт маршрутов", 23.0, TEXT, true);
                c.text(
                    mx + 28.0,
                    my + 80.0,
                    &format!("В файле: {count} правил. Будут импортированы заметки и политика."),
                    13.0,
                    TEXT,
                    false,
                );
                c.text(
                    mx + 28.0,
                    my + 118.0,
                    "Добавить: совпадающие правила будут обновлены.",
                    13.0,
                    MUTED,
                    false,
                );
                c.text(
                    mx + 28.0,
                    my + 148.0,
                    "Заменить всё: текущий список маршрутов будет заменён.",
                    13.0,
                    MUTED,
                    false,
                );
                c.text(
                    mx + 28.0,
                    my + 190.0,
                    &shortened(i18n::tr(language, &self.notice).as_ref(), 76),
                    11.0,
                    MUTED,
                    false,
                );
                c.button(mx + 28.0, my + 264.0, 115.0, "Отмена", Action::Close, false);
                c.button(
                    mx + 285.0,
                    my + 264.0,
                    160.0,
                    "Заменить всё",
                    Action::ReplaceRoutes,
                    false,
                );
                c.button(
                    mx + 465.0,
                    my + 264.0,
                    167.0,
                    "Добавить",
                    Action::Submit,
                    true,
                );
                self.hits = c.hits;
                return c.b.finish();
            }
            let note = *modal == Modal::Note;
            let pin_auth = *modal == Modal::PinAuth;
            let set_pin = *modal == Modal::SetPin;
            let is_pin = pin_auth || set_pin;
            let subscription = matches!(
                modal,
                Modal::SubscriptionName | Modal::SubscriptionUrl | Modal::AccessCode
            );
            let import = *modal == Modal::Import || note || subscription || is_pin;
            let ru = *modal == Modal::RuRule;
            let process = *modal == Modal::ProcessRule;
            let route_fields = matches!(modal, Modal::Rule | Modal::ProcessRule);
            let antigravity = route_fields && self.antigravity_template;
            c.text(
                mx + 28.0,
                my + 23.0,
                if note {
                    "Заметка к правилу"
                } else if pin_auth {
                    "Ввод PIN-кода"
                } else if set_pin {
                    if self.settings.pin_code.is_some() {
                        "Смена PIN-кода"
                    } else {
                        "Установка PIN-кода"
                    }
                } else if *modal == Modal::AccessCode {
                    "Активация доступа"
                } else if *modal == Modal::SubscriptionName {
                    "Название подписки"
                } else if *modal == Modal::SubscriptionUrl {
                    "Изменить ссылку подписки"
                } else if import {
                    "Добавить подписку или серверы"
                } else if antigravity {
                    "Маршрут для Antigravity"
                } else if self.editing_rule.is_some() {
                    "Изменить правило"
                } else if ru {
                    "Российские сайты"
                } else if process {
                    "Маршрут для приложения"
                } else {
                    "Новое правило"
                },
                23.0,
                TEXT,
                true,
            );
            if route_fields {
                c.button(
                    mx + 28.0,
                    my + 68.0,
                    155.0,
                    "Домены и IP",
                    Action::RuleField(false),
                    !process,
                );
                c.button(
                    mx + 195.0,
                    my + 68.0,
                    155.0,
                    "Приложения",
                    Action::RuleField(true),
                    process,
                );
                c.text(
                    mx + 367.0,
                    my + 81.0,
                    "Один маршрут для всех значений",
                    12.0,
                    MUTED,
                    false,
                );
            } else {
                c.text(
                    mx + 28.0,
                    my + 68.0,
                    if note {
                        "Короткая заметка: назначение правила, приложение или пояснение."
                    } else if pin_auth {
                        "Введите PIN-код для доступа к управлению настройками и маршрутами."
                    } else if set_pin {
                        "Задайте PIN-код (от 4 символов) для блокировки изменений."
                    } else if *modal == Modal::AccessCode {
                        "Ключ из 6 символов. Например: AB7K9X. Регистр букв не важен."
                    } else if *modal == Modal::SubscriptionName {
                        "Удобное название для списка. До 80 символов."
                    } else if *modal == Modal::SubscriptionUrl {
                        "Новая ссылка применится после успешной загрузки и проверки серверов."
                    } else if import {
                        "Вставьте HTTPS-ссылку, Everty JSON, VLESS URI, Base64 или YAML."
                    } else if ru {
                        if self.editing_rule.is_some() {
                            "Маршрут для локального набора российских сайтов."
                        } else {
                            "Зоны .ru, .рф и локальный набор российских сервисов."
                        }
                    } else {
                        "Домены, ссылки или IP через запятую или точку с запятой."
                    },
                    13.0,
                    MUTED,
                    false,
                );
            }
            let field = LogicalRect::from_xywh(
                mx + 28.0,
                my + if route_fields { 116.0 } else { 111.0 },
                604.0,
                49.0,
            );
            self.input_rect = if ru { None } else { Some(field) };
            if ru {
                c.b.fill_rect(field, BG, 8.0);
                c.b.stroke_rect(field, LINE, 1.0, 8.0);
                c.text(
                    field.left() + 14.0,
                    field.top() + 14.0,
                    "Российские сайты и домены",
                    14.0,
                    TEXT,
                    true,
                );
            } else {
                self.input_view.paint(
                    &self.edit,
                    &mut self.edit_scroll,
                    field,
                    self.field_focused && !self.busy,
                    self.caret_on,
                    field.contains(self.pointer),
                    if note {
                        "Например: основной и вспомогательный процессы приложения"
                    } else if pin_auth {
                        "PIN-код"
                    } else if set_pin {
                        "Новый PIN-код"
                    } else if *modal == Modal::AccessCode {
                        "AB7K9X"
                    } else if *modal == Modal::SubscriptionName {
                        "Например: Основная подписка"
                    } else if import {
                        "https://provider.example/subscription/…"
                    } else if process && cfg!(target_os = "macos") {
                        "Safari или /Applications/…/Contents/MacOS/…"
                    } else if process {
                        "program.exe"
                    } else {
                        "example.com или 192.168.0.0/16"
                    },
                    &mut c,
                );
            }
            if !ru {
                c.button(
                    mx + 28.0,
                    my + 177.0,
                    142.0,
                    if process && cfg!(target_os = "macos") {
                        "Выбрать .app…"
                    } else {
                        "Вставить"
                    },
                    if process && cfg!(target_os = "macos") {
                        Action::ChooseApplication
                    } else {
                        Action::Paste
                    },
                    false,
                );
            }
            if *modal == Modal::Import {
                c.button(
                    mx + 410.0,
                    my + 177.0,
                    222.0,
                    "Открыть файл…",
                    Action::ImportNodeFile,
                    false,
                );
            }
            if !import {
                let (target_id, target_name) = rule_targets
                    .get(self.rule_target)
                    .map(|(id, name)| (id.as_str(), name.as_str()))
                    .unwrap_or(("", "Напрямую"));
                let is_failed = self.antigravity_failed_nodes.contains(target_id);
                let display_label = if is_failed {
                    format!("⚠ {}", shortened(target_name, 15))
                } else {
                    shortened(target_name, 18).to_string()
                };
                c.button(
                    mx + 410.0,
                    my + 177.0,
                    222.0,
                    &format!("Маршрут: {display_label} →"),
                    Action::RuleTarget,
                    false,
                );
            }
            c.text(
                mx + if ru { 28.0 } else { 190.0 },
                my + 188.0,
                if ru {
                    "Локальный набор MetaCubeX"
                } else if cfg!(target_os = "macos") {
                    "⌘V · ⌘A · Enter · Esc"
                } else {
                    "Ctrl+V · Ctrl+A · Enter · Esc"
                },
                12.0,
                MUTED,
                false,
            );
            if !import {
                c.text(
                    mx + 190.0,
                    my + 225.0,
                    &shortened(&self.note_draft.replace(['\n', '\r', '\t'], " "), 48),
                    11.0,
                    MUTED,
                    false,
                );
                c.button(
                    mx + 28.0,
                    my + 216.0,
                    142.0,
                    "Заметка…",
                    Action::RuleNote,
                    false,
                );
                if antigravity {
                    c.button(
                        mx + 180.0,
                        my + 216.0,
                        220.0,
                        if self.antigravity_auto_select {
                            "☑ Автовыбор (HTTP 400)"
                        } else {
                            "☐ Автовыбор сервера"
                        },
                        Action::ToggleAntigravityAutoSelect,
                        self.antigravity_auto_select,
                    );
                    c.button(
                        mx + 410.0,
                        my + 216.0,
                        222.0,
                        "Проверить регион ☰",
                        Action::OpenAntigravityServersCheck,
                        false,
                    );
                }
            }
            if let Some((_, message)) = self.antigravity_result.as_ref().filter(|(id, _)| {
                antigravity
                    && rule_targets
                        .get(self.rule_target)
                        .is_some_and(|(target, _)| id == target)
            }) {
                let is_fail = message.contains("заблокирован")
                    || message.contains("не дала ответа")
                    || message.contains("Ошибка")
                    || message.contains("оборвался");
                c.text(
                    mx + 28.0,
                    my + 258.0,
                    &shortened(i18n::tr(language, message).as_ref(), 90),
                    12.0,
                    if is_fail { DANGER } else { ACCENT },
                    false,
                );
            }
            c.text(
                mx + 28.0,
                my + if import { 229.0 } else { 279.0 },
                &shortened(
                    i18n::tr(language, &self.notice).as_ref(),
                    if antigravity { 99 } else { 76 },
                ),
                11.0,
                MUTED,
                false,
            );
            c.button(
                mx + 355.0,
                my + modal_height - 56.0,
                115.0,
                "Отмена",
                Action::Close,
                false,
            );
            c.button(
                mx + 483.0,
                my + modal_height - 56.0,
                149.0,
                if self.busy {
                    "Проверка…"
                } else if pin_auth {
                    "Подтвердить"
                } else if set_pin {
                    "Сохранить"
                } else if note {
                    "Готово"
                } else if *modal == Modal::AccessCode {
                    "Активировать"
                } else if subscription {
                    "Сохранить"
                } else if import {
                    "Импортировать"
                } else if self.editing_rule.is_some() {
                    "Сохранить"
                } else {
                    "Добавить"
                },
                Action::Submit,
                true,
            );
            if self.selecting_target && !import {
                c.hits.clear();
                c.card(mx, my, 660.0, 320.0);
                c.text(mx + 25.0, my + 16.0, "Выберите маршрут", 20.0, TEXT, true);
                c.text(
                    mx + 25.0,
                    my + 48.0,
                    "Страны определяются по названиям включённых серверов.",
                    12.0,
                    MUTED,
                    false,
                );
                let pages = rule_targets.len().div_ceil(18).max(1);
                let page = self.target_page.min(pages - 1);
                c.text(
                    mx + 300.0,
                    my + 24.0,
                    &format!("{} / {}", page + 1, pages),
                    12.0,
                    MUTED,
                    false,
                );
                if page > 0 {
                    c.button(
                        mx + 357.0,
                        my + 15.0,
                        64.0,
                        "←",
                        Action::TargetPage(page - 1),
                        false,
                    );
                }
                if page + 1 < pages {
                    c.button(
                        mx + 432.0,
                        my + 15.0,
                        64.0,
                        "→",
                        Action::TargetPage(page + 1),
                        false,
                    );
                }
                for (i, (id, label)) in rule_targets.iter().enumerate().skip(page * 18).take(18) {
                    let local = i - page * 18;
                    let col = local / 9;
                    let row = local % 9;
                    let rx = mx + 20.0 + col as f64 * 310.0;
                    let ry = my + 70.0 + row as f64 * 27.0;
                    let action = Action::SelectRuleTarget(i);
                    let rect = LogicalRect::from_xywh(rx, ry, 302.0, 25.0);
                    let selected = i == self.rule_target;
                    if selected || c.hover == Some(action) {
                        c.b.fill_rect(
                            rect,
                            if selected {
                                SOFT
                            } else {
                                Color::rgba(32, 40, 50, 255)
                            },
                            5.0,
                        );
                    }
                    let value = if label.starts_with("Регион ") {
                        label.clone()
                    } else if let Some(code) = id.strip_prefix("COUNTRY-") {
                        format!("{code}  ·  {label}")
                    } else {
                        label.clone()
                    };
                    c.b.push_clip(rect);
                    c.text(
                        rx + 10.0,
                        ry + 3.0,
                        &shortened(&value, 35),
                        13.0,
                        if selected { ACCENT } else { TEXT },
                        selected,
                    );
                    let _ = c.b.pop_clip();
                    c.hits.push((rect, action));
                }
                c.button(
                    mx + 516.0,
                    my + 15.0,
                    115.0,
                    "Назад",
                    Action::RuleTarget,
                    false,
                );
            }
        }
        self.hits = c.hits;
        self.disabled_hits = c.disabled_hits;
        c.b.finish()
    }
}
fn policy_detail(policy: multilink_core::Policy) -> &'static str {
    match policy {
        multilink_core::Policy::Balance => "Распределяет подключения между доступными серверами.",
        multilink_core::Policy::Latency => "Выбирает сервер с наименьшей задержкой.",
        multilink_core::Policy::Stable => "Использует первый доступный сервер и резерв при сбое.",
        multilink_core::Policy::Smart => {
            "Сравнивает доступность и скорость, отдельно для правил сайтов."
        }
    }
}
fn server_list(
    c: &mut Canvas<'_>,
    settings: &Settings,
    connected: bool,
    offset: usize,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
) {
    c.card(x, y, w, h);
    if settings.nodes.is_empty() {
        c.rect(x + 24.0, y + 24.0, 42.0, 42.0, SOFT, 11.0);
        c.text(x + 38.0, y + 28.0, "+", 26.0, ACCENT, false);
        c.text(
            x + 82.0,
            y + 24.0,
            "Начните с вашей подписки",
            19.0,
            TEXT,
            true,
        );
        c.text(
            x + 82.0,
            y + 57.0,
            "Добавьте VLESS-серверы — остальное возьмёт на себя ядро.",
            13.0,
            MUTED,
            false,
        );
        c.button(
            x + 24.0,
            y + 103.0,
            206.0,
            "+ Добавить подписку",
            Action::Import,
            true,
        );
        return;
    }
    let col_qr = x + w - 58.0;
    let col_diag = col_qr - 40.0;
    let col_badge = col_diag - 105.0;
    let col_delay = col_badge - 85.0;

    c.text(x + 22.0, y + 14.0, "СЕРВЕР", 10.0, MUTED, true);
    c.text(col_delay, y + 14.0, "ЗАДЕРЖКА", 10.0, MUTED, true);
    c.text(col_badge, y + 14.0, "СОСТОЯНИЕ", 10.0, MUTED, true);
    c.text(col_diag + 6.0, y + 14.0, "ТЕСТ", 10.0, MUTED, true);
    c.text(col_qr + 9.0, y + 14.0, "QR", 10.0, MUTED, true);
    c.line(x + 18.0, y + 42.0, w - 36.0);
    let mut order: Vec<_> = settings.nodes.iter().enumerate().collect();
    if settings.rank_servers {
        order.sort_by_key(|(_, node)| {
            (
                std::cmp::Reverse(node.enabled),
                std::cmp::Reverse(node.quality.score().unwrap_or(0)),
            )
        });
    }
    for (position, (i, node)) in order
        .into_iter()
        .enumerate()
        .skip(offset)
        .take(((h - 47.0) / 59.0).max(1.0) as usize)
    {
        let ry = y + 48.0 + (position - offset) as f64 * 59.0;
        let row = LogicalRect::from_xywh(x + 10.0, ry, w - 20.0, 55.0);
        if !c.busy && c.hover == Some(Action::Toggle(i)) {
            c.b.fill_rect(
                row,
                if c.pressed == Some(Action::Toggle(i)) {
                    SOFT
                } else {
                    Color::rgba(30, 40, 48, 255)
                },
                7.0,
            );
        }
        c.rect(
            x + 20.0,
            ry + 9.0,
            34.0,
            34.0,
            if node.enabled { SOFT } else { BG },
            9.0,
        );
        c.text(
            x + 29.0,
            ry + 15.0,
            if node.enabled { "↗" } else { "—" },
            17.0,
            if node.enabled { ACCENT } else { MUTED },
            false,
        );
        let name_clip_w = (col_delay - (x + 68.0) - 12.0).max(10.0);
        c.b.push_clip(LogicalRect::from_xywh(x + 68.0, ry, name_clip_w, 55.0));
        let name_str = shortened(&node.name, 28);
        c.raw_text(
            x + 68.0,
            ry + 6.0,
            &name_str,
            14.0,
            TEXT,
            true,
        );
        let name_w = c.text_width(&name_str, 14.0, true).min(name_clip_w - 42.0);
        c.draw_availability_bars(x + 68.0 + name_w + 10.0, ry + 6.0, node, connected);
        let is_ru = multilink_core::is_russian_node(node);
        let quality_text = if is_ru {
            "🇷🇺 Локальный RU-узел (прямой доступ / вне пула умного режима)".to_string()
        } else {
            node.quality
                .score()
                .map(|score| {
                    format!(
                        "{}{} / 100 · {} проверок",
                        if node.quality.samples < 3 { "≈" } else { "" },
                        score,
                        node.quality.samples
                    )
                })
                .unwrap_or_else(|| "Оценка появится после проверок подключения".into())
        };
        c.text(
            x + 68.0,
            ry + 29.0,
            &quality_text,
            11.0,
            if is_ru { ACCENT } else { MUTED },
            false,
        );
        let _ = c.b.pop_clip();
        c.text(
            col_delay,
            ry + 18.0,
            &if connected {
                node.delay.map(|d| format!("{d} ms")).unwrap_or("—".into())
            } else {
                "—".into()
            },
            14.0,
            if node.delay.is_some() { ACCENT } else { MUTED },
            true,
        );
        let (badge_text, badge_active, badge_w) = if !node.enabled {
            ("Исключён", false, 84.0)
        } else if connected && node.delay.is_some() {
            ("● Доступен", true, 96.0)
        } else {
            ("🕒 Ожидание", false, 96.0)
        };
        c.badge_sized(
            col_badge,
            ry + 14.0,
            badge_text,
            badge_active,
            badge_w,
        );
        c.icon_button(
            col_diag,
            ry + 8.5,
            12,
            Action::OpenServerDiagnostics(i),
            "Расширенная диагностика сервера",
        );
        c.icon_button(
            col_qr,
            ry + 8.5,
            21,
            Action::ShareNode(i),
            "Поделиться узлом (QR)",
        );
        if !c.busy {
            c.hits.push((row, Action::Toggle(i)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn large_unicode_paste_has_bounded_layout_and_correct_caret_mapping() {
        let mut edit = TextEdit::new(4 * 1024 * 1024);
        edit.insert(&"абв😀".repeat(100_000));
        let mut text = TextSystem::new();
        let mut cache = TextCache::default();
        let mut c = Canvas {
            language: i18n::Language::Ru,
            tooltip: None,
            b: PaintBuilder::new(),
            text: &mut text,
            cache: &mut cache,
            hover: None,
            pressed: None,
            locked: false,
            busy: false,
            hits: Vec::new(),
            disabled_hits: Vec::new(),
        };
        let mut view = InputView::default();
        let mut scroll = 0.0;
        view.paint(
            &edit,
            &mut scroll,
            LogicalRect::from_xywh(0.0, 0.0, 604.0, 49.0),
            true,
            true,
            false,
            "",
            &mut c,
        );
        assert!(view.value.len() <= 1536);
        assert_eq!(view.index_at(f64::MAX), edit.caret());
        assert!(edit.text().is_char_boundary(view.index_at(0.0)));
        edit.set_caret(edit.text().len() / 2, false);
        view.paint(
            &edit,
            &mut scroll,
            LogicalRect::from_xywh(0.0, 0.0, 604.0, 49.0),
            true,
            true,
            false,
            "",
            &mut c,
        );
        assert!(view.value.len() <= 1536);
        assert!(edit.text().is_char_boundary(view.index_at(100.0)));
    }
    #[test]
    fn compact_controls_and_country_picker_are_available_without_a_window() {
        let mut app = App::new(None);
        app.compact = true;
        app.settings.nodes = multilink_core::parse_subscription(
            "vless://00000000-0000-4000-8000-000000000001@example.invalid:443#DE-01",
        )
        .unwrap();
        app.paint(390.0, 500.0);
        for target in [Action::Connect, Action::Expanded] {
            let rect = app
                .hits
                .iter()
                .find(|(_, action)| *action == target)
                .expect("compact control")
                .0;
            assert!(
                rect.left() >= 0.0
                    && rect.top() >= 0.0
                    && rect.right() <= 390.0
                    && rect.bottom() <= 500.0
            );
        }
        let circle = app
            .hits
            .iter()
            .find(|(_, action)| *action == Action::Connect)
            .unwrap()
            .0;
        app.pointer = LogicalPoint::new(circle.left() + 1.0, circle.top() + 1.0);
        assert_eq!(app.action_at_pointer(), None);
        app.pointer = LogicalPoint::new(
            circle.left() + circle.size.width.0 / 2.0,
            circle.top() + circle.size.height.0 / 2.0,
        );
        assert_eq!(app.action_at_pointer(), Some(Action::Connect));
        app.compact = false;
        app.page = Page::Routing;
        app.paint(1040.0, 740.0);
        assert!(app
            .hits
            .iter()
            .any(|(_, action)| *action == Action::AddRule));
        assert!(app
            .hits
            .iter()
            .any(|(_, action)| *action == Action::AddRuDomains));
        app.action(Action::AddRule);
        app.action(Action::RuleTarget);
        app.paint(1040.0, 740.0);
        let germany = app
            .rule_targets()
            .iter()
            .position(|(id, _)| id == "COUNTRY-DE")
            .unwrap();
        assert!(app
            .hits
            .iter()
            .any(|(_, action)| *action == Action::SelectRuleTarget(germany)));
        app.action(Action::SelectRuleTarget(germany));
        assert_eq!(app.rule_targets()[app.rule_target].0, "COUNTRY-DE");
        assert!(!app.selecting_target);
        let template = app.settings.nodes[0].clone();
        app.settings.nodes = multilink_core::COUNTRIES
            .iter()
            .take(30)
            .map(|(code, _, _)| {
                let mut node = template.clone();
                node.name = format!("{code}-01");
                node
            })
            .collect();
        app.action(Action::RuleTarget);
        app.paint(1040.0, 740.0);
        assert!(app.hits.iter().any(|(_, a)| *a == Action::TargetPage(1)));
        assert!(!app
            .hits
            .iter()
            .any(|(_, a)| *a == Action::SelectRuleTarget(20)));
        app.action(Action::TargetPage(1));
        app.paint(1040.0, 740.0);
        let rect = app
            .hits
            .iter()
            .find(|(_, a)| *a == Action::SelectRuleTarget(20))
            .expect("second page country")
            .0;
        assert!(rect.right() <= 1040.0 && rect.bottom() <= 740.0);
        app.action(Action::SelectRuleTarget(20));
        assert_eq!(app.target_page, 1);
    }
}
