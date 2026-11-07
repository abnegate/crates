"""Greeting helpers — 挨拶 and emoji 👋 support."""

GREETINGS = {
    "ja": "こんにちは世界",
    "zh": "你好，世界",
    "ko": "안녕하세요",
    "emoji": "👋🌍✨",
}


def greet(language: str, name: str) -> str:
    """Return a greeting such as 'こんにちは, 太郎' for the language code."""
    template = GREETINGS.get(language, "Hello")
    return f"{template}, {name} 🎉"


def widths(text: str) -> list:
    # Count east asian wide characters: 漢字 take two columns.
    return [2 if ord(ch) > 0x2E80 else 1 for ch in text]
