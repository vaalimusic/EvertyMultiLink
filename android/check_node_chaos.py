"""Controlled comparison of HEAD health checks and full-response ranking."""
from concurrent.futures import ThreadPoolExecutor
from contextlib import ExitStack
from http.client import HTTPConnection, HTTPException
import json
import random
import time
import uuid

from check_resilience import core, endpoint, mixed_port, proxy, request, require
from site_verification import verify_site


PAGE = "http://site.test/page"


def measure(listener, target=PAGE):
    start = time.monotonic()
    try:
        status, body = request(listener, target, timeout=0.7)
        return {"status": status, "body": body, "ms": round((time.monotonic() - start) * 1000)}
    except (HTTPException, OSError) as error:
        return {"error": type(error).__name__, "ms": round((time.monotonic() - start) * 1000)}


def chaos_cases(suite, binary, rounds, seed):
    rng = random.Random(seed)
    names = ("A", "B", "C")
    states = dict.fromkeys(names, "healthy")
    with ThreadPoolExecutor(max_workers=3) as probes:
        with ExitStack() as stack:
            entries = []
            listeners = {}
            while len(listeners) < len(names):
                name = names[len(listeners)]
                port = mixed_port()
                if port not in listeners.values():
                    listeners[name] = port
            for name in names:
                port = stack.enter_context(endpoint(name, fault=lambda name=name: states[name]))
                entries.append(proxy(name, port))
            baseline = {"name": "POOL", "type": "fallback", "proxies": list(names),
                        "url": "http://site.test/empty", "expected-status": "204",
                        "interval": 1, "lazy": False, "timeout": 500}
            inbounds = [{"name": f"probe-{name}", "type": "http", "listen": "127.0.0.1",
                         "port": port, "proxy": name} for name, port in listeners.items()]
            controller = mixed_port()
            while controller in listeners.values():
                controller = mixed_port()
            secret = uuid.uuid4().hex
            ranked_port = mixed_port()
            while ranked_port in (*listeners.values(), controller):
                ranked_port = mixed_port()
            inbounds.append({"name": "ranked", "type": "http", "listen": "127.0.0.1",
                             "port": ranked_port, "proxy": "RANKED"})
            selector = {"name": "RANKED", "type": "select", "proxies": ["REJECT", *names]}
            mixed = stack.enter_context(core(binary, ["MATCH,POOL"], entries, [baseline, selector],
                                             listeners=inbounds, controller=(controller, secret)))
            selected = "A"

            def rank():
                results = dict(zip(names, probes.map(measure, listeners.values())))
                valid = []
                for name, result in results.items():
                    if result.get("status") != 200:
                        continue
                    result["content_check"] = verify_site("fixture", PAGE, PAGE, "text/html",
                                                          result["body"], marker=name)
                    if result["content_check"] == "verified_content":
                        valid.append(name)
                return min(valid, key=lambda name: (results[name]["ms"], name), default=None), results

            def apply(chosen):
                conn = HTTPConnection("127.0.0.1", controller, timeout=2)
                try:
                    conn.request("PUT", "/proxies/RANKED", json.dumps({"name": chosen or "REJECT"}),
                                 {"Authorization": f"Bearer {secret}", "Content-Type": "application/json"})
                    response = conn.getresponse()
                    response.read()
                    require(response.status == 204, f"Selector returned HTTP {response.status}")
                finally:
                    conn.close()
                outcome = measure(ranked_port)
                require(outcome.get("body") == chosen if chosen else outcome.get("status", 0) != 200,
                        outcome)
                return outcome

            def half_response():
                states.update(A="truncated", B="slow", C="offline")
                health = measure(listeners["A"], "http://site.test/empty")
                before = measure(mixed)
                chosen, results = rank()
                require(health.get("status") == 204 and before.get("error") == "IncompleteRead"
                        and chosen == "B", {"health": health, "before": before, "results": results})
                return {"health": health, "baseline": before, "selected": chosen, "probes": results,
                        "after_switch": apply(chosen)}

            suite.check("HTTP 200 with truncated body defeats normal health check", half_response)

            def complete_placeholder(fault, verdict):
                states.update(A=fault, B="slow", C="offline")
                baseline_result = measure(mixed)
                chosen, results = rank()
                require(baseline_result.get("status") == 200 and chosen == "B"
                        and results["A"]["content_check"] == verdict, results)
                return {"baseline": baseline_result, "selected": chosen, "after_switch": apply(chosen)}

            suite.check("Complete HTTP 200 block page loses ranking",
                        lambda: complete_placeholder("blockpage", "block_or_challenge"))
            suite.check("Unknown HTTP 200 placeholder loses ranking",
                        lambda: complete_placeholder("placeholder", "unexpected_content"))
            for title, effective, mime, body, expected in (
                ("Redirect to an unrelated host", "https://youtube.com.block.invalid/", "text/html",
                 "ytInitialData ytimg.com", "unexpected_redirect"),
                ("Same-site challenge overrides markers", "https://www.youtube.com/checkpoint/", "text/html",
                 "ytInitialData ytimg.com", "block_or_challenge"),
                ("Brand name alone is inconclusive", "https://www.youtube.com/", "text/html",
                 "<title>YouTube</title>", "unverified_content"),
                ("HTML markers in a text response are inconclusive", "https://www.youtube.com/", "text/plain",
                 "ytInitialData ytimg.com", "unverified_content"),
            ):
                def verify(effective=effective, mime=mime, body=body, expected=expected):
                    actual = verify_site("youtube", "https://www.youtube.com/", effective, mime, body)
                    return require(actual == expected, f"{actual}, expected {expected}")
                suite.check(title, verify)

            def round_trip(number):
                nonlocal selected
                failed = selected or rng.choice(names)
                alternative = rng.choice([name for name in names if name != failed])
                fault = rng.choice(("offline", "truncated", "blockpage", "placeholder"))
                states.update(dict.fromkeys(names, "slow"))
                states[failed] = fault
                states[alternative] = "healthy"
                if number % 8 == 0:
                    states.update(dict.fromkeys(names, "offline"))
                start = time.monotonic()
                chosen, results = rank()
                applied = apply(chosen)
                elapsed = round((time.monotonic() - start) * 1000)
                if number % 8 == 0:
                    require(chosen is None, results)
                    states[alternative] = "healthy"
                    recovered, recovery = rank()
                    require(recovered == alternative, recovery)
                    apply(recovered)
                    selected = recovered
                else:
                    require(chosen == alternative, results)
                    selected = chosen
                require(elapsed <= 1500, f"Ranking took {elapsed} ms")
                return {"seed": seed, "round": number, "injected": fault, "previous": failed,
                        "all_offline": number % 8 == 0, "selected": chosen,
                        "after_recovery": selected, "ranking_ms": elapsed, "probes": results,
                        "after_switch": applied}

            for number in range(1, rounds + 1):
                suite.check(f"Seeded node fault and reranking #{number}", lambda n=number: round_trip(n))
