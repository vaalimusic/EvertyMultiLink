"""Run a read-only network matrix on an Android phone over ADB."""
import argparse
import concurrent.futures
from datetime import datetime, timezone
import ipaddress
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import subprocess
import sys
import time
import uuid
from urllib.parse import urlsplit
from site_verification import PROFILES, verify_site


SITES = {
    "yandex": "https://ya.ru/",
    "gosuslugi": "https://www.gosuslugi.ru/",
    "vk": "https://vk.com/",
    "youtube": "https://www.youtube.com/",
    "facebook": "https://www.facebook.com/",
}


def adb_path():
    sdk = os.environ.get("ANDROID_HOME") or os.environ.get("ANDROID_SDK_ROOT")
    if not sdk and os.name == "nt":
        sdk = str(Path(os.environ.get("LOCALAPPDATA", "")) / "Android" / "Sdk")
    fallback = Path(sdk or "") / "platform-tools" / ("adb.exe" if os.name == "nt" else "adb")
    return shutil.which("adb") or (str(fallback) if fallback.is_file() else "adb")


def adb_shell(adb, arguments, timeout=20):
    return subprocess.run(
        [*adb, "shell", shlex.join(arguments)], capture_output=True, text=True,
        encoding="utf-8", errors="replace", timeout=timeout,
    )


def run_probe(adb, name, url, mode, family, attempt):
    inspect_body = name in PROFILES
    metadata = "%{http_code} %{time_namelookup} %{time_connect} %{time_appconnect} %{time_total}"
    args = [
        "curl", "-s", "-o", "-" if inspect_body else "/dev/null",
        "--connect-timeout", "6", "--max-time", "12",
        "-w", "%{stderr}" + metadata + "\\n%{url_effective}\\n%{content_type}\\n" if inspect_body else metadata,
    ]
    args += ["--noproxy", "*"] if mode == "phone" else [
        "--noproxy", "", "--proxy", "http://127.0.0.1:7890"
    ]
    if family == "ipv4":
        args.append("-4")
    if url.startswith("https://"):
        args.extend(["-L", "--max-redirs", "5", "--proto-redir", "=https"])
    result = {"site": name, "host": urlsplit(url).hostname, "mode": mode, "family": family,
              "attempt": attempt, "time": datetime.now(timezone.utc).isoformat()}
    try:
        command = shlex.join([*args, "--", url])
        if inspect_body:
            # Keep curl's exit status and bound what leaves the device. curl on
            # older Android cannot enforce a limit for chunked responses itself.
            command = f"set -o pipefail; {command} | head -c 2097153"
        probe = subprocess.run([*adb, "shell", command], capture_output=True,
                               timeout=20, encoding="utf-8", errors="replace")
    except subprocess.TimeoutExpired:
        return dict(result, outcome="adb_timeout")
    if inspect_body:
        timing, _, details = probe.stderr.partition("\n")
        effective, _, content_type = details.strip().partition("\n")
    else:
        timing = probe.stdout
    fields = timing.split()
    if len(fields) != 5 or not re.fullmatch(r"\d{3}", fields[0]):
        return dict(result, outcome="adb_error", exit_code=probe.returncode)
    status = int(fields[0])
    try:
        dns, tcp, tls, total = (round(float(value) * 1000) for value in fields[1:])
    except (ValueError, OverflowError):
        return dict(result, outcome="invalid_timings", exit_code=probe.returncode)
    if probe.returncode == 0:
        outcome = "ok" if 200 <= status < 400 else "http_error"
    elif probe.returncode in (5, 6):
        outcome = "dns_error"
    elif probe.returncode == 7:
        outcome = "connect_error"
    elif probe.returncode == 28:
        outcome = "response_timeout" if tls or (tcp and not url.startswith("https://")) else (
            "tls_timeout" if tcp else "dns_or_connect_timeout")
    elif probe.returncode in (35, 51, 60):
        outcome = "tls_error"
    else:
        outcome = "curl_error"
    if inspect_body:
        if probe.returncode == 23 or len(probe.stdout) >= 2097153:
            outcome = "unverified_content"
        if outcome == "ok":
            verdict = verify_site(name, url, effective, content_type, probe.stdout)
            outcome = "ok" if verdict == "verified_content" else verdict
        else:
            verdict = None
        try:
            result["effective_host"] = urlsplit(effective).hostname
        except ValueError:
            result["effective_host"] = None
        result["content_check"] = verdict
    elif outcome == "ok":
        result["content_check"] = "not_checked"
    return dict(result, outcome=outcome, status=status, exit_code=probe.returncode,
                dns_ms=dns, tcp_ms=tcp, tls_ms=tls, total_ms=total)


def lan_url(value):
    parsed = urlsplit(value)
    try:
        ip = ipaddress.ip_address(parsed.hostname or "")
        _ = parsed.port
    except ValueError as error:
        raise argparse.ArgumentTypeError("use an HTTP(S) URL with a literal LAN address") from error
    networks = ("10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16", "169.254.0.0/16", "fc00::/7")
    if (parsed.scheme not in ("http", "https") or parsed.username is not None
            or parsed.password is not None or parsed.query or parsed.fragment
            or not any(ip in ipaddress.ip_network(network) for network in networks)):
        raise argparse.ArgumentTypeError("use an HTTP(S) URL with a LAN address and no credentials")
    return value


def collect_matrix(adb, options, log, label, modes):
    sites = dict(SITES)
    sites.update({f"extra-{i}": url for i, url in enumerate(options.site, 1)})
    sites.update({f"lan-{i}": url for i, url in enumerate(options.lan, 1)})
    failures = 0
    with concurrent.futures.ThreadPoolExecutor(6) as pool:
        for attempt in range(1, options.repeat + 1):
            jobs = [
                pool.submit(run_probe, adb, name, url, mode, family, attempt)
                for mode in modes for name, url in sites.items()
                for family in (("auto", "ipv4") if mode == "phone" and not name.startswith("lan-") else ("auto",))
            ]
            for future in concurrent.futures.as_completed(jobs):
                result = dict(future.result(), label=label)
                failures += result["outcome"] != "ok"
                log.write(json.dumps(result, ensure_ascii=False) + "\n")
                log.flush()
                print(f'{result["mode"]:5} {result["family"]:4} {result["site"]:10} '
                      f'{result["outcome"]:24} HTTP {result.get("status", 0):03} '
                      f'{result.get("total_ms", 0)} ms')
    return failures


def route_check(adb, address):
    probe = adb_shell(adb, ["ip", "route", "get", address])
    device = re.search(r"\bdev\s+(\S+)", probe.stdout)
    return device.group(1) if probe.returncode == 0 and device else None


def write_route_matches(adb, since, label, log, urls, expected):
    # Only emit match type and exit for the tested hosts, not raw core logs.
    result = adb_shell(adb, ["logcat", "-d", "-T", since, "-s", "MihomoCore:D"])
    hosts = {urlsplit(url).hostname for url in urls}
    matches = set()
    pattern = r"--> ([a-zA-Z0-9.-]+):\d+ match (.*?) using ([A-Za-z0-9_\[\]-]+)"
    for host, rule, target in re.findall(pattern, result.stdout):
        if host in hosts:
            matches.add((host, rule, target))
    for host, rule, target in sorted(matches):
        log.write(json.dumps({"label": label, "check": "mihomo-match", "host": host,
                              "rule": rule, "target": target}) + "\n")
        print(f"match {host} {rule} -> {target}")
    dial_pattern = r"dial ([A-Za-z0-9_\[\]-]+) \(match ([^)]+)\).*? --> ([a-zA-Z0-9.-]+):\d+ error:"
    for target, rule, host in sorted(set(re.findall(dial_pattern, result.stdout))):
        if host in hosts:
            log.write(json.dumps({"label": label, "check": "mihomo-dial-error", "host": host,
                                  "rule": rule, "target": target}) + "\n")
            print(f"dial error {host} {rule} -> {target}")
    failures = 0
    for host, target in expected.items():
        observed = {exit_name.split("[", 1)[0] for name, _, exit_name in matches if name == host}
        passed = bool(observed) and (observed == {"DIRECT"} if target == "DIRECT" else "DIRECT" not in observed)
        failures += not passed
        log.write(json.dumps({"label": label, "check": "expected-match", "host": host,
                              "expected": target, "observed": sorted(observed), "passed": passed}) + "\n")
        print(f"expect {host} -> {target}: {'ok' if passed else 'FAIL'}")
    return failures


def compare(adb, options, log, policy, baseline_device):
    runner = "ru.everty.multilink.debug.test/ru.everty.multilink.NetworkSmokeRunner"
    label = options.label + "-on-" + (policy or "configured").lower()
    # The test APK owns the VPN session and disconnects at the end of its window.
    token = uuid.uuid4().hex
    probe_count = 3 * (len(SITES) + len(options.site)) + 2 * len(options.lan)
    window = options.repeat * ((probe_count + 5) // 6) * 15 + 15
    if window > 300:
        raise RuntimeError("Too many probes for one VPN session. Reduce repeats or extra URLs.")
    since = adb_shell(adb, ["date", "+%m-%d %H:%M:%S.000"]).stdout.strip()
    process = subprocess.Popen(
        [*adb, "shell", "am", "instrument", "-w", "-r", "-e", "windowSeconds",
         str(window), "-e", "token", token, *(["-e", "policy", policy] if policy else []), runner],
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
        encoding="utf-8", errors="replace",
    )
    try:
        deadline = time.monotonic() + 25
        while time.monotonic() < deadline:
            if process.poll() is not None:
                output = process.communicate()[0]
                error = next((line.split("multilink=", 1)[1] for line in output.splitlines()
                              if "multilink=" in line), "Is the test APK installed?")
                raise RuntimeError(f"Could not start VPN test: {error}")
            tunnel_device = route_check(adb, "198.19.255.53")
            if tunnel_device and tunnel_device.startswith("tun"):
                break
            time.sleep(0.5)
        else:
            raise RuntimeError("VPN routes did not appear in 25 seconds.")

        failures = 0
        lan_addresses = dict.fromkeys([*map(str, options.lan_ip),
                                       *(urlsplit(url).hostname for url in options.lan)])
        for address, expect_tun in [("1.1.1.1", True), ("198.19.255.53", True),
                                    *((address, False) for address in lan_addresses)]:
            device = route_check(adb, address)
            passed = device is not None and device.startswith("tun") == expect_tun
            failures += not passed
            log.write(json.dumps({"label": label, "check": "route", "host": address,
                                  "device": device, "expected_tun": expect_tun, "passed": passed}) + "\n")
            print(f"route {address} via {device}: {'ok' if passed else 'FAIL'}")
        failures += collect_matrix(adb, options, log, label, ("phone", "core"))
        failures += write_route_matches(adb, since, label, log, [*SITES.values(), *options.site],
                                        options.expected)
        for address in options.lan_ip:
            ping = adb_shell(adb, ["ping", "-c", "2", "-W", "1", str(address)])
            passed = ping.returncode == 0
            log.write(json.dumps({"label": label, "check": "lan-ping", "host": str(address),
                                  "passed": passed}) + "\n")
            print(f"ping {address}: {'ok' if passed else 'no response'}")
    finally:
        if process.poll() is None:
            adb_shell(adb, ["run-as", "ru.everty.multilink.debug", "touch",
                            f"files/network-smoke-{token}.stop"])
        try:
            output = process.communicate(timeout=12)[0]
        except subprocess.TimeoutExpired:
            process.kill()
            process.communicate()
            raise RuntimeError("Test runner did not acknowledge shutdown.")
    if "INSTRUMENTATION_CODE: -1" not in output or route_check(adb, "1.1.1.1") != baseline_device:
        raise RuntimeError("VPN test did not finish with the original network route restored.")
    return int(bool(failures))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--adb", default=adb_path())
    parser.add_argument("--serial", help="ADB device serial if several devices are connected")
    parser.add_argument("--label", required=True, help="Run label, e.g. vpn-off or vpn-on-balance")
    parser.add_argument("--log", type=Path, required=True, help="Append JSON Lines results to this file")
    parser.add_argument("--repeat", type=int, choices=range(1, 6), default=2)
    parser.add_argument("--mode", choices=("phone", "core", "both"), default="phone")
    parser.add_argument("--compare", action="store_true", help="Run VPN off/on using the debug test APK")
    parser.add_argument("--policy", action="append", choices=("BALANCE", "LATENCY", "STABLE", "SMART"),
                        help="Policy to test with --compare. Repeat to compare several policies.")
    parser.add_argument("--site", action="append", default=[], help="Extra public HTTPS URL")
    parser.add_argument("--expect", action="append", default=[], metavar="HOST=DIRECT|PROXY",
                        help="Assert the Mihomo exit for a tested hostname with --compare")
    parser.add_argument("--lan", action="append", type=lan_url, default=[], help="Known HTTP(S) LAN endpoint")
    parser.add_argument("--lan-ip", action="append", type=ipaddress.ip_address, default=[],
                        help="Verify the route to a LAN IP even when it has no HTTP server")
    options = parser.parse_args()
    adb = [options.adb, *(["-s", options.serial] if options.serial else [])]
    try:
        check = adb_shell(adb, ["curl", "--version"])
    except (OSError, subprocess.TimeoutExpired) as error:
        parser.exit(2, f"ADB unavailable: {error}\n")
    if check.returncode:
        parser.exit(2, "ADB needs an authorized device with curl in its Android shell.\n")

    for site in options.site:
        parsed = urlsplit(site)
        if parsed.scheme != "https" or not parsed.hostname or parsed.username is not None or parsed.query or parsed.fragment:
            parser.error("--site requires an HTTPS URL without credentials, query or fragment")
    tested = {urlsplit(url).hostname for url in [*SITES.values(), *options.site]}
    options.expected = {}
    for entry in options.expect:
        host, _, target = entry.partition("=")
        if not options.compare or host not in tested or target not in ("DIRECT", "PROXY"):
            parser.error("--expect requires --compare and HOST=DIRECT or HOST=PROXY for a tested host")
        options.expected[host] = target
    modes = ("phone", "core") if options.mode == "both" else (options.mode,)
    options.log.parent.mkdir(parents=True, exist_ok=True)
    try:
        with options.log.open("a", encoding="utf-8") as log:
            if options.compare:
                baseline_device = route_check(adb, "1.1.1.1")
                if baseline_device is None or baseline_device.startswith("tun"):
                    raise RuntimeError("Disconnect the active VPN and check Internet access before comparison.")
                collect_matrix(adb, options, log, options.label + "-off", ("phone",))
                failures = sum(compare(adb, options, log, policy, baseline_device)
                               for policy in options.policy or (None,))
                return int(bool(failures))
            return int(bool(collect_matrix(adb, options, log, options.label, modes)))
    except (RuntimeError, OSError, subprocess.TimeoutExpired) as error:
        parser.exit(2, f"{error}\n")


if __name__ == "__main__":
    sys.exit(main())
