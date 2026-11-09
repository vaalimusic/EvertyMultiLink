# Everty MultiLink

<p align="center">
  <strong>Современный мультиплатформенный клиент умной VPN-маршрутизации для Windows, Android и Android TV на базе Mihomo Core.</strong><br>
  <em>Modern cross-platform smart proxy &amp; VPN routing client for Windows, Android, and Android TV powered by Mihomo Core.</em>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/Version-1.1.2-6FE6B4?style=flat-square" alt="Version 1.1.2" />
  <img src="https://img.shields.io/badge/Language-Rust%20%7C%20Kotlin-DEA584?style=flat-square" alt="Rust &amp; Kotlin" />
  <img src="https://img.shields.io/badge/Platforms-Windows%2010%2F11%20%7C%20Android%20%7C%20Android%20TV-0078D6?style=flat-square" alt="Platforms" />
  <img src="https://img.shields.io/badge/UI-EvertyUI%20(Zero%20WebView)%20%7C%20Material-green?style=flat-square" alt="UI" />
  <img src="https://img.shields.io/badge/Core-Mihomo-blue?style=flat-square" alt="Mihomo" />
  <img src="https://img.shields.io/badge/License-MIT-yellow?style=flat-square" alt="License" />
</p>

<p align="center">
  <a href="#русский">Русский</a> • <a href="#english">English</a> • <a href="https://boosty.to/vaali/donate">Поддержать / Donate ❤️</a> • <a href="https://desk.everty.ru/opensource">Everty OpenSource</a>
</p>

---

<a name="русский"></a>
## 🇷🇺 Русский

### О проекте

**Everty MultiLink** — высокопроизводительный нативный клиент для Windows, Android и Android TV, реализующий интеллектуальную раздельную маршрутизацию (Split Tunneling), управление пулами VLESS/Reality-серверов и мгновенную синхронизацию между устройствами через QR-коды и локальную сеть Wi-Fi без облачных посредников.

- **Автор проекта:** [Артур Валиев (Arthur Valiev)](https://desk.everty.ru/opensource)
- **Лицензия:** [MIT License](LICENSE)
- **Поддержать разработку:** [Boosty Donate](https://boosty.to/vaali/donate)
- **Экосистема проектов:** [https://desk.everty.ru/opensource](https://desk.everty.ru/opensource)

---

### Ключевые возможности

1. **⚡ Нулевой оверхед (Zero WebView):**
   - **Windows:** Написан на чистом Rust с собственной библиотекой отрисовки **EvertyUI** (< 25 МБ ОЗУ, мгновенный старт, без Electron/Chromium).
   - **Android & TV:** Нативный стек на Kotlin и Material Design с полной поддержкой управления с пульта Android TV (D-pad), темной темой и быстрым плиточным переключателем в шторке (Quick Settings Tile).
2. **🔄 Умная раздельная маршрутизация (Split Tunneling):**
   - Прямой доступ для российских сайтов: банки, Госуслуги, .ru, .рф, Кинопоиск, Яндекс работают на максимальной скорости вашего провайдера в обход VPN.
   - Пользовательские правила для доменов и IP (`DIRECT` или `PROXY`).
   - Разделение по приложениям (Per-App VPN) на Android.
3. **📲 Синхронизация между устройствами (QR & Wi-Fi LAN Sync):**
   - Передавайте подписки и отдельные серверы между ПК, смартфоном и Android TV в один клик.
   - Сканируйте QR-код камерой смартфона или открывайте встроенную локальную веб-страницу в браузере в домашней сети Wi-Fi.
   - Сохранение состояния отключенных узлов при синхронизации.
4. **⏳ Обратный отсчёт срока подписки:**
   - Автоматический разбор заголовка `subscription-userinfo` и отображение бейджа с количеством оставшихся дней.
5. **🎯 Гибкие политики балансировки:**
   - `Smart` (Умный выбор с независимым тестированием маршрутов для ключевых сервисов).
   - `Balance` (Балансировка нагрузки).
   - `Latency` (Минимальный пинг / RTT).
   - `Stable` (Резервирование с отказоустойчивым переключением Fallback).
6. **🛡️ Двойной режим работы на Windows:**
   - **System Proxy:** работа без прав администратора через WinINet.
   - **TUN-адаптер:** перенаправление всего системного трафика через виртуальный адаптер Wintun.

---

### Загрузка и релизы

Все готовые сборки доступны на странице [GitHub Releases](https://github.com/vaalimusic/EvertyMultiLink/releases):

| Платформа | Формат | Описание |
| :--- | :--- | :--- |
| **Windows x64** | `EvertyMultiLink-1.1.2-windows-x64-setup.exe` | Автоматический установщик для Windows 10/11 |
| **Windows x64** | `EvertyMultiLink-1.1.2-windows-x64-portable.zip` | Портативная версия (не требует установки) |
| **Android (arm64)** | `EvertyMultiLink-1.1.2-arm64-v8a-release.apk` | Смартфоны и современные ТВ-приставки |
| **Android (armv7)** | `EvertyMultiLink-1.1.2-armeabi-v7a-release.apk` | 32-битные ТВ и старые смартфоны |
| **Android (x86_64)** | `EvertyMultiLink-1.1.2-x86_64-release.apk` | Эмуляторы Android и устройства Intel/AMD |
| **Android (Universal)**| `EvertyMultiLink-1.1.2-universal-release.apk` | Универсальный APK для любых Android-устройств |

---

### Сборка из исходников

#### Windows:
```powershell
# Сборка и запуск десктоп-версии
cargo run -p multilink-app

# Создание релизного установщика Inno Setup
.\installer\build.ps1
```

#### Android:
```powershell
cd android
./gradlew assembleRelease
```

---

<a name="english"></a>
## 🇬🇧 English

### About The Project

**Everty MultiLink** is a high-performance native proxy client for Windows, Android, and Android TV featuring smart split tunneling, VLESS/Reality server management, multi-link balancing, and zero-cloud local QR / Wi-Fi sync between devices.

- **Author:** [Arthur Valiev](https://desk.everty.ru/opensource)
- **License:** [MIT License](LICENSE)
- **Support the Author:** [Boosty Donate ❤️](https://boosty.to/vaali/donate)
- **Open-Source Ecosystem:** [https://desk.everty.ru/opensource](https://desk.everty.ru/opensource)

---

### Key Features

1. **⚡ Zero WebView Architecture:**
   - **Windows:** Pure Rust implementation utilizing lightweight custom **EvertyUI** canvas rasterizer (< 25 MB RAM, instant start, zero Electron/Chromium dependencies).
   - **Android & TV:** Native Kotlin + Material Design, full Android TV D-pad remote controller support, Quick Settings Tile, and persistent notification stability on Android 14/15.
2. **🔄 Smart Split Tunneling:**
   - Direct routing for national domains (.ru, state services, banking) with full provider throughput bypassing the proxy.
   - User-defined domain and IP-CIDR custom rules (`DIRECT` vs `PROXY`).
   - Per-App VPN filtering on Android.
3. **📲 Cross-Device QR & Wi-Fi Sync:**
   - Seamlessly transfer subscriptions, server nodes, and disabled item states between PC, Phone, and Android TV.
   - Scan QR code or connect via embedded LAN HTTP server over the same Wi-Fi network (no cloud or third-party servers required).
4. **⏳ Subscription Expiry Countdown:**
   - Real-time `subscription-userinfo` header parsing with an intuitive remaining days badge.
5. **🎯 Adaptive Routing Policies:**
   - `Smart` (Adaptive per-service multi-link route testing).
   - `Balance` (Load balancing).
   - `Latency` (Lowest round-trip time).
   - `Stable` (High-availability failover fallback).
6. **🛡️ Flexible Windows Modes:**
   - **System Proxy:** Runs without administrative rights via WinINet.
   - **TUN Adapter:** Full-system L3 IP packet routing via Wintun.

---

### Downloads & Releases

Pre-built binaries and APK packages are available on [GitHub Releases](https://github.com/vaalimusic/EvertyMultiLink/releases):

| Platform | Package | Description |
| :--- | :--- | :--- |
| **Windows x64** | `EvertyMultiLink-1.1.2-windows-x64-setup.exe` | Recommended installer for Windows 10/11 |
| **Windows x64** | `EvertyMultiLink-1.1.2-windows-x64-portable.zip` | Standalone portable archive |
| **Android (arm64)** | `EvertyMultiLink-1.1.2-arm64-v8a-release.apk` | Modern 64-bit phones and Android TV boxes |
| **Android (armv7)** | `EvertyMultiLink-1.1.2-armeabi-v7a-release.apk` | 32-bit legacy devices and smart TVs |
| **Android (x86_64)** | `EvertyMultiLink-1.1.2-x86_64-release.apk` | Android emulators & x86 hardware |
| **Android (Universal)**| `EvertyMultiLink-1.1.2-universal-release.apk` | Single all-in-one APK for all architectures |

---

### Building from Source

#### Windows Client:
```powershell
# Run desktop in development mode
cargo run -p multilink-app

# Build Inno Setup installer
.\installer\build.ps1
```

#### Android Application:
```powershell
cd android
./gradlew assembleRelease
```

---

## License & Support

- **Author:** [Arthur Valiev](https://desk.everty.ru/opensource)
- **License:** [MIT License](LICENSE) © 2026 Arthur Valiev.
- **Donation:** If you enjoy Everty MultiLink, consider supporting development at [Boosty Donate](https://boosty.to/vaali/donate) ❤️.
