//! Modal dialogs: import, subscription, profile editor, settings, confirmations.
//!
//! Every dialog is an `egui::Modal` (dimmed backdrop). Esc cancels; a click on
//! the backdrop does not, so a half-filled form is not lost by accident.
//! Buttons live in a footer outside any scroll area and are always visible.

use std::path::PathBuf;

use eframe::egui::{self, Frame, Margin, RichText, Ui};
use rustbox_core::model::{Profile, Subscription};
use rustbox_core::{GroupId, ProfileId, Settings, Store, link};

use crate::app::RustBoxApp;
use crate::theme::{self, Palette};

#[derive(Default)]
pub struct Dialogs {
    pub import: Option<String>,
    pub subscription: Option<SubscriptionDialog>,
    pub settings: Option<Settings>,
    pub edit: Option<EditDialog>,
    pub confirm_delete: Option<Vec<ProfileId>>,
    pub confirm_delete_group: Option<GroupId>,
    /// (core executable, hint) when TUN privileges are missing.
    pub tun_hint: Option<(PathBuf, String)>,
}

impl Dialogs {
    pub fn any_open(&self) -> bool {
        self.import.is_some()
            || self.subscription.is_some()
            || self.settings.is_some()
            || self.edit.is_some()
            || self.confirm_delete.is_some()
            || self.confirm_delete_group.is_some()
            || self.tun_hint.is_some()
    }
}

#[derive(Default)]
pub struct SubscriptionDialog {
    /// `None` = new subscription.
    pub group: Option<GroupId>,
    pub name: String,
    pub url: String,
    pub user_agent: String,
}

impl SubscriptionDialog {
    pub fn edit(store: &Store, id: GroupId) -> Option<Self> {
        let g = store.group(id)?;
        let sub = g.subscription.clone().unwrap_or_default();
        Some(Self {
            group: Some(id),
            name: crate::view::group_name(g),
            url: sub.url,
            user_agent: sub.user_agent.unwrap_or_default(),
        })
    }
}

pub struct EditDialog {
    /// `None` = new profile.
    pub id: Option<ProfileId>,
    pub name: String,
    pub link: String,
    pub json: String,
    pub error: Option<String>,
}

impl EditDialog {
    pub fn for_profile(p: &Profile) -> Self {
        Self {
            id: Some(p.id),
            name: p.name.clone(),
            link: link::to_link(p),
            json: body_json(p),
            error: None,
        }
    }

    pub fn new_profile() -> Self {
        let template: Profile = serde_json::from_value(serde_json::json!({
            "name": "",
            "server": "example.com",
            "port": 443,
            "protocol": { "type": "vless", "uuid": "00000000-0000-0000-0000-000000000000" },
            "tls": { "sni": "example.com" },
            "transport": { "type": "ws", "path": "/" },
        }))
        .expect("valid template");
        Self {
            id: None,
            name: String::new(),
            link: String::new(),
            json: body_json(&template),
            error: None,
        }
    }
}

/// Profile JSON without bookkeeping fields.
fn body_json(p: &Profile) -> String {
    let mut v = serde_json::to_value(p).unwrap_or_default();
    if let Some(o) = v.as_object_mut() {
        for key in ["id", "group", "name", "latency"] {
            o.remove(key);
        }
    }
    serde_json::to_string_pretty(&v).unwrap_or_default()
}

pub fn show(app: &mut RustBoxApp, ctx: &egui::Context) {
    import_dialog(app, ctx);
    subscription_dialog(app, ctx);
    edit_dialog(app, ctx);
    settings_dialog(app, ctx);
    confirm_dialogs(app, ctx);
    tun_dialog(app, ctx);
}

// ── Building blocks ───────────────────────────────────────────

/// Shows a modal; returns the closure result and whether Esc was pressed.
fn modal<R>(
    ctx: &egui::Context,
    id: &str,
    width: f32,
    add: impl FnOnce(&mut Ui) -> R,
) -> (R, bool) {
    let p = Palette::for_ctx(ctx);
    let style = ctx.style_of(ctx.theme());
    let dark = style.visuals.dark_mode;
    let frame = Frame::new()
        .fill(p.surface)
        .stroke(egui::Stroke::new(1.0, p.border))
        .corner_radius(12)
        .inner_margin(Margin::same(22))
        .shadow(style.visuals.window_shadow);
    let resp = egui::Modal::new(egui::Id::new(id))
        .frame(frame)
        .backdrop_color(egui::Color32::from_black_alpha(if dark { 150 } else { 90 }))
        .show(ctx, |ui| {
            ui.set_width(width);
            add(ui)
        });
    let esc = resp.should_close() && !resp.backdrop_response.clicked();
    (resp.inner, esc)
}

fn title(ui: &mut Ui, text: &str, subtitle: Option<&str>) {
    ui.label(RichText::new(text).size(18.0).strong());
    if let Some(s) = subtitle {
        ui.label(RichText::new(s).color(Palette::of(ui).muted));
    }
    ui.add_space(10.0);
}

/// Right-aligned button row at the bottom of a dialog.
fn footer(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    ui.add_space(14.0);
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), add);
    });
}

fn error_banner(ui: &mut Ui, text: &str) {
    let p = Palette::of(ui);
    Frame::new()
        .fill(p.tint(p.bad, 0.14))
        .corner_radius(6)
        .inner_margin(Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(format!("⚠  {text}")).color(p.bad));
        });
}

fn hint(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).small().color(Palette::of(ui).muted));
}

fn form_grid(id: &str) -> egui::Grid {
    egui::Grid::new(id)
        .num_columns(2)
        .spacing([14.0, 8.0])
        .min_col_width(170.0)
}

// ── Dialogs ───────────────────────────────────────────────────

fn import_dialog(app: &mut RustBoxApp, ctx: &egui::Context) {
    let Some(mut text) = app.dialogs.import.take() else {
        return;
    };
    let mut keep = true;
    let (_, esc) = modal(ctx, "import", 600.0, |ui| {
        title(
            ui,
            "Импорт ссылок",
            Some(
                "По одной ссылке на строку. Подходит и содержимое подписки (base64 или Clash YAML).",
            ),
        );
        egui::ScrollArea::vertical()
            .max_height(300.0)
            .show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut text)
                        .hint_text("vless://…\nss://…")
                        .desired_rows(12)
                        .desired_width(f32::INFINITY)
                        .code_editor(),
                );
            });
        footer(ui, |ui| {
            let ready = !text.trim().is_empty();
            if ui
                .add_enabled_ui(ready, |ui| theme::primary_button(ui, "Импортировать"))
                .inner
                .clicked()
            {
                app.import_text(&text);
                keep = false;
            }
            if ui.button("Отмена").clicked() {
                keep = false;
            }
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                if ui.button("Вставить из буфера").clicked()
                    && let Ok(t) = arboard::Clipboard::new().and_then(|mut c| c.get_text())
                {
                    if !text.is_empty() && !text.ends_with('\n') {
                        text.push('\n');
                    }
                    text.push_str(&t);
                }
            });
        });
    });
    if keep && !esc {
        app.dialogs.import = Some(text);
    }
}

fn subscription_dialog(app: &mut RustBoxApp, ctx: &egui::Context) {
    let Some(mut d) = app.dialogs.subscription.take() else {
        return;
    };
    let mut keep = true;
    let editing = d.group.is_some();
    let (_, esc) = modal(ctx, "subscription", 560.0, |ui| {
        title(
            ui,
            if editing {
                "Группа"
            } else {
                "Новая подписка"
            },
            (!editing).then_some(
                "Ссылку на подписку выдаёт ваш VPN-сервис. Серверы обновятся автоматически.",
            ),
        );
        form_grid("sub_grid").show(ui, |ui| {
            ui.label("Название");
            ui.add(
                egui::TextEdit::singleline(&mut d.name)
                    .hint_text("Например, «Мой VPN»")
                    .desired_width(f32::INFINITY),
            );
            ui.end_row();
            ui.label("URL подписки");
            ui.add(
                egui::TextEdit::singleline(&mut d.url)
                    .hint_text("https://…")
                    .desired_width(f32::INFINITY),
            );
            ui.end_row();
            ui.label("User-Agent");
            ui.add(
                egui::TextEdit::singleline(&mut d.user_agent)
                    .desired_width(f32::INFINITY)
                    .hint_text(if app.settings.subscription_user_agent.trim().is_empty() {
                        "авто"
                    } else {
                        app.settings.subscription_user_agent.as_str()
                    }),
            );
            ui.end_row();
        });
        if editing {
            ui.add_space(4.0);
            hint(
                ui,
                "Пустой URL превращает группу в обычную (без обновления).",
            );
        }
        footer(ui, |ui| {
            let valid = !d.name.trim().is_empty() && (editing || !d.url.trim().is_empty());
            let label = if editing {
                "Сохранить"
            } else {
                "Добавить и обновить"
            };
            if ui
                .add_enabled_ui(valid, |ui| theme::primary_button(ui, label))
                .inner
                .clicked()
            {
                save_subscription(app, &d);
                keep = false;
            }
            if ui.button("Отмена").clicked() {
                keep = false;
            }
        });
    });
    if keep && !esc {
        app.dialogs.subscription = Some(d);
    }
}

fn save_subscription(app: &mut RustBoxApp, d: &SubscriptionDialog) {
    let url = d.url.trim().to_string();
    let user_agent = Some(d.user_agent.trim().to_string()).filter(|s| !s.is_empty());
    let group = match d.group {
        Some(id) => {
            let Some(g) = app.store.group_mut(id) else {
                return;
            };
            g.name = d.name.trim().to_string();
            g.subscription = if url.is_empty() {
                None
            } else {
                let mut sub = g.subscription.take().unwrap_or_default();
                sub.url = url;
                sub.user_agent = user_agent;
                Some(sub)
            };
            id
        }
        None => app.store.add_group(
            d.name.trim(),
            Some(Subscription {
                url,
                user_agent,
                ..Default::default()
            }),
        ),
    };
    app.save_store();
    app.current_group = group;
    app.selection.clear();
    app.update_subscription(group);
}

fn edit_dialog(app: &mut RustBoxApp, ctx: &egui::Context) {
    let Some(mut d) = app.dialogs.edit.take() else {
        return;
    };
    let mut keep = true;
    let (_, esc) = modal(ctx, "edit", 680.0, |ui| {
        title(
            ui,
            if d.id.is_some() {
                "Редактирование профиля"
            } else {
                "Новый профиль"
            },
            None,
        );
        form_grid("edit_grid").min_col_width(80.0).show(ui, |ui| {
            ui.label("Имя");
            ui.add(egui::TextEdit::singleline(&mut d.name).desired_width(f32::INFINITY));
            ui.end_row();
            ui.label("Ссылка");
            ui.horizontal(|ui| {
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut d.link)
                        .hint_text("vless://… — вставьте и нажмите «Разобрать»")
                        .desired_width(ui.available_width() - 110.0),
                );
                if ui
                    .button("Разобрать ↓")
                    .on_hover_text("Заполнить параметры из ссылки")
                    .clicked()
                    || (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                {
                    match link::parse_link(&d.link) {
                        Ok(p) => {
                            if d.name.is_empty() {
                                d.name = p.name.clone();
                            }
                            d.json = body_json(&p);
                            d.error = None;
                        }
                        Err(e) => d.error = Some(e.to_string()),
                    }
                }
            });
            ui.end_row();
        });
        ui.add_space(6.0);
        hint(ui, "Параметры (JSON)");
        egui::ScrollArea::vertical()
            .max_height(340.0)
            .show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut d.json)
                        .code_editor()
                        .desired_rows(16)
                        .desired_width(f32::INFINITY),
                );
            });
        if let Some(e) = &d.error {
            ui.add_space(6.0);
            error_banner(ui, e);
        }
        footer(ui, |ui| {
            if theme::primary_button(ui, "Сохранить").clicked() {
                match serde_json::from_str::<Profile>(&d.json) {
                    Ok(mut p) => {
                        p.name = d.name.trim().to_string();
                        match d.id.and_then(|id| app.store.profile_mut(id)) {
                            Some(existing) => {
                                p.id = existing.id;
                                p.group = existing.group;
                                *existing = p;
                            }
                            None => {
                                let group = app.current_group;
                                let ids = app.store.add_profiles(group, vec![p]);
                                app.selection = ids.into_iter().collect();
                            }
                        }
                        app.save_store();
                        keep = false;
                    }
                    Err(e) => d.error = Some(format!("ошибка в JSON: {e}")),
                }
            }
            if ui.button("Отмена").clicked() {
                keep = false;
            }
        });
    });
    if keep && !esc {
        app.dialogs.edit = Some(d);
    }
}

fn settings_dialog(app: &mut RustBoxApp, ctx: &egui::Context) {
    let Some(mut s) = app.dialogs.settings.take() else {
        return;
    };
    let mut keep = true;
    // The body scrolls, the footer never leaves the screen.
    let body_height = (ctx.content_rect().height() - 190.0).clamp(160.0, 560.0);
    let (_, esc) = modal(ctx, "settings", 600.0, |ui| {
        title(ui, "Настройки", None);
        egui::ScrollArea::vertical()
            .max_height(body_height)
            .auto_shrink([false, true])
            .show(ui, |ui| settings_body(app, ui, &mut s));

        footer(ui, |ui| {
            if theme::primary_button(ui, "Сохранить").clicked() {
                let restart = app.connection.is_some() && needs_restart(&app.settings, &s);
                app.settings = s.clone();
                app.save_settings();
                app.refresh_core_versions();
                app.info(if restart {
                    "настройки сохранены, переподключаюсь"
                } else {
                    "настройки сохранены"
                });
                if restart {
                    app.reconnect();
                }
                keep = false;
            }
            if ui.button("Отмена").clicked() {
                keep = false;
            }
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                if ui
                    .button("Сбросить")
                    .on_hover_text(
                        "Вернуть значения по умолчанию (сохранится только после «Сохранить»)",
                    )
                    .clicked()
                {
                    let selected = s.selected;
                    s = Settings {
                        selected,
                        ..Settings::default()
                    };
                }
            });
        });
    });
    if keep && !esc {
        app.dialogs.settings = Some(s);
    }
}

fn settings_body(app: &RustBoxApp, ui: &mut Ui, s: &mut Settings) {
    let p = Palette::of(ui);
    let section = |ui: &mut Ui, name: &str| {
        ui.add_space(10.0);
        theme::section_title(ui, name);
        ui.add_space(2.0);
    };

    section(ui, "Входящее подключение");
    form_grid("inbound").show(ui, |ui| {
        ui.label("Адрес");
        ui.add(egui::TextEdit::singleline(&mut s.listen).desired_width(200.0))
            .on_hover_text("127.0.0.1 — только этот компьютер; 0.0.0.0 — вся локальная сеть");
        ui.end_row();
        ui.label("Порт HTTP + SOCKS5");
        ui.add(egui::DragValue::new(&mut s.port).range(1..=65535));
        ui.end_row();
    });

    section(ui, "Маршрутизация и DNS");
    ui.checkbox(&mut s.bypass_lan, "Локальные адреса напрямую (bypass LAN)");
    form_grid("dns").show(ui, |ui| {
        ui.label("Удалённый DNS");
        ui.add(egui::TextEdit::singleline(&mut s.dns_remote).desired_width(f32::INFINITY))
            .on_hover_text("https://…, tls://…, tcp://…, udp://… или IP");
        ui.end_row();
        ui.label("Прямой DNS");
        ui.add(egui::TextEdit::singleline(&mut s.dns_direct).desired_width(f32::INFINITY))
            .on_hover_text("local — системный резолвер, или IP");
        ui.end_row();
    });

    section(ui, "TUN (sing-box)");
    form_grid("tun").show(ui, |ui| {
        ui.label("Стек");
        egui::ComboBox::from_id_salt("tun_stack")
            .selected_text(s.tun_stack.clone())
            .show_ui(ui, |ui| {
                for stack in ["mixed", "system", "gvisor"] {
                    ui.selectable_value(&mut s.tun_stack, stack.to_string(), stack);
                }
            });
        ui.end_row();
        ui.label("MTU");
        ui.add(egui::DragValue::new(&mut s.tun_mtu).range(1280..=65535));
        ui.end_row();
    });

    section(ui, "Ядра");
    form_grid("cores").show(ui, |ui| {
        path_field(ui, "sing-box", &mut s.sing_box_path);
        path_field(ui, "Xray", &mut s.xray_path);
        ui.label("Уровень лога");
        egui::ComboBox::from_id_salt("log_level")
            .selected_text(s.log_level.clone())
            .show_ui(ui, |ui| {
                for level in ["debug", "info", "warn", "error"] {
                    ui.selectable_value(&mut s.log_level, level.to_string(), level);
                }
            });
        ui.end_row();
    });
    for (kind, v) in &app.core_versions {
        match v {
            Ok(v) => ui.label(
                RichText::new(format!("✓  {kind}: {v}"))
                    .small()
                    .color(p.muted),
            ),
            Err(e) => ui.label(
                RichText::new(format!("⚠  {kind}: {e}"))
                    .small()
                    .color(p.warn),
            ),
        };
    }

    section(ui, "Проверка задержки");
    form_grid("test").show(ui, |ui| {
        ui.label("URL");
        ui.add(egui::TextEdit::singleline(&mut s.test_url).desired_width(f32::INFINITY));
        ui.end_row();
        ui.label("Таймаут, мс");
        ui.add(
            egui::DragValue::new(&mut s.test_timeout_ms)
                .range(500..=30000)
                .speed(100),
        );
        ui.end_row();
        ui.label("Одновременно");
        ui.add(egui::DragValue::new(&mut s.test_concurrency).range(1..=64));
        ui.end_row();
    });

    section(ui, "Подписки");
    form_grid("subs").show(ui, |ui| {
        ui.label("User-Agent");
        ui.add(
            egui::TextEdit::singleline(&mut s.subscription_user_agent)
                .hint_text("авто")
                .desired_width(f32::INFINITY),
        )
        .on_hover_text(
            "Пусто — автоматически: полные конфиги Xray с балансировкой (как в Happ), \
             если Xray установлен, иначе обычные ссылки",
        );
        ui.end_row();
    });
    ui.checkbox(
        &mut s.subscription_via_proxy,
        "Обновлять через активное подключение",
    );
    hint(
        ui,
        "Помогает, если сайт подписки заблокирован у провайдера.",
    );
}

fn needs_restart(old: &Settings, new: &Settings) -> bool {
    let strip = |s: &Settings| Settings {
        test_url: String::new(),
        test_timeout_ms: 0,
        test_concurrency: 0,
        test_kind: Default::default(),
        subscription_user_agent: String::new(),
        subscription_via_proxy: false,
        selected: None,
        ..s.clone()
    };
    strip(old) != strip(new)
}

fn path_field(ui: &mut Ui, label: &str, value: &mut Option<PathBuf>) {
    ui.label(format!("Путь к {label}"));
    let mut text = value
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    if ui
        .add(
            egui::TextEdit::singleline(&mut text)
                .desired_width(f32::INFINITY)
                .hint_text("определяется автоматически"),
        )
        .changed()
    {
        *value = Some(text.trim())
            .filter(|t| !t.is_empty())
            .map(PathBuf::from);
    }
    ui.end_row();
}

fn confirm(ctx: &egui::Context, id: &str, heading: &str, text: &str, action: &str) -> Option<bool> {
    let mut answer = None;
    let (_, esc) = modal(ctx, id, 400.0, |ui| {
        title(ui, heading, None);
        ui.label(text);
        footer(ui, |ui| {
            if theme::danger_button(ui, action).clicked() {
                answer = Some(true);
            }
            if ui.button("Отмена").clicked() {
                answer = Some(false);
            }
        });
    });
    if esc { Some(false) } else { answer }
}

fn confirm_dialogs(app: &mut RustBoxApp, ctx: &egui::Context) {
    if let Some(ids) = app.dialogs.confirm_delete.take() {
        let text = if ids.len() == 1 {
            let name = app
                .store
                .profile(ids[0])
                .map(|p| p.display_name())
                .unwrap_or_default();
            format!("Профиль «{name}» будет удалён.")
        } else {
            format!("Будет удалено профилей: {}.", ids.len())
        };
        match confirm(ctx, "confirm_delete", "Удалить?", &text, "Удалить") {
            Some(true) => app.delete_profiles(&ids),
            Some(false) => {}
            None => app.dialogs.confirm_delete = Some(ids),
        }
    }
    if let Some(group) = app.dialogs.confirm_delete_group.take() {
        let name = app
            .store
            .group(group)
            .map(crate::view::group_name)
            .unwrap_or_default();
        let count = app.store.profiles_in(group).count();
        let text = format!("Группа «{name}» и все её профили ({count}) будут удалены.");
        match confirm(
            ctx,
            "confirm_delete_group",
            "Удалить группу?",
            &text,
            "Удалить группу",
        ) {
            Some(true) => app.delete_group(group),
            Some(false) => {}
            None => app.dialogs.confirm_delete_group = Some(group),
        }
    }
}

fn tun_dialog(app: &mut RustBoxApp, ctx: &egui::Context) {
    let Some((exe, hint_text)) = app.dialogs.tun_hint.take() else {
        return;
    };
    let mut keep = true;
    let command = hint_text
        .split(" (")
        .next()
        .unwrap_or(&hint_text)
        .to_string();
    let (_, esc) = modal(ctx, "tun_hint", 560.0, |ui| {
        // Windows: TUN needs an elevated process, there is nothing to grant.
        if cfg!(windows) {
            title(
                ui,
                "Нужны права администратора",
                Some(
                    "Режим TUN создаёт сетевой адаптер — на Windows это разрешено только администратору.",
                ),
            );
            ui.label(&hint_text);
            footer(ui, |ui| {
                if theme::primary_button(ui, "Понятно").clicked() {
                    keep = false;
                }
            });
            return;
        }
        title(
            ui,
            "Нужны права для TUN",
            Some(
                "Режим TUN создаёт сетевой интерфейс — для этого ядру нужна capability CAP_NET_ADMIN.",
            ),
        );
        ui.label(
            RichText::new(format!("Ядро: {}", exe.display()))
                .small()
                .color(Palette::of(ui).muted),
        );
        ui.add_space(6.0);
        hint(
            ui,
            "Можно выдать права сейчас (попросит пароль) или выполнить команду в терминале:",
        );
        let mut cmd = hint_text.clone();
        ui.add(
            egui::TextEdit::multiline(&mut cmd)
                .code_editor()
                .desired_rows(2)
                .desired_width(f32::INFINITY),
        );
        footer(ui, |ui| {
            if theme::primary_button(ui, "Выдать права").clicked() {
                app.grant_tun(exe.clone());
            }
            if ui.button("Скопировать команду").clicked() {
                ui.ctx().copy_text(command.clone());
            }
            if ui.button("Закрыть").clicked() {
                keep = false;
            }
        });
    });
    if keep && !esc && app.dialogs.tun_hint.is_none() {
        app.dialogs.tun_hint = Some((exe, hint_text));
    }
}
