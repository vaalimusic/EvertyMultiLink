"""Reproducible Mihomo fault tests and optional Android transport transitions."""
import argparse
from contextlib import ExitStack, contextmanager
from datetime import datetime, timezone
from http.client import HTTPConnection, IncompleteRead
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from ipaddress import ip_address
import random
import re
import json
from pathlib import Path
import socket
import subprocess
import tempfile
import threading
import time
import uuid
from urllib.request import ProxyHandler, build_opener

from check_network import SITES, adb_path, adb_shell, route_check, run_probe


class Skip(Exception):
    pass


class Suite:
    def __init__(self, log):
        self.log = log
        self.results = []

    def check(self, name, action):
        start = time.monotonic()
        result = {"id": len(self.results) + 1, "scenario": name,
                  "time": datetime.now(timezone.utc).isoformat()}
        try:
            result["evidence"] = action()
            result["outcome"] = "pass"
        except Skip as error:
            result.update(outcome="skip", evidence=str(error))
        except (AssertionError, OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
            result.update(outcome="fail", evidence=str(error) or type(error).__name__)
        result["ms"] = round((time.monotonic() - start) * 1000)
        self.results.append(result)
        self.log.write(json.dumps(result, ensure_ascii=False) + "\n")
        self.log.flush()
        print(f'{result["id"]:02} {result["outcome"]:4} {name}: {result["evidence"]}', flush=True)


def require(condition, evidence):
    if not condition:
        raise AssertionError(evidence)
    return evidence


def eventually(action, seconds=10):
    end = time.monotonic() + seconds
    while True:
        value = action()
        if value:
            return value
        if time.monotonic() >= end:
            raise AssertionError(f"Condition did not become true within {seconds}s")
        time.sleep(0.2)


@contextmanager
def endpoint(marker, port=0, fault=None):
    class Handler(BaseHTTPRequestHandler):
        def do_CONNECT(self):
            # The mock exit terminates the tunnel locally. It never dials
            # the hostname in the CONNECT request.
            if fault and fault() == "offline":
                return
            self.send_response(200, "Connection Established")
            self.end_headers()
            self.handle_one_request()

        def do_GET(self):
            path = self.path.rsplit("/", 1)[-1]
            code = {"forbidden": 403, "rate": 429, "outage": 503,
                    "missing": 404, "auth": 401, "empty": 204}.get(path, 200)
            if path == "redirect":
                self.send_response(302)
                self.send_header("Location", "/ok")
                self.end_headers()
                return
            if path == "slow":
                time.sleep(0.7)
            if fault:
                state = fault()
                if state == "offline":
                    return
                if state == "slow":
                    time.sleep(0.15)
                if path == "page" and self.command == "GET" and state == "truncated":
                    path = "truncated"
            body = b"" if code == 204 else marker.encode()
            if fault and path == "page" and state == "blockpage":
                body = b"<html><title>Access denied</title>This website is blocked</html>"
            if fault and path == "page" and state == "placeholder":
                body = b"<html><title>Welcome</title>Service temporarily unavailable</html>"
            try:
                self.send_response(code)
                self.send_header("Content-Length", str(len(body) + (8 if path == "truncated" else 0)))
                self.end_headers()
                if self.command != "HEAD":
                    self.wfile.write(body)
            except ConnectionError:
                pass

        do_HEAD = do_GET

        def log_message(self, *args):
            pass

    with ThreadingHTTPServer(("127.0.0.1", port), Handler) as server:
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            yield server.server_port
        finally:
            server.shutdown()
            thread.join()


def request(port, target, timeout=2):
    connection = HTTPConnection("127.0.0.1", port, timeout=timeout)
    try:
        connection.request("GET", target)
        response = connection.getresponse()
        return response.status, response.read().decode()
    finally:
        connection.close()


def expect_response(port, target, status, marker):
    actual = request(port, target)
    return require(actual == (status, marker), f"expected {(status, marker)}, got {actual}")


def expect_error(error_type, action):
    try:
        action()
    except error_type as error:
        return type(error).__name__
    raise AssertionError(f"Expected {error_type.__name__}")


def mixed_port():
    # Windows can exclude different ranges for TCP and UDP. Mihomo binds both.
    for port in random.sample(range(20000, 49000), 64):
        with socket.socket() as tcp, socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as udp:
            try:
                tcp.bind(("127.0.0.1", port))
                udp.bind(("127.0.0.1", port))
                return port
            except OSError:
                continue
    raise RuntimeError("No free TCP and UDP port for Mihomo")


def http_cases(suite):
    with endpoint("origin") as port:
        for path, status in (("ok", 200), ("empty", 204), ("auth", 401), ("forbidden", 403),
                             ("missing", 404), ("rate", 429), ("outage", 503)):
            suite.check(f"HTTP {status} returned by a controlled endpoint",
                        lambda p=path, s=status: expect_response(port, "/" + p, s, "" if s == 204 else "origin"))
        suite.check("HTTP redirect completes", lambda: require(
            build_opener(ProxyHandler({})).open(f"http://127.0.0.1:{port}/redirect", timeout=2)
            .read() == b"origin", "302 resolved to /ok"))
        suite.check("Slow response times out", lambda: expect_error(
            TimeoutError, lambda: request(port, "/slow", timeout=0.1)))
        suite.check("Truncated response is rejected", lambda: expect_error(
            IncompleteRead, lambda: request(port, "/truncated")))


@contextmanager
def core(binary, rules, proxies, groups=(), hosts=None, dns=None, listeners=(), controller=None):
    with tempfile.TemporaryDirectory(prefix="everty-resilience-") as folder:
        port = mixed_port()
        config = {"mixed-port": port, "allow-lan": False, "bind-address": "127.0.0.1",
                  "mode": "rule", "log-level": "warning", "rules": rules,
                  "proxies": proxies, "proxy-groups": list(groups), "hosts": hosts or {}}
        if listeners:
            config["listeners"] = list(listeners)
        if controller:
            api_port, secret = controller
            config.update({"external-controller": f"127.0.0.1:{api_port}", "secret": secret})
        config["rule-providers"] = {
            "RU": {"type": "file", "behavior": "domain", "format": "mrs",
                   "path": "./category-ru.mrs"}
        }
        if dns is not None:
            config["dns"] = dns
        source = Path(__file__).resolve().parents[1] / "assets/category-ru.mrs"
        (Path(folder) / "category-ru.mrs").write_bytes(source.read_bytes())
        path = Path(folder) / "config.yaml"
        path.write_text(json.dumps(config), encoding="utf-8")
        with (Path(folder) / "startup.log").open("wb") as startup:
            process = subprocess.Popen([str(binary), "-d", folder, "-f", str(path)],
                                       stdout=startup, stderr=subprocess.STDOUT)
        try:
            def listening():
                if process.poll() is not None:
                    raise RuntimeError(f"Mihomo exited with code {process.returncode}")
                try:
                    with socket.create_connection(("127.0.0.1", port), timeout=0.2):
                        return True
                except OSError:
                    return False
            try:
                eventually(listening, 8)
            except (AssertionError, RuntimeError) as error:
                detail = (Path(folder) / "startup.log").read_text(encoding="utf-8", errors="replace")
                raise RuntimeError(f"{error}; core output: {detail[-500:]}") from error
            yield port
        finally:
            if process.poll() is None:
                process.terminate()
            process.wait(timeout=5)


def proxy(name, port):
    return {"name": name, "type": "http", "server": "127.0.0.1", "port": port}


def core_cases(suite, binary):
    with ExitStack() as stack:
        local = stack.enter_context(endpoint("direct"))
        good = stack.enter_context(endpoint("primary"))
        backup = stack.enter_context(endpoint("backup"))
        reserved = stack.enter_context(socket.socket())
        reserved.bind(("127.0.0.1", 0))
        dead = reserved.getsockname()[1]
        proxies = [proxy("PRIMARY", good), proxy("BACKUP", backup), proxy("DEAD", dead)]
        base = ["DOMAIN,override.ru,PRIMARY", "DOMAIN,private.test,DIRECT",
                "DOMAIN-SUFFIX,ru,DIRECT", "DOMAIN-SUFFIX,xn--p1ai,DIRECT",
                "DOMAIN-SUFFIX,su,DIRECT", "RULE-SET,RU,DIRECT",
                "IP-CIDR,127.0.0.1/32,DIRECT,no-resolve", "MATCH,PRIMARY"]
        hosts = {host: "127.0.0.1" for host in
                 ("ya.ru", "www.gosuslugi.ru", "vk.com", "test.xn--p1ai", "test.su", "private.test")}
        for host, exit_name in (
            ("ya.ru", "direct"), ("www.gosuslugi.ru", "direct"), ("vk.com", "direct"),
            ("test.xn--p1ai", "direct"), ("test.su", "direct"), ("private.test", "direct"),
            ("override.ru", "primary"), ("fake-ru.invalid", "primary"),
            ("www.youtube.com", "primary"), ("www.facebook.com", "primary"),
            ("127.0.0.1", "direct"),
        ):
            def route(host=host, exit_name=exit_name):
                with core(binary, base, proxies, hosts=hosts) as port:
                    return expect_response(port, f"http://{host}:{local}/ok", 200, exit_name)
            suite.check(f"Route {host} to {exit_name}", route)

        def through(target, path, status, marker, group=None, selected_proxies=None):
            with core(binary, ["MATCH," + target], selected_proxies or proxies,
                      [group] if group else []) as port:
                if group:
                    # Wait for the configured health check, not just the listener.
                    return eventually(lambda: observed(port, path, status, marker), 6)
                return expect_response(port, f"http://route.invalid/{path}", status, marker)

        def observed(port, path, status, marker):
            try:
                result = request(port, f"http://route.invalid/{path}", timeout=1)
            except OSError:
                return None
            return str(result) if result == (status, marker) else None

        def group(kind, members, path="empty"):
            return {"name": "POOL", "type": kind, "proxies": members, "interval": 1,
                    "timeout": 500, "lazy": False, "url": f"http://health.invalid/{path}",
                    "expected-status": "204"}

        def unavailable(target, configuration=None):
            with core(binary, ["MATCH," + target], proxies, [configuration] if configuration else [],
                      {"route.invalid": "127.0.0.1"}) as port:
                try:
                    status, marker = request(port, f"http://route.invalid:{local}/ok")
                except OSError as error:
                    return type(error).__name__
                return require(status >= 400 and marker != "direct", f"HTTP {status}, {marker[:40]}")

        suite.check("Pinned live server", lambda: through("PRIMARY", "ok", 200, "primary"))
        suite.check("Pinned refused server cannot leak to direct", lambda: unavailable("DEAD"))
        suite.check("Dead first server falls back to backup", lambda: through(
            "POOL", "ok", 200, "backup", group("fallback", ["DEAD", "BACKUP"])))
        suite.check("All fallback servers down cannot leak to direct", lambda: unavailable(
            "POOL", group("fallback", ["DEAD"])))
        suite.check("Latency policy avoids a refused server", lambda: through(
            "POOL", "ok", 200, "backup", group("url-test", ["DEAD", "BACKUP"])))
        suite.check("Balance policy avoids a refused server", lambda: through(
            "POOL", "ok", 200, "backup", group("load-balance", ["DEAD", "BACKUP"])))
        suite.check("Fallback keeps first healthy server", lambda: through(
            "POOL", "ok", 200, "primary", group("fallback", ["PRIMARY", "BACKUP"])))
        suite.check("Upstream HTTP 403 is reported", lambda: through("PRIMARY", "forbidden", 403, "primary"))
        suite.check("Upstream HTTP 429 is reported", lambda: through("BACKUP", "rate", 429, "backup"))
        suite.check("Upstream HTTP 503 is reported", lambda: through("PRIMARY", "outage", 503, "primary"))
        def recover():
            with core(binary, ["MATCH,POOL"], proxies,
                      [group("fallback", ["DEAD", "BACKUP"])]) as port:
                eventually(lambda: observed(port, "ok", 200, "backup"), 6)
                reserved.close()
                # Bring the original endpoint up on the same port while
                # the same Mihomo process and health checker are running.
                with endpoint("recovered", dead):
                    return eventually(lambda: observed(port, "ok", 200, "recovered"), 8)
        suite.check("Fallback returns to recovered server in the same session", recover)


def dns_cases(suite, binary):
    import struct

    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as upstream:
        upstream.bind(("127.0.0.1", 0))
        upstream.settimeout(0.1)
        stop = threading.Event()
        names = {"known.test": (0, "192.0.2.11"), "missing.test": (3, None),
                 "broken.test": (2, None), "dropped.test": (None, None),
                 "changed.test": (0, "192.0.2.12")}

        def serve():
            while not stop.is_set():
                try:
                    data, peer = upstream.recvfrom(2048)
                except TimeoutError:
                    continue
                qend = data.index(b"\0", 12)
                # The fixture uses two labels, with DNS label lengths on wire.
                first = data[12]
                name = data[13:13 + first].decode() + "." + data[14 + first:qend].decode()
                code, address = names.get(name, (3, None))
                if code is None:
                    continue
                answer = b"" if not address else b"\xc0\x0c" + struct.pack(
                    "!HHIH", 1, 1, 0, 4) + socket.inet_aton(address)
                header = data[:2] + struct.pack("!HHHHH", 0x8180 | code, 1, bool(answer), 0, 0)
                upstream.sendto(header + data[12:qend + 5] + answer, peer)

        thread = threading.Thread(target=serve, daemon=True)
        thread.start()

        def query(name, expected, address=None):
            with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as reserved:
                reserved.bind(("127.0.0.1", 0))
                dns_port = reserved.getsockname()[1]
            config = {"enable": True, "listen": f"127.0.0.1:{dns_port}",
                      "nameserver": [f"udp://127.0.0.1:{upstream.getsockname()[1]}"],
                      "enhanced-mode": "redir-host"}
            with core(binary, ["MATCH,DIRECT"], [], dns=config):
                wire = b"".join(bytes([len(label)]) + label.encode() for label in name.split("."))
                packet = struct.pack("!HHHHHH", 1, 0x100, 1, 0, 0, 0) + wire + b"\0\0\1\0\1"
                with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as client:
                    client.settimeout(1.2)
                    client.sendto(packet, ("127.0.0.1", dns_port))
                    if expected is None:
                        return expect_error(TimeoutError, lambda: client.recvfrom(2048))
                    reply = client.recvfrom(2048)[0]
                    code = reply[3] & 15
                    return require(code == expected and (not address or reply[-4:] == socket.inet_aton(address)),
                                   f"DNS response code {code}, expected {expected}, address {address}")
        try:
            suite.check("DNS returns an A record", lambda: query("known.test", 0, "192.0.2.11"))
            suite.check("DNS NXDOMAIN remains NXDOMAIN", lambda: query("missing.test", 3))
            suite.check("DNS upstream SERVFAIL propagates", lambda: query("broken.test", 2))
            suite.check("Unresponsive DNS is bounded", lambda: query("dropped.test", None))
            suite.check("DNS resolves another host after previous outage", lambda: query("changed.test", 0, "192.0.2.12"))
        finally:
            stop.set()
            thread.join()


def phone_cases(suite, adb, gateway, transitions, try_cellular, vpn_transitions):
    from concurrent.futures import ThreadPoolExecutor

    baseline_device = route_check(adb, "1.1.1.1") if adb else None
    if adb:
        with ThreadPoolExecutor(5) as pool:
            baseline = dict(zip(SITES, pool.map(
                lambda entry: run_probe(adb, *entry, "phone", "auto", 1), SITES.items())))
    else:
        baseline = {}

    def probe(name):
        if not adb:
            raise Skip("ADB phone required")
        result = baseline[name]
        result = dict(result, interface=baseline_device)
        if name in ("youtube", "facebook") and result["outcome"] != "ok" and (
            not baseline_device or not baseline_device.startswith("tun")
        ):
            raise Skip(f"Direct endpoint unavailable: {result['outcome']}")
        return require(result["outcome"] == "ok", result)

    for name in SITES:
        suite.check(f"Phone access to {name} on the current route", lambda name=name: probe(name))

    class Phone:
        def shell(self, *args):
            if not adb:
                raise Skip("ADB phone required")
            result = adb_shell(adb, list(args))
            if result.returncode:
                raise RuntimeError(f"ADB command {args[0]} exited {result.returncode}")
            return result.stdout.strip()

        def setting(self, name):
            value = self.shell("settings", "get", "global", name)
            if value not in ("0", "1"):
                raise Skip(f"Cannot verify original setting {name}")
            return value

        def mobile_settings(self):
            settings = {}
            for line in self.shell("settings", "list", "global").splitlines():
                key, _, value = line.partition("=")
                if key.startswith("mobile_data") and key.removeprefix("mobile_data").isdigit():
                    settings[key] = value
            if not settings:
                settings["mobile_data"] = self.setting("mobile_data")
            if any(value not in ("0", "1") for value in settings.values()):
                raise Skip("Cannot verify the original per-SIM mobile data settings")
            return settings

        def switch(self, name, enabled):
            self.shell("svc", name, "enable" if enabled else "disable")
            expected = str(int(enabled))
            if name == "wifi":
                eventually(lambda: self.setting("wifi_on") == expected, 8)
            else:
                eventually(lambda: all(value == expected for value in self.mobile_settings().values()), 8)

        def http(self):
            result = run_probe(adb, "yandex", SITES["yandex"], "phone", "ipv4", 1)
            return result["outcome"] == "ok"

        def wifi(self):
            routes = self.shell("ip", "-4", "route", "show", "table", "all")
            return any(line.startswith("default ") and " dev wlan" in line for line in routes.splitlines())

        def mobile(self):
            routes = self.shell("ip", "-4", "route", "show", "table", "all")
            return any(line.startswith("default ") and any(f" dev {name}" in line
                       for name in ("rmnet", "ccmni", "pdp", "wwan")) for line in routes.splitlines())

    phone = Phone()
    suite.check("LAN gateway has a Wi-Fi route", lambda: require(
        bool(phone.shell("ip", "route", "get", gateway).find(" dev wlan") >= 0),
        "gateway uses Wi-Fi"))
    suite.check("LAN gateway responds to ICMP", lambda: require(
        "1 received" in phone.shell("ping", "-c", "1", "-W", "2", gateway), "gateway answered"))
    def verify_active_route():
        public = phone.shell("ip", "route", "get", "1.1.1.1")
        if " dev tun" not in public:
            return "Public traffic uses the ordinary Android route"
        return require(" dev tun" in phone.shell("ip", "route", "get", "198.19.255.53"),
                       "Public traffic and VPN DNS both use TUN")
    suite.check("Public route and VPN DNS agree", verify_active_route)
    suite.check("SIM is available for cellular tests", lambda: require(
        any(state in ("READY", "LOADED") for state in phone.shell("getprop", "gsm.sim.state").split(",")),
        "SIM ready"))
    suite.check("Wi-Fi default route is present", lambda: require(phone.wifi(), "Wi-Fi default route"))

    def transition_cases():
        if not adb or not transitions:
            raise Skip("Requires --transitions and a USB-connected Android phone")
        device = route_check(adb, "1.1.1.1")
        if device is None or device.startswith("tun"):
            raise Skip("Disconnect VPN first so transport switches can be measured independently")
        check = subprocess.run([adb[0], "-d", "get-serialno"], capture_output=True, text=True, timeout=5)
        selected = subprocess.run([*adb, "get-serialno"], capture_output=True, text=True, timeout=5)
        if check.returncode or selected.returncode or check.stdout.strip() != selected.stdout.strip():
            raise Skip("Use USB ADB so Wi-Fi can be switched safely")
        wifi = phone.setting("wifi_on")
        data = phone.mobile_settings()
        if len(set(data.values())) != 1:
            raise Skip("SIM slots have different data settings; the test cannot restore them with svc")
        if wifi != "1" or not phone.wifi():
            raise Skip("Start with Wi-Fi connected")
        return wifi, data

    def wifi_only():
        phone.switch("data", False)
        eventually(phone.wifi, 20)
        eventually(phone.http, 15)
        return "HTTPS succeeded over Wi-Fi with mobile data disabled"

    def offline():
        phone.switch("wifi", False)
        eventually(lambda: not phone.wifi(), 12)
        return require(not phone.mobile() and not phone.http(), "no cellular fallback and no HTTP")

    def restore_wifi():
        phone.switch("wifi", True)
        eventually(phone.wifi, 20)
        eventually(phone.http, 15)
        return "Wi-Fi route and HTTPS recovered"

    def to_cellular():
        phone.switch("data", True)
        phone.switch("wifi", False)
        eventually(phone.mobile, 25)
        return "cellular default route appeared with Wi-Fi disabled"

    def hardware_cases(data):
        suite.check("Mobile disabled while Wi-Fi continues serving requests", wifi_only)
        suite.check("Loss of Wi-Fi and mobile data produces offline result", offline)
        suite.check("Wi-Fi returns after total outage", restore_wifi)
        if "1" not in data.values() and not try_cellular:
            reason = "Mobile data was disabled before test; enable it to test live cellular"
            for name in ("Wi-Fi to cellular handover", "Cellular-only HTTPS access",
                         "Cellular to Wi-Fi handover", "Disable cellular without losing Wi-Fi"):
                suite.check(name, lambda: unavailable(reason))
            return
        suite.check("Wi-Fi to cellular handover", to_cellular)
        def cellular_http():
            if phone.wifi() or not phone.mobile():
                raise Skip("Cellular handover did not establish a mobile-only route")
            return require(phone.http(), "HTTPS succeeds with only cellular")
        suite.check("Cellular-only HTTPS access", cellular_http)
        suite.check("Wi-Fi and HTTPS return after the cellular-only attempt", restore_wifi)
        suite.check("Disable cellular without losing Wi-Fi", wifi_only)

    def unavailable(reason):
        raise Skip(reason)

    @contextmanager
    def vpn_session():
        package = "ru.everty.multilink.debug"
        runner = f"{package}.test/ru.everty.multilink.NetworkSmokeRunner"
        token = uuid.uuid4().hex
        process = subprocess.Popen(
            [*adb, "shell", "am", "instrument", "-w", "-r", "-e", "token", token,
             "-e", "windowSeconds", "180", runner],
            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
            encoding="utf-8", errors="replace")
        try:
            def ready():
                if process.poll() is not None:
                    output = process.communicate()[0]
                    message = next((line.split("multilink=", 1)[1] for line in output.splitlines()
                                    if "multilink=" in line), "Install the debug test APK")
                    raise RuntimeError(message)
                device = route_check(adb, "198.19.255.53")
                return device and device.startswith("tun")
            eventually(ready, 25)
            yield
        finally:
            if process.poll() is None:
                adb_shell(adb, ["run-as", package, "touch", f"files/network-smoke-{token}.stop"])
            try:
                output = process.communicate(timeout=12)[0]
            except subprocess.TimeoutExpired:
                process.kill()
                process.communicate()
                raise RuntimeError("VPN test session did not acknowledge shutdown")
            if "INSTRUMENTATION_CODE: -1" not in output:
                raise RuntimeError("VPN test session did not finish cleanly")

    def vpn_hardware_cases(data):
        def tunneled():
            device = route_check(adb, "198.19.255.53")
            return device and device.startswith("tun")

        def ensure_vpn_http():
            from concurrent.futures import ThreadPoolExecutor

            def check(name):
                started = time.monotonic()
                result = None
                while time.monotonic() - started < 20:
                    if tunneled():
                        result = run_probe(adb, name, SITES[name], "phone", "ipv4", 1)
                        if result["outcome"] == "ok":
                            break
                    time.sleep(0.2)
                return {"recovery_ms": round((time.monotonic() - started) * 1000), "probe": result}

            with ThreadPoolExecutor(2) as pool:
                direct, proxied = pool.map(check, ("yandex", "youtube"))
            logs = adb_shell(adb, ["logcat", "-d", "-T", probe_since, "-s", "MihomoCore:D"])
            for row, host in ((direct, "ya.ru"), (proxied, "www.youtube.com")):
                pattern = rf"--> {re.escape(host)}:\d+ match .*? using ([\w\[\]-]+)"
                row["exits"] = sorted(set(re.findall(pattern, logs.stdout)))
            passed = all((row["probe"] or {}).get("outcome") == "ok" for row in (direct, proxied))
            passed &= direct["exits"] == ["DIRECT"] and bool(proxied["exits"])
            passed &= "DIRECT" not in proxied["exits"]
            return require(passed, {"direct": direct, "proxied": proxied})

        def vpn_wifi_only():
            phone.switch("data", False)
            return ensure_vpn_http()

        def vpn_offline():
            phone.switch("wifi", False)
            eventually(lambda: not phone.wifi(), 12)
            return require(tunneled() and not phone.mobile() and not phone.http(),
                           "TUN retained, both uplinks unavailable, HTTPS failed as expected")

        def vpn_wifi_recovery():
            phone.switch("wifi", True)
            eventually(phone.wifi, 20)
            return ensure_vpn_http()

        def vpn_cellular():
            if "1" not in data.values() and not try_cellular:
                raise Skip("Use --try-cellular to enable mobile data temporarily")
            to_cellular()
            return ensure_vpn_http()

        cases = (
            ("VPN stays usable on Wi-Fi with cellular disabled", vpn_wifi_only),
            ("VPN retains TUN during total uplink outage", vpn_offline),
            ("VPN recovers over Wi-Fi without reconnecting", vpn_wifi_recovery),
            ("VPN stays usable after Wi-Fi to cellular handover", vpn_cellular),
            ("VPN stays usable after restoring Wi-Fi", vpn_wifi_recovery),
        )
        completed = 0
        probe_since = ""
        try:
            with vpn_session():
                for name, action in cases:
                    probe_since = adb_shell(adb, ["date", "+%m-%d %H:%M:%S.000"]).stdout.strip()
                    suite.check(name, action)
                    completed += 1
        except (RuntimeError, AssertionError, OSError, subprocess.SubprocessError) as error:
            for name, _ in cases[completed:]:
                suite.check(name, lambda: unavailable(str(error)))
            suite.check("VPN test session lifecycle", lambda: require(False, str(error)))
        else:
            suite.check("VPN test session lifecycle", lambda: require(
                route_check(adb, "1.1.1.1") == baseline_device, "VPN stopped and original route returned"))

    try:
        wifi, data = transition_cases()
    except (Skip, OSError, RuntimeError, subprocess.SubprocessError) as error:
        reason = str(error)
        for name in ("Mobile disabled while Wi-Fi continues serving requests",
                     "Loss of Wi-Fi and mobile data produces offline result",
                     "Wi-Fi returns after total outage", "Wi-Fi to cellular handover",
                     "Cellular-only HTTPS access", "Wi-Fi and HTTPS return after the cellular-only attempt",
                     "Disable cellular without losing Wi-Fi", "Restore original Wi-Fi and cellular settings"):
            suite.check(name, lambda: unavailable(reason))
    else:
        def restore():
            phone.switch("wifi", wifi == "1")
            phone.switch("data", "1" in data.values())
            eventually(lambda: phone.setting("wifi_on") == wifi and phone.mobile_settings() == data, 20)
            return "original Wi-Fi and mobile settings restored"
        try:
            hardware_cases(data)
            if vpn_transitions:
                restore()
                vpn_hardware_cases(data)
        finally:
            suite.check("Restore original Wi-Fi and cellular settings", restore)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    default_binary = Path(__file__).resolve().parents[1] / "vendor/mihomo/mihomo.exe"
    parser.add_argument("--core", type=Path, default=default_binary)
    parser.add_argument("--log", type=Path, required=True)
    parser.add_argument("--phone", action="store_true")
    parser.add_argument("--serial")
    parser.add_argument("--gateway", type=ip_address, default=ip_address("192.168.0.1"))
    parser.add_argument("--transitions", action="store_true", help="Temporarily switch radios over USB")
    parser.add_argument("--try-cellular", action="store_true",
                        help="Try enabling mobile data even if it was originally disabled")
    parser.add_argument("--vpn-transitions", action="store_true",
                        help="Also exercise outages and handover inside an Android debug VPN session")
    parser.add_argument("--chaos", type=int, default=0, metavar="ROUNDS",
                        help="Run seeded node failures and compare full-body ranking with normal health checks")
    parser.add_argument("--seed", type=int, default=20261113)
    parser.add_argument("--chaos-only", action="store_true")
    parser.add_argument("--phone-only", action="store_true", help="Skip local HTTP, DNS and core cases")
    options = parser.parse_args()
    if options.transitions and not options.phone:
        parser.error("--transitions requires --phone")
    if options.phone_only and (not options.phone or options.chaos_only):
        parser.error("--phone-only requires --phone and cannot be used with --chaos-only")
    if options.chaos < 0 or (options.chaos_only and not options.chaos):
        parser.error("--chaos-only requires a positive --chaos count")
    if (options.try_cellular or options.vpn_transitions) and not options.transitions:
        parser.error("--try-cellular and --vpn-transitions require --transitions")
    if (not options.phone_only or options.chaos) and not options.core.is_file():
        parser.error("Mihomo executable not found; pass --core")
    adb = None
    if options.phone:
        adb = [adb_path(), *(["-s", options.serial] if options.serial else ["-d"])]
        if adb_shell(adb, ["curl", "--version"]).returncode:
            parser.error("Connect an authorized USB phone with curl")
    options.log.parent.mkdir(parents=True, exist_ok=True)
    with options.log.open("w", encoding="utf-8") as log:
        suite = Suite(log)
        if options.chaos:
            from check_node_chaos import chaos_cases
            chaos_cases(suite, options.core.resolve(), options.chaos, options.seed)
        if not (options.chaos_only or options.phone_only):
            http_cases(suite)
            core_cases(suite, options.core.resolve())
            dns_cases(suite, options.core.resolve())
        if not options.chaos_only:
            phone_cases(suite, adb, str(options.gateway), options.transitions, options.try_cellular,
                        options.vpn_transitions)
        counts = {status: sum(result["outcome"] == status for result in suite.results)
                  for status in ("pass", "fail", "skip")}
        print(f'{len(suite.results)} scenarios: {counts}', flush=True)
        return int(counts["fail"] > 0)


if __name__ == "__main__":
    raise SystemExit(main())
