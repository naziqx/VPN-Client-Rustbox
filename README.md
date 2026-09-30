# RustBox

Десктопный прокси/VPN-клиент на Rust в духе NekoBox и Happ: подписки, VLESS/Reality,
Hysteria2, TUIC и другие протоколы, ядра **sing-box** и **Xray**, режимы прокси,
системного прокси и TUN.

Работает на **Linux** (основная платформа, проверено на Arch). Поддержка **Windows**
написана, но пока не проверена на реальной системе (см. [Windows](#windows)).

![Главное окно](docs/screenshots/main-dark.png)

## Возможности

**Серверы и подписки**
- Share-ссылки: `vless://` (REALITY, Vision, gRPC, WS, XHTTP, HTTPUpgrade), `vmess://`, `trojan://`,
  `ss://` (SIP002, 2022, legacy, плагины), `hysteria2://`/`hy2://`, `tuic://`, `socks://`.
- Импорт по Ctrl+V, из текста или вручную (JSON-редактор); экспорт ссылок.
- Подписки: base64, обычный список, Clash/Clash.Meta YAML и **полные конфиги Xray в формате Happ**.
  В клиенте видны трафик и срок подписки (`subscription-userinfo`).
- Пиннинг сертификата (`pcs`) для серверов с самоподписанными сертификатами.

**Подключение**
- Ядра sing-box и Xray. Если выбранное ядро не поддерживает профиль, для этого
  профиля автоматически используется другое.
- Три режима:

  | Режим | Что идёт через VPN |
  |---|---|
  | **Прокси** | только программы, настроенные на `127.0.0.1:2080` (HTTP + SOCKS5) |
  | **Системный прокси** | браузеры и большинство программ, автоматически |
  | **TUN** | весь трафик системы, включая игры и программы без поддержки прокси |

- После подключения клиент сам проверяет, что интернет через сервер действительно работает.

**Проверка серверов**
- URL-тест (реальный запрос через сервер) и TCP ping. Выбор в списке рядом с кнопкой «Проверить».
- Результаты появляются по мере готовности, проверку можно остановить, таблица сортируется по задержке.

**Интерфейс**
- Тёмная и светлая тема, карточка подключения, переключатель режимов, группы и подписки
  в боковой панели, лог ядра по запросу.

### Подписки в формате Happ

Многие панели (Remnawave, Marzban и др.) отдают Happ полные конфиги Xray. «Авто»-серверы в них —
это балансировщик на несколько серверов с автоматическим выбором живого. Обычным клиентам те же
записи приходят ссылками, и каждая превращается в **один** запасной сервер, который часто не работает.

Если Xray установлен, RustBox по умолчанию запрашивает полные конфиги (User-Agent
`RustBox/… (Happ compatible)`) и запускает их целиком: с балансировкой и правилами провайдера.
Заменяются только локальные входы, а встроенный DNS Xray идёт тем же маршрутом, что и трафик
(в исходных конфигах он часто уходит в мёртвый запасной сервер, и сайты «напрямую» открываются
по 10 секунд). Если панель отдаёт непонятный ответ, клиент сам перезапрашивает обычные ссылки.

### Скриншоты

| Светлая тема | Первый запуск | Настройки |
|---|---|---|
| ![](docs/screenshots/main-light.png) | ![](docs/screenshots/empty-dark.png) | ![](docs/screenshots/settings-dark.png) |

## Установка

### Linux (Arch)

Ядра:

```sh
sudo pacman -S sing-box xray v2ray-geoip v2ray-domain-list-community
```

Готовая сборка есть в [Releases](../../releases): распакуйте архив и запустите `rustbox`.
Можно собрать и самому (см. [Сборка](#сборка)).

Для режима TUN ядру sing-box нужна capability `CAP_NET_ADMIN`. Клиент сам предложит выдать её
через pkexec, или вручную:

```sh
sudo setcap cap_net_admin,cap_net_bind_service=+ep /usr/bin/sing-box
```

Системный прокси настраивается для GNOME (gsettings) и KDE (kwriteconfig6).

### Windows

Архив `RustBox-windows.zip` содержит всё нужное: `rustbox.exe`, ядра Xray и sing-box, файлы geoip/geosite.
Распакуйте и запустите `rustbox.exe`.

- Программа не подписана, поэтому Windows может предупредить: «Подробнее» → «Выполнить в любом случае».
- Режим TUN требует запуска от имени администратора.
- Системный прокси включается через настройки WinINet. Прежние настройки сохраняются
  и восстанавливаются при отключении.

> Поддержка Windows пока не проверена на реальной системе. Если что-то не работает,
> создайте issue и приложите лог (кнопка «Лог» внизу окна).

## Быстрый старт

1. Скопируйте ссылку на подписку или сервер.
2. В клиенте нажмите **Ctrl+V** или «+ Добавить» → «Добавить подписку…».
3. Выберите сервер, при желании нажмите «Проверить».
4. Выберите режим и нажмите **«Подключить»** (или двойной клик по серверу / Enter).

Горячие клавиши: Ctrl+V — импорт, Enter — подключиться, Del — удалить, Ctrl+A — выделить все.

## Командная строка

```sh
rustbox-cli import 'vless://…' 'ss://…'      # ссылки, файл или "-" для stdin
rustbox-cli sub add "Мой VPN" https://…/sub
rustbox-cli sub update
rustbox-cli list
rustbox-cli core xray                          # выбрать ядро
rustbox-cli ping --url                         # URL-тест группы
rustbox-cli run 5 --system-proxy               # подключиться (Ctrl+C — отключиться)
rustbox-cli config 5                           # показать сгенерированный конфиг ядра
```

## Где хранятся данные

| | Linux | Windows |
|---|---|---|
| Настройки | `~/.config/rustbox/settings.json` | `%APPDATA%\rustbox\settings.json` |
| Профили | `~/.local/share/rustbox/profiles.json` | `%APPDATA%\rustbox\profiles.json` |
| Конфиги ядер | `$XDG_RUNTIME_DIR/rustbox/` | `%TEMP%\rustbox\` |

## Сборка

Нужен Rust 1.85+ (edition 2024).

```sh
cargo build --release
./target/release/rustbox          # GUI
./target/release/rustbox-cli --help
```

Сборка для Windows с Linux (архив с ядрами в `dist/RustBox-windows.zip`):

```sh
sudo pacman -S mingw-w64-gcc
scripts/package-windows.sh
```

## Архитектура

```
crates/
  platform/  всё, что зависит от ОС (трейт Platform)
             ├─ linux.rs        gsettings/KDE, getcap/setcap, поиск ядер
             ├─ windows.rs      реестр WinINet, права администратора для TUN
             └─ unsupported.rs  заглушка для остальных ОС
  core/      логика без привязки к ОС и интерфейсу
             ├─ model.rs        Profile / Protocol / Tls / Transport / Group
             ├─ link/           разбор и экспорт share-ссылок
             ├─ subscription.rs загрузка подписок, выбор User-Agent
             ├─ xray_json.rs    полные конфиги Xray (формат Happ)
             ├─ config/         генераторы конфигов sing-box и Xray
             ├─ process.rs      запуск ядер, логи, остановка
             ├─ latency.rs      TCP ping / URL-тест
             ├─ connection.rs   сессия: конфиг → проверка → запуск → системный прокси
             └─ storage.rs, settings.rs, tls_pin.rs
  cli/       rustbox-cli (clap)
  gui/       rustbox (egui/eframe): app.rs — состояние, view.rs — главное окно,
             dialogs.rs — диалоги, theme.rs — тема, tasks.rs — фоновые задачи
```

- **Новая ОС:** модуль в `crates/platform` с реализацией `Platform`, подключить в `platform::current()`.
- **Новое ядро:** вариант в `CoreKind` и модуль в `core/src/config/`.

## Тесты

```sh
cargo test --workspace
scripts/screenshots.sh        # скриншоты окна без экрана → target/screenshots/
```

Если sing-box и Xray установлены, сгенерированные конфиги проверяются самими ядрами.

## Планы

- Проверка и доводка Windows, перезапуск с правами администратора для TUN
- Трей-иконка, автозапуск, автообновление подписок по таймеру
- Правила маршрутизации (geosite/geoip, выбор приложений)
- Статистика трафика и скорости
- macOS

## Лицензия

GPL-3.0-or-later. Шрифт Adwaita Sans (на основе Inter) — SIL Open Font License 1.1,
см. `crates/gui/assets/fonts/AdwaitaSans-LICENSE.txt`.
