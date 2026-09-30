#!/usr/bin/env bash
# Собирает RustBox для Windows x64 в dist/RustBox-windows.zip:
#
#   RustBox-windows/
#     rustbox.exe, rustbox-cli.exe, README.txt
#     cores/xray.exe, cores/sing-box.exe, cores/geoip.dat, cores/geosite.dat
#
# Нужно (Arch): sudo pacman -S mingw-w64-gcc
# Версии ядер: XRAY_VERSION=v26.3.27 SINGBOX_VERSION=1.14.2 scripts/package-windows.sh
set -euo pipefail
cd "$(dirname "$(readlink -f "$0")")/.."

TARGET=x86_64-pc-windows-gnu
XRAY_VERSION=${XRAY_VERSION:-v26.3.27}
SINGBOX_VERSION=${SINGBOX_VERSION:-1.14.2}

command -v x86_64-w64-mingw32-gcc >/dev/null \
  || { echo "Нужен mingw-w64-gcc: sudo pacman -S mingw-w64-gcc" >&2; exit 1; }
rustup target list --installed | grep -qx "$TARGET" || rustup target add "$TARGET"

echo "Сборка rustbox.exe и rustbox-cli.exe"
cargo build --release --target "$TARGET" -p rustbox-gui -p rustbox-cli

out=dist/RustBox-windows
rm -rf "$out"
mkdir -p "$out/cores"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
cp "target/$TARGET/release/rustbox.exe" "target/$TARGET/release/rustbox-cli.exe" "$out/"

echo "Xray $XRAY_VERSION"
base=https://github.com/XTLS/Xray-core/releases/download/$XRAY_VERSION
curl -fsSL -o "$tmp/xray.zip" "$base/Xray-windows-64.zip"
want=$(curl -fsSL "$base/Xray-windows-64.zip.dgst" | awk -F'= *' '/^SHA2-256/ { print $2 }')
got=$(sha256sum "$tmp/xray.zip" | awk '{ print $1 }')
[[ -n $want && $want == "$got" ]] || { echo "Контрольная сумма Xray не совпала" >&2; exit 1; }
unzip -q -o "$tmp/xray.zip" xray.exe geoip.dat geosite.dat -d "$out/cores"

echo "sing-box $SINGBOX_VERSION"
# sing-box не публикует контрольные суммы; качаем только с официального релиза по HTTPS.
sb=sing-box-$SINGBOX_VERSION-windows-amd64
curl -fsSL -o "$tmp/sb.zip" \
  "https://github.com/SagerNet/sing-box/releases/download/v$SINGBOX_VERSION/$sb.zip"
unzip -q -o -j "$tmp/sb.zip" "$sb/sing-box.exe" -d "$out/cores"

cp crates/gui/assets/fonts/AdwaitaSans-LICENSE.txt "$out/"
cat > "$out/README.txt" <<'EOF'
RustBox для Windows

Запуск: rustbox.exe. Ничего устанавливать не нужно, папку можно положить куда угодно
(не перемещайте из неё папку cores - там ядра Xray и sing-box).

Windows может показать "Windows защитила ваш компьютер": нажмите
"Подробнее" -> "Выполнить в любом случае". Программа не подписана сертификатом.

Режимы:
  Прокси            - только локальный порт 127.0.0.1:2080 (HTTP и SOCKS5)
  Системный прокси  - браузеры и большинство программ пойдут через VPN сами
  TUN               - весь трафик системы. Нужен запуск от имени администратора:
                      правый клик по rustbox.exe -> "Запуск от имени администратора"

Настройки и профили: %APPDATA%\rustbox
EOF
# Сохраняем CRLF, чтобы файл нормально открылся в Блокноте
sed -i 's/$/\r/' "$out/README.txt"

# Не должен зависеть от DLL mingw, которых нет на чистой Windows
if command -v x86_64-w64-mingw32-objdump >/dev/null; then
  echo "Зависимости rustbox.exe от DLL:"
  x86_64-w64-mingw32-objdump -p "$out/rustbox.exe" | awk '/DLL Name/ { print "  " $3 }'
fi

(cd dist && rm -f RustBox-windows.zip && zip -qr RustBox-windows.zip RustBox-windows)
echo "Готово: dist/RustBox-windows.zip ($(du -h dist/RustBox-windows.zip | cut -f1))"
