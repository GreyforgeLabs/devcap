"""Text cleanup helpers for terminal and Markdown output."""

from __future__ import annotations

import html
import re

ANSI_ESCAPE_RE = re.compile(r"\x1b(?:[@-Z\\-_]|\[[0-?]*[ -/]*[@-~])")
CONTROL_RE = re.compile(r"[\x00-\x08\x0b\x0c\x0e-\x1f\x7f-\x9f]")


def clean_text(value: object, *, max_length: int | None = None) -> str:
    """Return a single-line string without terminal control sequences."""
    text = "" if value is None else str(value)
    text = ANSI_ESCAPE_RE.sub("", text)
    text = CONTROL_RE.sub(" ", text)
    text = text.replace("\r", " ").replace("\n", " ").replace("\t", " ")
    text = " ".join(text.split())
    if max_length is not None and len(text) > max_length:
        return text[: max_length - 3] + "..."
    return text


def markdown_cell(value: object, *, max_length: int | None = 240) -> str:
    """Escape a value for use inside a Markdown table cell."""
    text = html.escape(clean_text(value, max_length=max_length), quote=False)
    return text.replace("\\", "\\\\").replace("|", "\\|").replace("`", "\\`")


def markdown_inline_code(value: object, *, max_length: int | None = 120) -> str:
    """Format a sanitized value as Markdown inline code."""
    text = markdown_cell(value, max_length=max_length)
    return f"`{text}`"
