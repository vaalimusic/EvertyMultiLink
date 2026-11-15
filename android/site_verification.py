"""Classify a completed response without recording page content in logs."""
import re
from urllib.parse import unquote, urlsplit


# Require independent markers. A title or a brand name alone is easy to mimic.
PROFILES = {
    "yandex": (("ya.ru", "yandex.ru"), (r"yandex\.|яндекс", r"yastatic\.net|name=[\"']text[\"']")),
    "gosuslugi": (("gosuslugi.ru",), (r"госуслуг|gosuslugi", r"app-root|\.js\b|\.css\b")),
    "vk": (("vk.com",), (r"вконтакте|\bvk\b", r"vk\.me|vk\.com|vkuseraudio|vkcdn")),
    "youtube": (("youtube.com",), (r"ytinitialdata|ytcfg\.set", r"ytimg\.com|youtubei")),
    "facebook": (("facebook.com",), (r"facebook", r"fbcdn\.net|\b__bbox\b")),
}
BLOCK_TEXT = re.compile(
    r"доступ к (?:сайту|ресурсу) ограничен|ресурс заблокирован|"
    r"access to this (?:site|website) has been blocked|blocked by your (?:isp|provider)|"
    r"this website is blocked|user location is not supported",
    re.IGNORECASE,
)
INTERSTITIAL_PATH = re.compile(
    r"(?:^|/)(?:sorry|blocked|blockpage|checkpoint|captcha|challenge)(?:/|$)",
    re.IGNORECASE,
)


def verify_site(name, requested, effective, content_type, body, marker=None):
    try:
        source = urlsplit(requested)
        target = urlsplit(effective)
        host = (target.hostname or "").rstrip(".").lower()
    except ValueError:
        return "unverified_content"
    if not host:
        return "unverified_content"
    if target.scheme != source.scheme or target.username is not None:
        return "unexpected_redirect"
    if BLOCK_TEXT.search(body) or INTERSTITIAL_PATH.search(unquote(target.path)):
        return "block_or_challenge"
    allowed, signatures = PROFILES.get(name, ((source.hostname,), ()))
    if not any(domain and (host == domain or host.endswith("." + domain)) for domain in allowed):
        return "unexpected_redirect"
    if marker is not None:
        return "verified_content" if body.strip() == marker else "unexpected_content"
    if not signatures or not content_type.lower().startswith(("text/html", "application/xhtml+xml")):
        return "unverified_content"
    if all(re.search(signature, body, re.IGNORECASE) for signature in signatures):
        return "verified_content"
    return "unverified_content"
